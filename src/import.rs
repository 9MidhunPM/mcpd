use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, IsTerminal, Write},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use crate::{
    Paths, config,
    diagnostics::{McpdError, Result},
    model::Server,
    state::{self, TargetState},
    sync::{self, fs::hash_bytes},
    targets::{self, ImportMode, ImportSkipped},
};

#[derive(Debug, Clone, Serialize)]
pub struct ImportedEntry {
    pub name: String,
    pub transport: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub target: String,
    pub dry_run: bool,
    pub imported: Vec<ImportedEntry>,
    pub skipped: Vec<ImportSkipped>,
    pub migrated_secrets: Vec<String>,
    pub warnings: Vec<String>,
}

struct PreparedImport {
    target: String,
    target_path: PathBuf,
    target_snapshot: Vec<u8>,
    canonical: config::AddServersPlan,
    next_state: TargetState,
    imported: Vec<ImportedEntry>,
    skipped: Vec<ImportSkipped>,
    secret_writes: Vec<crate::secrets::SecretWrite>,
    migrated_secrets: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PendingImport {
    version: u32,
    target: String,
    target_path: PathBuf,
    target_hash: String,
    canonical_pre_hash: String,
    canonical_desired_hash: String,
    next_state: TargetState,
}

pub fn import_target(
    target: &str,
    selection: Option<&BTreeSet<String>>,
    paths: &Paths,
    dry_run: bool,
    strict: bool,
    secret_mappings: BTreeMap<String, String>,
) -> Result<ImportReport> {
    let secret_mappings = resolve_secret_mappings(target, selection, paths, secret_mappings)?;
    if dry_run {
        let mut prepared = prepare(target, selection, paths, &secret_mappings, strict)?;
        validate_secret_destinations(paths, &mut prepared.secret_writes)?;
        return Ok(report(&prepared, true));
    }
    let mut preflight = prepare(target, selection, paths, &secret_mappings, strict)?;
    validate_secret_destinations(paths, &mut preflight.secret_writes)?;
    if preflight.imported.is_empty() {
        return Ok(report(&preflight, false));
    }

    sync::with_sync_lock(paths, || {
        let mut prepared = prepare(target, selection, paths, &secret_mappings, strict)?;
        confirm_secret_migrations(target, paths, &prepared, &secret_mappings, false)?;
        validate_secret_destinations(paths, &mut prepared.secret_writes)?;
        if prepared.imported.is_empty() {
            return Ok(report(&prepared, false));
        }
        verify_target_snapshot(&prepared)?;
        let current_canonical =
            fs::read(&paths.config).map_err(|source| McpdError::io(&paths.config, source))?;
        if current_canonical != prepared.canonical.before {
            return Err(McpdError::Conflict {
                message: format!(
                    "{} changed after import was planned",
                    paths.config.display()
                ),
                hint: "review the external edit and rerun import; no write was performed".into(),
            });
        }
        let pending = PendingImport {
            version: 1,
            target: prepared.target.clone(),
            target_path: prepared.target_path.clone(),
            target_hash: hash_bytes(&prepared.target_snapshot),
            canonical_pre_hash: hash_bytes(&prepared.canonical.before),
            canonical_desired_hash: hash_bytes(&prepared.canonical.rendered),
            next_state: prepared.next_state.clone(),
        };
        let completed_report = report(&prepared, false);
        let store = crate::secrets::SecretStore::discover(paths)?;
        let rollback = store.write_batch(prepared.secret_writes)?;
        if let Err(error) = save_pending(paths, &pending) {
            store.rollback(rollback)?;
            return Err(error);
        }
        if let Err(error) = config::apply_add_servers(&paths.config, &prepared.canonical) {
            remove_pending(paths)?;
            store.rollback(rollback)?;
            return Err(error);
        }
        commit_state(paths, &pending.target, pending.next_state)?;
        remove_pending(paths)?;
        Ok(completed_report)
    })
}

fn validate_secret_destinations(
    paths: &Paths,
    writes: &mut Vec<crate::secrets::SecretWrite>,
) -> Result<()> {
    let store = crate::secrets::SecretStore::discover(paths)?;
    let mut retained = Vec::new();
    for write in writes.drain(..) {
        match store.get(&write.name)? {
            None => retained.push(write),
            Some(existing) if existing.expose() == write.value.expose() => retained.push(write),
            Some(_) => {
                return Err(McpdError::Conflict {
                    message: format!(
                        "keyring secret `{}` already exists with a different value",
                        write.name
                    ),
                    hint: "choose a different keyring name; mcpd import never overwrites an existing secret"
                        .into(),
                });
            }
        }
    }
    *writes = retained;
    Ok(())
}

fn prepare(
    target: &str,
    selection: Option<&BTreeSet<String>>,
    paths: &Paths,
    secret_mappings: &BTreeMap<String, String>,
    strict: bool,
) -> Result<PreparedImport> {
    let adapter = targets::adapter(target, paths)?;
    let state_path = paths.state_dir.join("state.toml");
    let state_file = state::load(&state_path)?;
    let previous = state_file.targets.get(target).filter(|state| {
        state.config_path == adapter.config_path()
            && state.adapter_version == adapter.adapter_version()
    });
    let previously_managed = previous.map(|state| &state.managed);
    let bulk = selection.is_none();
    let selected = if let Some(selection) = selection {
        selection.clone()
    } else {
        adapter.server_names()?.into_iter().collect()
    };
    let canonical_before = config::load(&paths.config)?;
    let mut effective_selection = BTreeSet::new();
    let mut skipped = Vec::new();
    for name in selected {
        let reason = if previously_managed.is_some_and(|managed| managed.contains_key(&name)) {
            Some("already managed by mcpd".to_owned())
        } else if canonical_before.servers.contains_key(&name) {
            Some(format!("already exists in {}", paths.config.display()))
        } else {
            None
        };
        if let Some(reason) = reason {
            if !bulk || strict {
                return Err(McpdError::Conflict {
                    message: format!("server `{name}` {reason}"),
                    hint: "keep the target entry unmanaged or remove/rename the canonical entry explicitly"
                        .into(),
                });
            }
            skipped.push(ImportSkipped { name, reason });
        } else {
            effective_selection.insert(name);
        }
    }
    let mut imported = adapter.import(
        Some(&effective_selection),
        secret_mappings,
        if bulk && !strict {
            ImportMode::BestEffort
        } else {
            ImportMode::Strict
        },
    )?;
    skipped.append(&mut imported.skipped);

    let mut additions = BTreeMap::new();
    let mut ownership = previous
        .map(|state| state.managed.clone())
        .unwrap_or_default();
    let mut entries = Vec::new();
    for (name, entry) in imported.servers {
        entries.push(ImportedEntry {
            name: name.clone(),
            transport: transport_name(&entry.server),
        });
        additions.insert(name.clone(), entry.server);
        ownership.insert(name, entry.ownership);
    }
    let migrated_secrets = imported
        .secret_writes
        .iter()
        .map(|write| write.name.clone())
        .collect();

    let canonical = config::plan_add_servers(&paths.config, &additions)?;
    Ok(PreparedImport {
        target: imported.target,
        target_path: imported.path.clone(),
        target_snapshot: imported.snapshot,
        canonical,
        next_state: TargetState {
            config_path: imported.path,
            adapter_version: adapter.adapter_version(),
            last_success_unix_ms: sync::now_ms()?,
            managed: ownership,
        },
        imported: entries,
        skipped,
        secret_writes: imported.secret_writes,
        migrated_secrets,
        warnings: imported.warnings,
    })
}

fn transport_name(server: &Server) -> &'static str {
    match server {
        Server::Stdio { .. } => "stdio",
        Server::Http { .. } => "http",
    }
}

