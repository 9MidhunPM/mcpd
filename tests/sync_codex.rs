use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

use syncplane::{Paths, config, sync, targets::ChangeKind};
use tempfile::TempDir;

fn sync_one(
    config: &syncplane::model::CanonicalConfig,
    paths: &Paths,
    dry_run: bool,
) -> syncplane::diagnostics::Result<sync::SyncReport> {
    Ok(sync::sync_enabled_targets(config, paths, dry_run)?
        .into_iter()
        .next()
        .expect("test configuration enables Codex"))
}

fn sync_one_allowing_removals(
    config: &syncplane::model::CanonicalConfig,
    paths: &Paths,
) -> syncplane::diagnostics::Result<sync::SyncReport> {
    Ok(sync::sync_enabled_targets_with_policy(
        config,
        paths,
        false,
        &sync::RemovalPolicy::AllowAll,
    )?
    .into_iter()
    .next()
    .expect("test configuration enables Codex"))
}

fn plan_one(
    config: &syncplane::model::CanonicalConfig,
    paths: &Paths,
) -> syncplane::diagnostics::Result<syncplane::targets::TargetPlan> {
    Ok(sync::plan_enabled_targets(config, paths)?
        .into_iter()
        .next()
        .expect("test configuration enables Codex"))
}

fn paths(temp: &TempDir) -> Paths {
    let home = temp.path().join("home");
    Paths {
        config: temp.path().join("config.toml"),
        state_dir: temp.path().join("state"),
        codex_config: home.join(".codex/config.toml"),
        home,
    }
}

fn canonical(path: &std::path::Path, command: &str) -> syncplane::model::CanonicalConfig {
    config::parse(
        &format!(
            r#"
version = 1

[servers.demo]
transport = "stdio"
command = "{command}"
args = ["--safe"]

[targets.codex]
enabled = true
"#
        ),
        path,
    )
    .unwrap()
}

#[test]
fn sync_preserves_unmanaged_content_and_is_idempotent() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    fs::create_dir_all(paths.codex_config.parent().unwrap()).unwrap();
    fs::write(
        &paths.codex_config,
        r#"# user comment
model = "gpt-example"

[mcp_servers.manual]
command = "manual"
"#,
    )
    .unwrap();

    let config = canonical(&paths.config, "demo-v1");
    let first = sync_one(&config, &paths, false).unwrap();
    assert_eq!(first.changes.len(), 1);
    let target_after_first = fs::read(&paths.codex_config).unwrap();
    let state_after_first = fs::read(paths.state_dir.join("state.toml")).unwrap();
    assert!(String::from_utf8_lossy(&target_after_first).contains("# user comment"));
    assert!(String::from_utf8_lossy(&target_after_first).contains("mcp_servers.manual"));

    let second = sync_one(&config, &paths, false).unwrap();
    assert!(second.changes.is_empty());
    assert_eq!(target_after_first, fs::read(&paths.codex_config).unwrap());
    assert_eq!(
        state_after_first,
        fs::read(paths.state_dir.join("state.toml")).unwrap()
    );
}

#[test]
fn unmanaged_name_collision_is_a_conflict() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    fs::create_dir_all(paths.codex_config.parent().unwrap()).unwrap();
    fs::write(
        &paths.codex_config,
        "[mcp_servers.demo]\ncommand='manual'\n",
    )
    .unwrap();
    let error = sync_one(&canonical(&paths.config, "desired"), &paths, false).unwrap_err();
    assert!(error.to_string().contains("unmanaged MCP server `demo`"));
    assert!(!paths.state_dir.join("backups").exists());
}

#[test]
fn dry_run_has_no_filesystem_side_effects() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    let report = sync_one(&canonical(&paths.config, "demo"), &paths, true).unwrap();
    assert_eq!(report.changes.len(), 1);
    assert!(!paths.state_dir.exists());
    assert!(!paths.codex_config.exists());
}

