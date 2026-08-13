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
    targets::{self, TargetPlan},
};

#[derive(Debug, Clone, Serialize)]
pub struct SyncReport {
    pub target: String,
    pub path: PathBuf,
    pub dry_run: bool,
    pub changes: Vec<crate::targets::Change>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncFailure {
    pub target: String,
    pub error: String,
    pub hint: Option<String>,
    pub exit_code: i32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncBatch {
    pub reports: Vec<SyncReport>,
    pub failures: Vec<SyncFailure>,
}

impl SyncBatch {
    pub fn into_result(self) -> Result<Vec<SyncReport>> {
        if self.failures.is_empty() {
            return Ok(self.reports);
        }
        let summary = self
            .failures
            .iter()
            .map(|failure| {
                format!(
                    "{}: {}",
                    targets::display_name(&failure.target),
                    failure.error
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let message = format!(
            "{} target synchronization(s) failed: {summary}",
            self.failures.len()
        );
        let mut hints = self
            .failures
            .iter()
            .filter_map(|failure| failure.hint.clone())
            .collect::<Vec<_>>();
        hints.sort();
        hints.dedup();
        hints.push(
            "successful targets were kept; fix the reported targets and rerun `mcpd sync`".into(),
        );
        let hint = hints.join("; ");
        match self.failures[0].exit_code {
            2 => Err(McpdError::InvalidInput { message, hint }),
            3 => Err(McpdError::TargetUnavailable {
                target: self.failures[0].target.clone(),
                message,
                hint,
            }),
            4 => Err(McpdError::Conflict { message, hint }),
            5 => Err(McpdError::Security {
                path: PathBuf::from("<multiple targets>"),
                message,
                hint,
            }),
            _ => Err(McpdError::Operational { message, hint }),
        }
    }
}

pub fn enabled_targets(config: &CanonicalConfig, _paths: &Paths) -> Result<Vec<String>> {
    let mut enabled = Vec::new();
    for (id, target) in &config.targets {
        if target.enabled {
            enabled.push(id.clone());
        }
    }
    enabled.sort_by_key(|id| {
        targets::TARGET_IDS
            .iter()
            .position(|candidate| candidate == id)
            .unwrap_or(usize::MAX)
    });
    Ok(enabled)
}

fn selected_targets<'a>(
    config: &CanonicalConfig,
    _paths: &Paths,
    selected: &'a [String],
) -> Result<Vec<&'a str>> {
    let mut result = Vec::new();
    for target in selected {
        if !config
            .targets
            .get(target)
            .is_some_and(|target| target.enabled)
        {
            return Err(McpdError::InvalidInput {
                message: format!("target `{target}` is not enabled"),
                hint: format!("run `mcpd targets enable {target}` first"),
            });
        }
        if !result.contains(&target.as_str()) {
            result.push(target.as_str());
        }
    }
    Ok(result)
}

pub fn plan_enabled_targets(config: &CanonicalConfig, paths: &Paths) -> Result<Vec<TargetPlan>> {
    enabled_targets(config, paths)?
        .into_iter()
        .map(|target| plan_target(&target, config, paths))
        .collect()
}

pub fn plan_selected_targets(
    config: &CanonicalConfig,
    paths: &Paths,
    selected: &[String],
) -> Result<Vec<TargetPlan>> {
    if selected.is_empty() {
        return plan_enabled_targets(config, paths);
    }
    selected_targets(config, paths, selected)?
        .into_iter()
        .map(|target| plan_target(target, config, paths))
        .collect()
}

pub fn sync_enabled_targets(
    config: &CanonicalConfig,
    paths: &Paths,
    dry_run: bool,
) -> Result<Vec<SyncReport>> {
    sync_targets_isolated(config, paths, &enabled_targets(config, paths)?, dry_run).into_result()
}

pub fn sync_selected_targets(
    config: &CanonicalConfig,
    paths: &Paths,
    selected: &[String],
    dry_run: bool,
) -> Result<Vec<SyncReport>> {
    if selected.is_empty() {
        return sync_enabled_targets(config, paths, dry_run);
    }
    let selected = selected_targets(config, paths, selected)?
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    sync_targets_isolated(config, paths, &selected, dry_run).into_result()
}

pub fn sync_selected_targets_isolated(
    config: &CanonicalConfig,
    paths: &Paths,
    selected: &[String],
    dry_run: bool,
) -> Result<SyncBatch> {
    let targets = if selected.is_empty() {
        enabled_targets(config, paths)?
    } else {
        selected_targets(config, paths, selected)?
            .into_iter()
            .map(str::to_owned)
            .collect()
    };
    Ok(sync_targets_isolated(config, paths, &targets, dry_run))
}

fn sync_targets_isolated(
    config: &CanonicalConfig,
    paths: &Paths,
    targets: &[String],
    dry_run: bool,
) -> SyncBatch {
    let mut batch = SyncBatch::default();
    for target in targets {
        match sync_target(target, config, paths, dry_run) {
            Ok(report) => batch.reports.push(report),
            Err(error) => {
                let exit_code = crate::diagnostics::ExitCode::from(&error) as i32;
                batch.failures.push(SyncFailure {
                    target: target.clone(),
                    error: error.to_string(),
                    hint: error.hint().map(str::to_owned),
                    exit_code,
                });
            }
        }
    }
    batch
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

fn plan_target(target: &str, config: &CanonicalConfig, paths: &Paths) -> Result<TargetPlan> {
    let state_path = paths.state_dir.join("state.toml");
    let state = state::load(&state_path)?;
    let adapter = targets::adapter(target, paths)?;
    adapter.plan(config, state.targets.get(adapter.id()))
}

fn sync_target(
    target: &str,
    config: &CanonicalConfig,
    paths: &Paths,
    dry_run: bool,
) -> Result<SyncReport> {
    if dry_run {
        let plan = plan_target(target, config, paths)?;
        return Ok(report(&plan, true));
    }
    with_sync_lock(paths, || {
        let state_path = paths.state_dir.join("state.toml");
        let mut state_file = state::load(&state_path)?;
        let adapter = targets::adapter(target, paths)?;
        let mut plan = adapter.plan(config, state_file.targets.get(adapter.id()))?;
        if plan.is_noop() {
            return Ok(report(&plan, false));
        }
        if plan.changes.is_empty() {
            plan.next_state.last_success_unix_ms = now_ms()?;
            state_file
                .targets
                .insert(plan.target.clone(), plan.next_state.clone());
            state::save(&state_path, &state_file)?;
            return Ok(report(&plan, false));
        }

        verify_snapshot(&plan)?;
        backup(&plan, &paths.state_dir)?;
        plan.next_state.last_success_unix_ms = now_ms()?;
        let pending = PendingTransaction {
            version: 1,
            target: plan.target.clone(),
            target_path: plan.path.clone(),
            pre_hash: plan.before.as_deref().map(fs::hash_bytes),
            desired_hash: fs::hash_bytes(&plan.rendered),
            next_state: plan.next_state.clone(),
        };
        save_pending(paths, &pending)?;
        fs::atomic_write(&plan.path, &plan.rendered, None)?;
        state_file
            .targets
            .insert(plan.target.clone(), plan.next_state.clone());
        state::save(&state_path, &state_file)?;
        remove_pending(paths)?;
        Ok(report(&plan, false))
    })
}

pub(crate) fn with_sync_lock<T>(paths: &Paths, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    ensure_private_dir(&paths.state_dir)?;
    let lock_path = paths.state_dir.join("sync.lock");
    reject_symlink_lock(&lock_path)?;
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

pub fn lock_status(paths: &Paths) -> Result<&'static str> {
    let lock_path = paths.state_dir.join("sync.lock");
    match stdfs::symlink_metadata(&lock_path) {
        Ok(_) => reject_symlink_lock(&lock_path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok("absent"),
        Err(source) => return Err(McpdError::io(&lock_path, source)),
    }
    let lock = stdfs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|source| McpdError::io(&lock_path, source))?;
    match lock.try_lock_exclusive() {
        Ok(()) => {
            fs2::FileExt::unlock(&lock).map_err(|source| McpdError::io(&lock_path, source))?;
            Ok("available")
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok("active"),
        Err(source) => Err(McpdError::io(&lock_path, source)),
    }
}

fn reject_symlink_lock(lock_path: &Path) -> Result<()> {
    if stdfs::symlink_metadata(lock_path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(McpdError::Security {
            path: lock_path.to_path_buf(),
            message: "sync lock is a symbolic link".into(),
            hint: "remove the unexpected symlink before running mutating commands".into(),
        });
    }
    Ok(())
}

fn report(plan: &TargetPlan, dry_run: bool) -> SyncReport {
    SyncReport {
        target: plan.target.clone(),
        path: plan.path.clone(),
        dry_run,
        changes: plan.changes.clone(),
    }
}

fn backup(plan: &TargetPlan, state_dir: &Path) -> Result<()> {
    let Some(before) = &plan.before else {
        return Ok(());
    };
    let dir = state_dir.join("backups").join(&plan.target);
    ensure_private_dir(&dir)?;
    let stamp = now_ms()?;
    let mut selected = None;
    for suffix in 0..100_u8 {
        let path = dir.join(format!("{stamp}-{suffix:02}-config.backup"));
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
    if stdfs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(McpdError::Security {
            path: path.to_path_buf(),
            message: "private state directory is a symbolic link".into(),
            hint: "replace it with a regular private directory before allowing mcpd to write state"
                .into(),
        });
    }
    stdfs::create_dir_all(path).map_err(|source| McpdError::io(path, source))?;
    stdfs::set_permissions(path, stdfs::Permissions::from_mode(0o700))
        .map_err(|source| McpdError::io(path, source))
}

fn pending_path(paths: &Paths) -> PathBuf {
    paths.state_dir.join("pending-sync.toml")
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
    if pending.version != 1 || targets::adapter(&pending.target, paths).is_err() {
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
            .insert(pending.target.clone(), pending.next_state);
        state::save(&state_path, &state_file)?;
        remove_pending(paths)
    } else if current == pending.pre_hash {
        remove_pending(paths)
    } else {
        Err(McpdError::Conflict {
            message: format!(
                "{} config changed during recovery of {}",
                targets::display_name(&pending.target),
                path.display()
            ),
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

    #[test]
    fn advisory_lock_status_distinguishes_active_and_released_locks() {
        let temp = TempDir::new().unwrap();
        let paths = Paths {
            config: temp.path().join("config.toml"),
            state_dir: temp.path().join("state"),
            codex_config: temp.path().join("home/.codex/config.toml"),
            home: temp.path().join("home"),
        };
        ensure_private_dir(&paths.state_dir).unwrap();
        let lock_path = paths.state_dir.join("sync.lock");
        let lock = stdfs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        lock.lock_exclusive().unwrap();
        assert_eq!(lock_status(&paths).unwrap(), "active");
        drop(lock);
        assert_eq!(lock_status(&paths).unwrap(), "available");
    }
}