fn report(prepared: &PreparedImport, dry_run: bool) -> ImportReport {
    ImportReport {
        target: prepared.target.clone(),
        dry_run,
        imported: prepared.imported.clone(),
        skipped: prepared.skipped.clone(),
        migrated_secrets: prepared.migrated_secrets.clone(),
        warnings: prepared.warnings.clone(),
    }
}

pub(crate) fn resolve_secret_mappings(
    target: &str,
    selection: Option<&BTreeSet<String>>,
    paths: &Paths,
    mut mappings: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let adapter = targets::adapter(target, paths)?;
    let effective_selection = if let Some(selection) = selection {
        selection.clone()
    } else {
        let state_file = state::load(&paths.state_dir.join("state.toml"))?;
        let managed = state_file
            .targets
            .get(target)
            .filter(|state| {
                state.config_path == adapter.config_path()
                    && state.adapter_version == adapter.adapter_version()
            })
            .map(|state| &state.managed);
        adapter
            .server_names()?
            .into_iter()
            .filter(|name| !managed.is_some_and(|managed| managed.contains_key(name)))
            .collect()
    };
    let candidates = adapter.import_secret_candidates(Some(&effective_selection))?;
    for (server, fields) in group_sensitive_candidates(candidates) {
        let unmapped = fields
            .into_iter()
            .filter(|field| {
                !mappings.contains_key(field)
                    && !mappings.contains_key(&mapping_key(&server, field))
            })
            .collect::<Vec<_>>();
        for field in unmapped {
            let secret_name = automatic_secret_name(&server, &field);
            crate::secrets::validate_name(&secret_name)?;
            mappings.insert(mapping_key(&server, &field), secret_name);
        }
    }
    Ok(mappings)
}

