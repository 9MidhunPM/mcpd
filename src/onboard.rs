use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, IsTerminal, Write},
};

use serde::Serialize;

use crate::{
    Paths, adopt, config,
    diagnostics::{Result, SyncplaneError},
    import,
    model::{CanonicalConfig, Server},
    sync,
    targets::{self, ImportMode},
};

#[derive(Debug, Clone, Serialize)]
pub struct OnboardEntry {
    pub name: String,
    pub found_in: Vec<String>,
    pub transport: &'static str,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OnboardReport {
    pub dry_run: bool,
    pub entries: Vec<OnboardEntry>,
    pub canonical_servers: usize,
    pub enabled_targets: Vec<String>,
    pub imported: Vec<String>,
    pub skipped_conflicts: Vec<String>,
    pub unsupported: Vec<String>,
    pub sync: Option<sync::SyncBatch>,
}

#[derive(Clone)]
struct Source {
    target: String,
    server: Server,
}

/// Fresh-install orchestration. Target-specific import remains the precise,
/// advanced command; this layer only coordinates its existing primitives.
pub fn import_all(
    paths: &Paths,
    requested_targets: &[String],
    dry_run: bool,
    yes: bool,
    no_sync: bool,
) -> Result<OnboardReport> {
    let requested_targets = requested_targets
        .iter()
        .map(|target| {
            if target == "openchamber" {
                "opencode".into()
            } else {
                target.clone()
            }
        })
        .collect::<Vec<_>>();
    let canonical = if paths.config.exists() {
        config::load(&paths.config)?
    } else {
        CanonicalConfig {
            version: 1,
            servers: BTreeMap::new(),
            targets: BTreeMap::new(),
        }
    };
    let adapters = targets::adapters(paths)?
        .into_iter()
        .filter(|adapter| {
            (requested_targets.is_empty()
                || requested_targets
                    .iter()
                    .any(|target| target == adapter.id()))
                && adapter.config_path().exists()
        })
        .collect::<Vec<_>>();
    if !requested_targets.is_empty() {
        for target in &requested_targets {
            if !adapters.iter().any(|adapter| adapter.id() == target) {
                return Err(SyncplaneError::TargetUnavailable {
                    target: target.clone(),
                    message: "no readable target configuration was found".into(),
                    hint: "install/configure the client first, or omit --target to scan available clients".into(),
                });
            }
        }
    }

    let mut sources = BTreeMap::<String, Vec<Source>>::new();
    let mut unsupported = Vec::new();
    for adapter in &adapters {
        let mappings = import::resolve_secret_mappings(adapter.id(), None, paths, BTreeMap::new())?;
        let imported = adapter.import(None, &mappings, ImportMode::BestEffort)?;
        for skipped in imported.skipped {
            unsupported.push(format!(
                "{}: {} ({})",
                adapter.display_name(),
                skipped.name,
                skipped.reason
            ));
        }
        for (name, entry) in imported.servers {
            sources.entry(name).or_default().push(Source {
                target: adapter.id().into(),
                server: entry.server,
            });
        }
    }

    let mut chosen = BTreeMap::<String, Source>::new();
    let mut entries = Vec::new();
    let mut skipped_conflicts = Vec::new();
    for (name, group) in &sources {
        let distinct = group
            .iter()
            .fold(Vec::<&Server>::new(), |mut values, source| {
                if !values.iter().any(|value| **value == source.server) {
                    values.push(&source.server);
                }
                values
            });
        let existing = canonical.servers.get(name);
        let mut status = if existing.is_some() {
            "existing canonical".to_owned()
        } else if distinct.len() == 1 {
            if group.len() == 1 {
                "unique".into()
            } else {
                "equivalent duplicate".into()
            }
        } else if yes {
            format!(
                "conflict: using first discovered (--yes); {}",
                redacted_semantic_diff(&group[0].server, &group[1].server)
            )
        } else {
            format!(
                "conflict: needs a choice; {}",
                redacted_semantic_diff(&group[0].server, &group[1].server)
            )
        };
        if existing.is_none() && distinct.len() > 1 && !yes && !dry_run && io::stdin().is_terminal()
        {
            eprintln!("Conflict for `{name}`:");
            for (index, source) in group.iter().enumerate() {
                eprintln!(
                    "  {}) {} ({})",
                    index + 1,
                    targets::display_name(&source.target),
                    transport_name(&source.server)
                );
            }
            eprint!("Choose a canonical definition, or press Enter to skip: ");
            io::stderr()
                .flush()
                .map_err(|source| SyncplaneError::io("<terminal>", source))?;
            let mut answer = String::new();
            io::stdin()
                .read_line(&mut answer)
                .map_err(|source| SyncplaneError::io("<terminal>", source))?;
            if let Ok(index) = answer.trim().parse::<usize>()
                && let Some(source) = group.get(index.saturating_sub(1))
            {
                chosen.insert(name.clone(), source.clone());
                status = format!(
                    "conflict: selected {}",
                    targets::display_name(&source.target)
                );
            } else {
                skipped_conflicts.push(name.clone());
            }
        } else if existing.is_none() && (distinct.len() == 1 || yes) {
            chosen.insert(name.clone(), group[0].clone());
        } else if existing.is_none() && distinct.len() > 1 {
            skipped_conflicts.push(name.clone());
        }
        entries.push(OnboardEntry {
            name: name.clone(),
            found_in: group
                .iter()
                .map(|source| targets::display_name(&source.target).to_owned())
                .collect(),
            transport: transport_name(&group[0].server),
            status,
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    if dry_run {
        return Ok(OnboardReport {
            dry_run: true,
            entries,
            canonical_servers: canonical.servers.len() + chosen.len(),
            enabled_targets: adapters.iter().map(|adapter| adapter.id().into()).collect(),
            imported: chosen.keys().cloned().collect(),
            skipped_conflicts,
            unsupported,
            sync: None,
        });
    }
    if !paths.config.exists() {
        config::init(paths)?;
    }
    // Each single-server import retains its existing canonical/keyring/state
    // transaction and grouped secret confirmation. A later source is adopted,
    // rather than re-imported, so duplicate definitions remain one canonical MCP.
    let mut imported_names = Vec::new();
    for (name, source) in &chosen {
        let selection = BTreeSet::from([name.clone()]);
        let mappings = import::resolve_secret_mappings(
            &source.target,
            Some(&selection),
            paths,
            BTreeMap::new(),
        )?;
        let report = import::import_target(
            &source.target,
            Some(&selection),
            paths,
            false,
            true,
            mappings,
        )?;
        if !report.imported.is_empty() {
            imported_names.push(name.clone());
        }
    }
    let resolved = crate::resolve::load(paths)?.config;
    let enabled_targets = adapters
        .iter()
        .map(|adapter| adapter.id().to_owned())
        .collect::<Vec<_>>();
    for target in &enabled_targets {
        config::set_target_enabled(&paths.config, target, true)?;
    }
    if no_sync {
        return Ok(OnboardReport {
            dry_run: false,
            entries,
            canonical_servers: resolved.servers.len(),
            enabled_targets,
            imported: imported_names,
            skipped_conflicts,
            unsupported,
            sync: None,
        });
    }
    // Claim equivalent copies and explicitly replace only sources selected for
    // a resolved conflict. Errors are deferred to isolated sync reporting.
    for (name, group) in &sources {
        if !resolved.servers.contains_key(name) {
            continue;
        }
        for source in group {
            let selection = BTreeSet::from([name.clone()]);
            let replace = resolved
                .servers
                .get(name)
                .is_some_and(|wanted| wanted != &source.server);
            let _ = adopt::adopt(&source.target, Some(&selection), paths, false, replace);
        }
    }
    let final_config = crate::resolve::load(paths)?.config;
    let sync = sync::sync_selected_targets_isolated_with_policy(
        &final_config,
        paths,
        &enabled_targets,
        false,
        &sync::RemovalPolicy::Deny,
    )?;
    Ok(OnboardReport {
        dry_run: false,
        entries,
        canonical_servers: final_config.servers.len(),
        enabled_targets,
        imported: imported_names,
        skipped_conflicts,
        unsupported,
        sync: Some(sync),
    })
}

fn transport_name(server: &Server) -> &'static str {
    match server {
        Server::Stdio { .. } => "stdio",
        Server::Http { .. } => "http",
    }
}

fn redacted_semantic_diff(left: &Server, right: &Server) -> String {
    match (left, right) {
        (
            Server::Stdio {
                command: a_command,
                args: a_args,
                env: a_env,
                cwd: a_cwd,
                ..
            },
            Server::Stdio {
                command: b_command,
                args: b_args,
                env: b_env,
                cwd: b_cwd,
                ..
            },
        ) => {
            let mut fields = Vec::new();
            if a_command != b_command {
                fields.push("command <redacted>");
            }
            if a_args != b_args {
                fields.push("arguments");
            }
            if a_env != b_env {
                fields.push("environment <redacted>");
            }
            if a_cwd != b_cwd {
                fields.push("working directory <redacted>");
            }
            format!("definition differs: {}", fields.join(", "))
        }
        (
            Server::Http {
                url: a_url,
                headers: a_headers,
                ..
            },
            Server::Http {
                url: b_url,
                headers: b_headers,
                ..
            },
        ) => {
            let mut fields = Vec::new();
            if a_url != b_url {
                fields.push("URL <redacted>");
            }
            if a_headers != b_headers {
                fields.push("headers <redacted>");
            }
            format!("definition differs: {}", fields.join(", "))
        }
        _ => "definition differs: transport".into(),
    }
}