#[test]
fn drift_is_repaired_and_backup_is_private() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    fs::create_dir_all(paths.codex_config.parent().unwrap()).unwrap();
    let config = canonical(&paths.config, "expected");
    sync_one(&config, &paths, false).unwrap();
    fs::write(
        &paths.codex_config,
        "[mcp_servers.demo]\ncommand='drifted-secret-like-value'\n",
    )
    .unwrap();

    let report = sync_one(&config, &paths, false).unwrap();
    assert_eq!(report.changes[0].kind, ChangeKind::DriftRepair);
    let backups = fs::read_dir(paths.state_dir.join("backups/codex"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(backups.len(), 1);
    let backup = backups[0].path();
    assert_eq!(
        fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        fs::read_to_string(backup)
            .unwrap()
            .contains("drifted-secret-like-value")
    );
}

#[test]
fn removing_canonical_server_removes_only_owned_entry() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    fs::create_dir_all(paths.codex_config.parent().unwrap()).unwrap();
    let config = canonical(&paths.config, "demo");
    sync_one(&config, &paths, false).unwrap();
    let empty = config::parse("version=1\n[targets.codex]\nenabled=true\n", &paths.config).unwrap();
    let error = sync_one(&empty, &paths, false).unwrap_err();
    assert!(error.to_string().contains("refusing to remove"));
    assert!(
        fs::read_to_string(&paths.codex_config)
            .unwrap()
            .contains("mcp_servers.demo")
    );
    let report = sync_one_allowing_removals(&empty, &paths).unwrap();
    assert_eq!(report.changes[0].kind, ChangeKind::Remove);
    assert!(
        !fs::read_to_string(&paths.codex_config)
            .unwrap()
            .contains("mcp_servers.demo")
    );
}

#[test]
fn malformed_and_symlinked_targets_are_never_modified() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    fs::create_dir_all(paths.codex_config.parent().unwrap()).unwrap();
    fs::write(&paths.codex_config, "not = [valid").unwrap();
    assert!(sync_one(&canonical(&paths.config, "demo"), &paths, false).is_err());
    assert_eq!(
        fs::read_to_string(&paths.codex_config).unwrap(),
        "not = [valid"
    );

    fs::remove_file(&paths.codex_config).unwrap();
    let outside = temp.path().join("outside.toml");
    fs::write(&outside, "keep = true\n").unwrap();
    symlink(&outside, &paths.codex_config).unwrap();
    let error = sync_one(&canonical(&paths.config, "demo"), &paths, false).unwrap_err();
    assert!(error.to_string().contains("symbolic links"));
    assert_eq!(fs::read_to_string(outside).unwrap(), "keep = true\n");
}

#[test]
fn secret_bearing_stdio_uses_runtime_wrapper_without_resolving_during_sync() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    let config = config::parse(
        r#"
version=1
[servers.secure]
transport="stdio"
command="tool"
[servers.secure.secrets]
TOKEN="service.token"
[targets.codex]
enabled=true
"#,
        &paths.config,
    )
    .unwrap();
    let dry_run = sync_one(&config, &paths, true).unwrap();
    assert_eq!(dry_run.changes.len(), 1);
    assert!(!paths.codex_config.exists());
    sync_one(&config, &paths, false).unwrap();
    let target = fs::read_to_string(&paths.codex_config).unwrap();
    assert!(target.contains("command = \"syncplane\""));
    assert!(target.contains("args = [\"exec\", \"secure\"]"));
    #[cfg(target_os = "linux")]
    assert!(target.contains("env_vars = [\"DBUS_SESSION_BUS_ADDRESS\"]"));
    assert!(!target.contains("service.token"));
    assert!(!target.contains("TOKEN"));
}