fn confirm_secret_migrations(
    target: &str,
    paths: &Paths,
    prepared: &PreparedImport,
    mappings: &BTreeMap<String, String>,
    dry_run: bool,
) -> Result<()> {
    if dry_run || !io::stdin().is_terminal() || prepared.secret_writes.is_empty() {
        return Ok(());
    }
    let written = prepared
        .secret_writes
        .iter()
        .map(|write| &write.name)
        .collect::<BTreeSet<_>>();
    let adapter = targets::adapter(target, paths)?;
    let imported = prepared
        .imported
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<BTreeSet<_>>();
    for (server, fields) in
        group_sensitive_candidates(adapter.import_secret_candidates(Some(&imported))?)
    {
        let fields = fields
            .into_iter()
            .filter(|field| {
                mappings
                    .get(&mapping_key(&server, field))
                    .or_else(|| mappings.get(field))
                    .is_some_and(|name| written.contains(name))
            })
            .collect::<Vec<_>>();
        if fields.is_empty() {
            continue;
        }
        eprintln!("{server} contains sensitive environment values:\n");
        for field in &fields {
            eprintln!("  {field}");
        }
        eprint!("\nStore securely in the OS keyring? [Y/n] ");
        io::stderr()
            .flush()
            .map_err(|source| McpdError::io("<terminal>", source))?;
        let mut answer = String::new();
        io::stdin()
            .read_line(&mut answer)
            .map_err(|source| McpdError::io("<terminal>", source))?;
        if matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no") {
            return Err(McpdError::InvalidInput {
                message: format!("secure secret migration for `{server}` was declined"),
                hint: "no canonical configuration or keyring entries were modified".into(),
            });
        }
    }
    Ok(())
}

fn group_sensitive_candidates(
    candidates: Vec<crate::targets::ImportSecretCandidate>,
) -> BTreeMap<String, Vec<String>> {
    let mut grouped = BTreeMap::<String, BTreeSet<String>>::new();
    for candidate in candidates
        .into_iter()
        .filter(|candidate| candidate.sensitive)
    {
        grouped
            .entry(candidate.server)
            .or_default()
            .insert(candidate.field);
    }
    grouped
        .into_iter()
        .map(|(server, fields)| (server, fields.into_iter().collect()))
        .collect()
}

fn automatic_secret_name(server: &str, field: &str) -> String {
    format!("{server}.{field}")
}

fn mapping_key(server: &str, field: &str) -> String {
    format!("{server}\0{field}")
}

fn verify_target_snapshot(prepared: &PreparedImport) -> Result<()> {
    let current = fs::read(&prepared.target_path)
        .map_err(|source| McpdError::io(&prepared.target_path, source))?;
    if current != prepared.target_snapshot {
        return Err(McpdError::Conflict {
            message: format!(
                "{} changed after import was planned",
                prepared.target_path.display()
            ),
            hint: "review the external edit and rerun import; no write was performed".into(),
        });
    }
    Ok(())
}

fn pending_path(paths: &Paths) -> PathBuf {
    paths.state_dir.join("pending-import.toml")
}

fn save_pending(paths: &Paths, pending: &PendingImport) -> Result<()> {
    let text = toml::to_string_pretty(pending).map_err(|error| McpdError::Operational {
        message: format!("could not serialize pending import transaction: {error}"),
        hint: "report this as an mcpd bug".into(),
    })?;
    sync::fs::atomic_write(&pending_path(paths), text.as_bytes(), Some(0o600))
}

fn remove_pending(paths: &Paths) -> Result<()> {
    let path = pending_path(paths);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(McpdError::io(path, source)),
    }
}

fn commit_state(paths: &Paths, target: &str, next_state: TargetState) -> Result<()> {
    let path = paths.state_dir.join("state.toml");
    let mut state_file = state::load(&path)?;
    state_file.targets.insert(target.into(), next_state);
    state::save(&path, &state_file)
}

