use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{
    Paths,
    diagnostics::{McpdError, Result},
    model::{CanonicalConfig, Server},
    state::{self, TargetState},
    sync,
    targets::{self, ImportMode},
};

#[derive(Debug, Clone, Serialize)]
pub struct Adopted {
    pub name: String,
    pub replaced: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub name: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct AdoptReport {
    pub target: String,
    pub dry_run: bool,
    pub adopted: Vec<Adopted>,
    pub skipped: Vec<Skipped>,
    pub warnings: Vec<String>,
}

pub fn adopt(
    target: &str,
    selection: Option<&BTreeSet<String>>,
    paths: &Paths,
    dry_run: bool,
    replace: bool,
) -> Result<AdoptReport> {
    let canonical = crate::resolve::load(paths)?.config;
    let adapter = targets::adapter(target, paths)?;
    let state_path = paths.state_dir.join("state.toml");
    let mut state_file = state::load(&state_path)?;
    let previous = state_file
        .targets
        .get(target)
        .filter(|s| {
            s.config_path == adapter.config_path() && s.adapter_version == adapter.adapter_version()
        })
        .cloned();
    let names = selection
        .cloned()
        .unwrap_or_else(|| canonical.servers.keys().cloned().collect());
    let mut next = previous.clone().unwrap_or(TargetState {
        config_path: adapter.config_path().to_path_buf(),
        adapter_version: adapter.adapter_version(),
        last_success_unix_ms: 0,
        managed: BTreeMap::new(),
    });
    let mut adopted = Vec::new();
    let mut skipped = Vec::new();
    let mut replacements = BTreeMap::new();
    for name in names {
        let Some(wanted) = canonical.servers.get(&name) else {
            skipped.push(Skipped {
                name,
                reason: "not present in resolved canonical configuration".into(),
            });
            continue;
        };
        if next.managed.contains_key(&name) {
            continue;
        }
        let selected = BTreeSet::from([name.clone()]);
        match adapter.import(Some(&selected), &BTreeMap::new(), ImportMode::Strict) {
            Ok(found) => match found.servers.get(&name) {
                Some(entry) if entry.server == *wanted => {
                    next.managed.insert(name.clone(), entry.ownership.clone());
                    adopted.push(Adopted {
                        name,
                        replaced: false,
                    });
                }
                Some(entry) if replace => {
                    next.managed.insert(name.clone(), entry.ownership.clone());
                    replacements.insert(name.clone(), entry.ownership.clone());
                    adopted.push(Adopted {
                        name,
                        replaced: true,
                    });
                }
                Some(entry) => skipped.push(Skipped {
                    name,
                    reason: semantic_diff(wanted, &entry.server),
                }),
                None => skipped.push(Skipped {
                    name,
                    reason: "target MCP entry does not exist".into(),
                }),
            },
            Err(_) => skipped.push(Skipped {
                name,
                reason: "target definition cannot be compared safely without exposing secrets"
                    .into(),
            }),
        }
    }
    // Bulk adoption is intentionally all-or-nothing: it must never leave a
    // surprising subset of the canonical servers newly owned.
    if !skipped.is_empty() && (selection.is_none() || adopted.is_empty()) {
        return Err(McpdError::Conflict {
            message: format!("cannot adopt `{}`", skipped[0].name),
            hint: skipped[0].reason.clone(),
        });
    }
    if dry_run {
        return Ok(AdoptReport {
            target: target.into(),
            dry_run: true,
            adopted,
            skipped,
            warnings: adapter.warnings(),
        });
    }
    sync::with_sync_lock(paths, || {
        if !replacements.is_empty() {
            let scoped = scoped_config(&canonical, target, &replacements);
            // The adapter needs temporary ownership of only the entries being
            // replaced, so it can safely render them without inspecting or
            // reconciling unrelated target collisions.
            let mut scoped_state = previous.clone().unwrap_or(TargetState {
                config_path: adapter.config_path().to_path_buf(),
                adapter_version: adapter.adapter_version(),
                last_success_unix_ms: 0,
                managed: BTreeMap::new(),
            });
            scoped_state.managed.extend(replacements.clone());
            let mut plan = adapter.plan(&scoped, Some(&scoped_state))?;
            // `plan` intentionally contains only the replacement scope. Keep
            // all pre-existing ownership and add equivalent adoptions.
            plan.next_state = next.clone();
            plan.next_state.config_path = adapter.config_path().to_path_buf();
            plan.next_state.adapter_version = adapter.adapter_version();
            sync::commit_adoption(paths, &mut state_file, plan)
        } else {
            next.last_success_unix_ms = sync::now_ms()?;
            state_file.targets.insert(target.into(), next);
            state::save(&state_path, &state_file)
        }
    })?;
    Ok(AdoptReport {
        target: target.into(),
        dry_run: false,
        adopted,
        skipped,
        warnings: adapter.warnings(),
    })
}

fn scoped_config(
    canonical: &CanonicalConfig,
    target: &str,
    replacements: &BTreeMap<String, state::ManagedServer>,
) -> CanonicalConfig {
    let selected = replacements.keys().cloned().collect::<BTreeSet<_>>();
    let mut scoped = canonical.clone();
    scoped.servers.retain(|name, _| selected.contains(name));
    if let Some(target_config) = scoped.targets.get_mut(target) {
        target_config
            .servers
            .retain(|name, _| selected.contains(name));
    }
    scoped
}

fn semantic_diff(canonical: &Server, target: &Server) -> String {
    let mut differences = Vec::new();
    match (canonical, target) {
        (
            Server::Stdio {
                command: left_command,
                args: left_args,
                env: left_env,
                cwd: left_cwd,
                ..
            },
            Server::Stdio {
                command: right_command,
                args: right_args,
                env: right_env,
                cwd: right_cwd,
                ..
            },
        ) => {
            if left_command != right_command {
                differences.push("command (<redacted> -> <redacted>)".into());
            }
            if left_args != right_args {
                differences.push(format!(
                    "arguments ({} -> {})",
                    left_args.len(),
                    right_args.len()
                ));
            }
            if left_env != right_env {
                differences.push("environment (<redacted>)".into());
            }
            if left_cwd != right_cwd {
                differences.push("working directory (<redacted>)".into());
            }
        }
        (
            Server::Http {
                url: left_url,
                headers: left_headers,
                ..
            },
            Server::Http {
                url: right_url,
                headers: right_headers,
                ..
            },
        ) => {
            if left_url != right_url {
                differences.push("URL (<redacted> -> <redacted>)".into());
            }
            if left_headers != right_headers {
                differences.push("headers (<redacted>)".into());
            }
        }
        (Server::Stdio { .. }, Server::Http { .. })
        | (Server::Http { .. }, Server::Stdio { .. }) => {
            differences.push("transport (stdio -> HTTP or HTTP -> stdio)".into())
        }
    }
    format!("definition differs: {}", differences.join("; "))
}
