pub mod fs;

use std::{
    fs as stdfs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{
    Paths,
    diagnostics::{McpdError, Result},
    model::CanonicalConfig,
    state::{self, TargetState},
    targets::{TargetAdapter, TargetPlan, codex::CodexAdapter},
};

#[derive(Debug, Clone, Serialize)]
pub struct SyncReport {
    pub target: &'static str,
    pub path: PathBuf,
    pub dry_run: bool,
    pub changes: Vec<crate::targets::Change>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnabledTarget {
    Codex,
}

pub fn enabled_targets(config: &CanonicalConfig) -> Vec<EnabledTarget> {
    let mut targets = Vec::new();
    if config
        .targets
        .get("codex")
        .is_some_and(|target| target.enabled)
    {
        targets.push(EnabledTarget::Codex);
    }
    targets
}

pub fn plan_enabled_targets(config: &CanonicalConfig, paths: &Paths) -> Result<Vec<TargetPlan>> {
    enabled_targets(config)
        .into_iter()
        .map(|target| match target {
            EnabledTarget::Codex => plan_codex(config, paths),
        })
        .collect()
}

pub fn sync_enabled_targets(
    config: &CanonicalConfig,
    paths: &Paths,
    dry_run: bool,
) -> Result<Vec<SyncReport>> {
    enabled_targets(config)
        .into_iter()
        .map(|target| match target {
            EnabledTarget::Codex => sync_codex(config, paths, dry_run),
        })
        .collect()
}

#[derive(Debug, Serialize, Deserialize)]
struct PendingTransaction {
    version: u32,
    target: String,
    target_path: PathBuf,
    pre_hash: Option<String>,
    desired_hash: String,
    next_state: TargetState,
}

fn plan_codex(config: &CanonicalConfig, paths: &Paths) -> Result<TargetPlan> {
    let state_path = paths.state_dir.join("state.toml");
    let state = state::load(&state_path)?;
    let adapter = CodexAdapter::new(paths.codex_config.clone(), paths.home.clone());
    adapter.plan(config, state.targets.get(adapter.id()))
}

fn sync_codex(config: &CanonicalConfig, paths: &Paths, dry_run: bool) -> Result<SyncReport> {
    if dry_run {
        let plan = plan_codex(config, paths)?;
        return Ok(report(&plan, true));
    }
    with_sync_lock(paths, || {
        let state_path = paths.state_dir.join("state.toml");
        let mut state_file = state::load(&state_path)?;
        let adapter = CodexAdapter::new(paths.codex_config.clone(), paths.home.clone());
        let mut plan = adapter.plan(config, state_file.targets.get(adapter.id()))?;
        if plan.is_noop() {
            return Ok(report(&plan, false));
        }
        if plan.changes.is_empty() {
            plan.next_state.last_success_unix_ms = now_ms()?;
            state_file
                .targets
                .insert(plan.target.into(), plan.next_state.clone());
            state::save(&state_path, &state_file)?;
            return Ok(report(&plan, false));
        }

        verify_snapshot(&plan)?;
        backup(&plan, &paths.state_dir)?;
        plan.next_state.last_success_unix_ms = now_ms()?;
        let pending = PendingTransaction {
            version: 1,
            target: plan.target.into(),
            target_path: plan.path.clone(),
            pre_hash: plan.before.as_deref().map(fs::hash_bytes),
            desired_hash: fs::hash_bytes(&plan.rendered),
            next_state: plan.next_state.clone(),
        };
        save_pending(paths, &pending)?;
        fs::atomic_write(&plan.path, &plan.rendered, None)?;
        state_file
            .targets
            .insert(plan.target.into(), plan.next_state.clone());
        state::save(&state_path, &state_file)?;
        remove_pending(paths)?;
        Ok(report(&plan, false))
    })
}

pub(crate) fn with_sync_lock<T>(paths: &Paths, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    ensure_private_dir(&paths.state_dir)?;
    let lock_path = paths.state_dir.join("sync.lock");
    let lock = stdfs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|source| McpdError::io(&lock_path, source))?;
    lock.lock_exclusive()
        .map_err(|source| McpdError::io(&lock_path, source))?;
    recover_pending(paths)?;
    crate::import::recover_pending(paths)?;
    operation()
}

fn report(plan: &TargetPlan, dry_run: bool) -> SyncReport {
    SyncReport {
        target: plan.target,
        path: plan.path.clone(),
        dry_run,
        changes: plan.changes.clone(),
    }
}

fn backup(plan: &TargetPlan, state_dir: &Path) -> Result<()> {
    let Some(before) = &plan.before else {
        return Ok(());
    };
    let dir = state_dir.join("backups").join(plan.target);
    ensure_private_dir(&dir)?;
    let stamp = now_ms()?;
    let mut selected = None;
    for suffix in 0..100_u8 {
        let path = dir.join(format!("{stamp}-{suffix:02}-config.toml"));
        let mut options = stdfs::OpenOptions::new();
        options.create_new(true).write(true).mode(0o600);
        match options.open(&path) {
            Ok(file) => {
                selected = Some((path, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(McpdError::io(path, source)),
        }
    }
    let (path, mut file) = selected.ok_or_else(|| McpdError::Operational {
        message: format!("could not choose a unique backup name in {}", dir.display()),
        hint: "retry after checking the backup directory".into(),
    })?;
    file.write_all(before)
        .map_err(|source| McpdError::io(&path, source))?;
    file.sync_all()
        .map_err(|source| McpdError::io(&path, source))?;
    Ok(())
}

fn verify_snapshot(plan: &TargetPlan) -> Result<()> {
    let current = match stdfs::read(&plan.path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(McpdError::io(&plan.path, source)),
    };
    if current != plan.before {
        return Err(McpdError::Conflict {
            message: format!("{} changed after it was planned", plan.path.display()),
            hint: "review the external edit and rerun sync; no target write was performed".into(),
        });
    }
    Ok(())
}

pub(crate) fn ensure_private_dir(path: &Path) -> Result<()> {
    stdfs::create_dir_all(path).map_err(|source| McpdError::io(path, source))?;
    stdfs::set_permissions(path, stdfs::Permissions::from_mode(0o700))
        .map_err(|source| McpdError::io(path, source))
}

fn pending_path(paths: &Paths) -> PathBuf {
    paths.state_dir.join("pending-codex.toml")
}

fn save_pending(paths: &Paths, pending: &PendingTransaction) -> Result<()> {
    let text = toml::to_string_pretty(pending).map_err(|error| McpdError::Operational {
        message: format!("could not serialize pending transaction: {error}"),
        hint: "report this as an mcpd bug".into(),
    })?;
    fs::atomic_write(&pending_path(paths), text.as_bytes(), Some(0o600))
}

fn remove_pending(paths: &Paths) -> Result<()> {
    let path = pending_path(paths);
    match stdfs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(McpdError::io(path, source)),
    }
}

fn recover_pending(paths: &Paths) -> Result<()> {
    let path = pending_path(paths);
    if !path.exists() {
        return Ok(());
    }
    let text = stdfs::read_to_string(&path).map_err(|source| McpdError::io(&path, source))?;
    let pending: PendingTransaction =
        toml::from_str(&text).map_err(|error| McpdError::InvalidInput {
            message: format!(
                "pending transaction {} is malformed: {error}",
                path.display()
            ),
            hint: "inspect the target and state before removing the pending transaction manually"
                .into(),
        })?;
    if pending.version != 1 || pending.target != "codex" {
        return Err(McpdError::InvalidInput {
            message: "unsupported pending transaction record".into(),
            hint: "use a compatible mcpd version".into(),
        });
    }
    let current = match stdfs::read(&pending.target_path) {
        Ok(bytes) => Some(fs::hash_bytes(&bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(McpdError::io(&pending.target_path, source)),
    };
    if current.as_deref() == Some(&pending.desired_hash) {
        let state_path = paths.state_dir.join("state.toml");
        let mut state_file = state::load(&state_path)?;
        state_file
            .targets
            .insert("codex".into(), pending.next_state);
        state::save(&state_path, &state_file)?;
        remove_pending(paths)
    } else if current == pending.pre_hash {
        remove_pending(paths)
    } else {
        Err(McpdError::Conflict {
            message: format!("Codex config changed during recovery of {}", path.display()),
            hint: "inspect the target and backup; mcpd will not guess which version to own".into(),
        })
    }
}

pub(crate) fn now_ms() -> Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .map_err(|error| McpdError::Operational {
            message: format!("system clock is before the Unix epoch: {error}"),
            hint: "correct the system clock".into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    #[test]
    fn pending_target_write_is_committed_to_state_on_recovery() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let paths = Paths {
            config: temp.path().join("config.toml"),
            state_dir: temp.path().join("state"),
            codex_config: home.join(".codex/config.toml"),
            home,
        };
        ensure_private_dir(&paths.state_dir).unwrap();
        stdfs::create_dir_all(paths.codex_config.parent().unwrap()).unwrap();
        let desired = b"[mcp_servers.demo]\ncommand='demo'\n";
        stdfs::write(&paths.codex_config, desired).unwrap();
        let next_state = TargetState {
            config_path: paths.codex_config.clone(),
            adapter_version: 1,
            last_success_unix_ms: 42,
            managed: BTreeMap::new(),
        };
        save_pending(
            &paths,
            &PendingTransaction {
                version: 1,
                target: "codex".into(),
                target_path: paths.codex_config.clone(),
                pre_hash: None,
                desired_hash: fs::hash_bytes(desired),
                next_state: next_state.clone(),
            },
        )
        .unwrap();

        recover_pending(&paths).unwrap();
        assert_eq!(
            state::load(&paths.state_dir.join("state.toml"))
                .unwrap()
                .targets["codex"],
            next_state
        );
        assert!(!pending_path(&paths).exists());
    }
}