pub(crate) fn recover_pending(paths: &Paths) -> Result<()> {
    let path = pending_path(paths);
    if !path.exists() {
        return Ok(());
    }
    let text = fs::read_to_string(&path).map_err(|source| McpdError::io(&path, source))?;
    let pending: PendingImport =
        toml::from_str(&text).map_err(|error| McpdError::InvalidInput {
            message: format!(
                "pending import transaction {} is malformed: {error}",
                path.display()
            ),
            hint: "inspect canonical config and ownership state before removing it manually".into(),
        })?;
    if pending.version != 1 {
        return Err(McpdError::InvalidInput {
            message: "unsupported pending import transaction record".into(),
            hint: "use a compatible mcpd version".into(),
        });
    }
    let target = fs::read(&pending.target_path)
        .map_err(|source| McpdError::io(&pending.target_path, source))?;
    if hash_bytes(&target) != pending.target_hash {
        return Err(McpdError::Conflict {
            message: format!("{} changed during import recovery", pending.target_path.display()),
            hint: "inspect the target, canonical config, and ownership state; mcpd will not guess ownership"
                .into(),
        });
    }
    let canonical =
        fs::read(&paths.config).map_err(|source| McpdError::io(&paths.config, source))?;
    let canonical_hash = hash_bytes(&canonical);
    if canonical_hash == pending.canonical_desired_hash {
        commit_state(paths, &pending.target, pending.next_state)?;
        remove_pending(paths)
    } else if canonical_hash == pending.canonical_pre_hash {
        remove_pending(paths)
    } else {
        Err(McpdError::Conflict {
            message: format!("{} changed during import recovery", paths.config.display()),
            hint: "inspect canonical config and ownership state; mcpd will not guess ownership"
                .into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn completed_canonical_write_commits_ownership_during_recovery() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let paths = Paths {
            config: temp.path().join("config.toml"),
            state_dir: temp.path().join("state"),
            codex_config: home.join(".codex/config.toml"),
            home,
        };
        config::init(&paths).unwrap();
        fs::create_dir_all(paths.codex_config.parent().unwrap()).unwrap();
        let target = b"[mcp_servers.demo]\ncommand = 'demo'\n";
        fs::write(&paths.codex_config, target).unwrap();
        let mut additions = BTreeMap::new();
        additions.insert(
            "demo".into(),
            Server::Stdio {
                command: "demo".into(),
                args: Vec::new(),
                env: BTreeMap::new(),
                secrets: BTreeMap::new(),
                cwd: None,
            },
        );
        let canonical = config::plan_add_servers(&paths.config, &additions).unwrap();
        let mut managed = BTreeMap::new();
        managed.insert(
            "demo".into(),
            crate::state::ManagedServer {
                canonical_hash: "canonical".into(),
                rendered_hash: "rendered".into(),
            },
        );
        let next_state = TargetState {
            config_path: paths.codex_config.clone(),
            adapter_version: 1,
            last_success_unix_ms: 42,
            managed,
        };
        let pending = PendingImport {
            version: 1,
            target: "codex".into(),
            target_path: paths.codex_config.clone(),
            target_hash: hash_bytes(target),
            canonical_pre_hash: hash_bytes(&canonical.before),
            canonical_desired_hash: hash_bytes(&canonical.rendered),
            next_state: next_state.clone(),
        };
        save_pending(&paths, &pending).unwrap();
        config::apply_add_servers(&paths.config, &canonical).unwrap();

        recover_pending(&paths).unwrap();

        assert_eq!(
            state::load(&paths.state_dir.join("state.toml"))
                .unwrap()
                .targets["codex"],
            next_state
        );
        assert!(!pending_path(&paths).exists());
    }

    #[test]
    fn sensitive_candidates_are_grouped_once_per_server_with_deterministic_names() {
        let groups = group_sensitive_candidates(vec![
            crate::targets::ImportSecretCandidate {
                server: "github".into(),
                field: "GITHUB_TOKEN".into(),
                sensitive: true,
            },
            crate::targets::ImportSecretCandidate {
                server: "github".into(),
                field: "GITHUB_TOKEN".into(),
                sensitive: true,
            },
            crate::targets::ImportSecretCandidate {
                server: "github".into(),
                field: "API_URL".into(),
                sensitive: false,
            },
            crate::targets::ImportSecretCandidate {
                server: "dokploy".into(),
                field: "DOKPLOY_API_KEY".into(),
                sensitive: true,
            },
        ]);
        assert_eq!(groups["github"], vec!["GITHUB_TOKEN"]);
        assert_eq!(groups["dokploy"], vec!["DOKPLOY_API_KEY"]);
        assert_eq!(
            automatic_secret_name("github", "GITHUB_TOKEN"),
            "github.GITHUB_TOKEN"
        );
    }
}