#[cfg(target_os = "linux")]
#[test]
fn secret_wrapper_merges_forwarded_environment_and_stays_idempotent() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    let config = config::parse(
        r#"
version=1
[servers.secure]
transport="stdio"
command="tool"
[servers.secure.env]
TOKEN={secret="service.token"}
PUBLIC_HOST="${env:PUBLIC_HOST}"
[targets.codex]
enabled=true
"#,
        &paths.config,
    )
    .unwrap();
    sync_one(&config, &paths, false).unwrap();
    fs::write(
        &paths.codex_config,
        r#"[mcp_servers.secure]
command="syncplane"
args=["exec", "secure"]
env_vars=["EXISTING", "DBUS_SESSION_BUS_ADDRESS", "EXISTING"]
"#,
    )
    .unwrap();

    let repaired = sync_one(&config, &paths, false).unwrap();
    assert_eq!(repaired.changes[0].kind, ChangeKind::DriftRepair);
    let target = fs::read_to_string(&paths.codex_config).unwrap();
    assert!(
        target.contains("env_vars = [\"DBUS_SESSION_BUS_ADDRESS\", \"EXISTING\", \"PUBLIC_HOST\"]")
    );
    assert!(!target.contains("service.token"));
    assert!(!target.contains("TOKEN"));
    let state_after_repair = fs::read(paths.state_dir.join("state.toml")).unwrap();

    let second = sync_one(&config, &paths, false).unwrap();
    assert!(second.changes.is_empty());
    assert_eq!(target, fs::read_to_string(&paths.codex_config).unwrap());
    assert_eq!(
        state_after_repair,
        fs::read(paths.state_dir.join("state.toml")).unwrap()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn non_secret_stdio_does_not_forward_keyring_runtime_environment() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    sync_one(&canonical(&paths.config, "tool"), &paths, false).unwrap();
    assert!(
        !fs::read_to_string(&paths.codex_config)
            .unwrap()
            .contains("DBUS_SESSION_BUS_ADDRESS")
    );
}

#[test]
fn environment_references_use_codex_indirection() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    let config = config::parse(
        r#"
version=1
[servers.local]
transport="stdio"
command="tool"
[servers.local.env]
TOKEN="${env:TOKEN}"
MODE="safe"
[servers.remote]
transport="http"
url="https://example.test/mcp"
[servers.remote.headers]
Authorization="${env:REMOTE_TOKEN}"
X_Mode="safe"
[targets.codex]
enabled=true
"#,
        &paths.config,
    )
    .unwrap();
    sync_one(&config, &paths, false).unwrap();
    let target = fs::read_to_string(&paths.codex_config).unwrap();
    assert!(target.contains("env_vars = [\"TOKEN\"]"));
    assert!(target.contains("TOKEN") && target.contains("MODE = \"safe\""));
    assert!(target.contains("env_http_headers"));
    assert!(target.contains("Authorization = \"REMOTE_TOKEN\""));
}

#[test]
fn concurrent_syncs_are_serialized() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    fs::create_dir_all(paths.codex_config.parent().unwrap()).unwrap();
    let config = canonical(&paths.config, "demo");
    let first_paths = paths.clone();
    let first_config = config.clone();
    let first = std::thread::spawn(move || sync_one(&first_config, &first_paths, false));
    let second_paths = paths.clone();
    let second_config = config.clone();
    let second = std::thread::spawn(move || sync_one(&second_config, &second_paths, false));
    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();
    let final_plan = plan_one(&config, &paths).unwrap();
    assert!(final_plan.is_noop());
}

proptest::proptest! {
    #[test]
    fn arbitrary_unmanaged_servers_survive_sync(suffix in "[a-z]{1,12}") {
        let temp = TempDir::new().unwrap();
        let paths = paths(&temp);
        fs::create_dir_all(paths.codex_config.parent().unwrap()).unwrap();
        let unmanaged = format!("manual_{suffix}");
        fs::write(&paths.codex_config, format!("[mcp_servers.{unmanaged}]\ncommand='manual'\n")).unwrap();
        let config = canonical(&paths.config, "demo");
        sync_one(&config, &paths, false).unwrap();
        let target = fs::read_to_string(&paths.codex_config).unwrap();
        let unmanaged_heading = format!("mcp_servers.{unmanaged}");
        proptest::prop_assert!(target.contains(&unmanaged_heading));
        proptest::prop_assert!(plan_one(&config, &paths).unwrap().is_noop());
    }
}
