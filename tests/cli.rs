use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    time::SystemTime,
};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

struct TestEnvironment {
    _temp: TempDir,
    home: PathBuf,
    config: PathBuf,
    state: PathBuf,
    codex: PathBuf,
}

impl TestEnvironment {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        Self {
            config: temp.path().join("xdg-config/mcpd/config.toml"),
            state: temp.path().join("xdg-state/mcpd"),
            codex: home.join(".codex/config.toml"),
            home,
            _temp: temp,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::cargo_bin("mcpd").unwrap();
        command
            .env("HOME", &self.home)
            .env(
                "XDG_CONFIG_HOME",
                self.config.parent().unwrap().parent().unwrap(),
            )
            .env("XDG_STATE_HOME", self.state.parent().unwrap())
            .env("MCPD_HOME", &self.home)
            .env("MCPD_CONFIG", &self.config)
            .env("MCPD_STATE_DIR", &self.state)
            .env("MCPD_CODEX_CONFIG", &self.codex);
        command.env("MCPD_SECRET_BACKEND", "mock-file");
        command
    }

    fn root(&self) -> &Path {
        self._temp.path()
    }

    fn init(&self) {
        self.command().arg("init").assert().success();
    }

    fn add_context7_no_sync(&self) {
        self.command()
            .args([
                "add",
                "context7",
                "--no-sync",
                "--",
                "npx",
                "-y",
                "@upstash/context7-mcp",
            ])
            .assert()
            .success();
    }

    fn enable_codex(&self) {
        self.command()
            .args(["targets", "enable", "codex"])
            .assert()
            .success();
    }

    fn write_codex(&self, content: &str) {
        fs::create_dir_all(self.codex.parent().unwrap()).unwrap();
        fs::write(&self.codex, content).unwrap();
    }

    fn parse_config(&self) -> toml::Value {
        toml::from_str(&fs::read_to_string(&self.config).unwrap()).unwrap()
    }

    fn parse_codex(&self) -> toml::Value {
        toml::from_str(&fs::read_to_string(&self.codex).unwrap()).unwrap()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct SnapshotEntry {
    is_directory: bool,
    contents: Vec<u8>,
    mode: u32,
    modified: SystemTime,
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, SnapshotEntry> {
    fn visit(root: &Path, current: &Path, snapshot: &mut BTreeMap<PathBuf, SnapshotEntry>) {
        let mut entries = fs::read_dir(current)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let is_directory = metadata.is_dir();
            snapshot.insert(
                path.strip_prefix(root).unwrap().to_path_buf(),
                SnapshotEntry {
                    is_directory,
                    contents: if metadata.is_file() {
                        fs::read(&path).unwrap()
                    } else {
                        Vec::new()
                    },
                    mode: metadata.permissions().mode(),
                    modified: metadata.modified().unwrap(),
                },
            );
            if is_directory {
                visit(root, &path, snapshot);
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot);
    snapshot
}

fn project_command(environment: &TestEnvironment, project: &Path) -> Command {
    let mut command = environment.command();
    command.env("MCPD_PROJECT_ROOT", project);
    command
}

const UNMANAGED_CODEX: &str = r#"# keep this comment
model = "gpt-example"
approval_policy = "on-request"

[mcp_servers.github]
command = "github-mcp"

[mcp_servers.playwright]
command = "playwright-mcp"
args = ["--isolated"]

[mcp_servers.dokploy]
url = "https://dokploy.example/mcp"
"#;

#[test]
fn init_creates_only_canonical_config() {
    let environment = TestEnvironment::new();
    environment.write_codex(UNMANAGED_CODEX);
    let original_codex = fs::read(&environment.codex).unwrap();

    environment
        .command()
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("No clients were modified"));

    assert!(environment.config.is_file());
    assert_eq!(fs::read(&environment.codex).unwrap(), original_codex);
    assert!(!environment.state.exists());
    assert!(!environment.state.join("state.toml").exists());
    assert!(!environment.state.join("sync.lock").exists());
    assert!(!environment.state.join("backups").exists());
}

#[test]
fn deleted_canonical_config_cannot_be_reinitialized_or_silently_remove_owned_servers() {
    let environment = TestEnvironment::new();
    let old_config = r#"version = 1

[servers.bing-search]
transport = "stdio"
command = "bing-search-mcp"

[servers.dokploy]
transport = "http"
url = "https://dokploy.example/mcp"

[servers.github]
transport = "stdio"
command = "github-mcp"

[targets.codex]
enabled = true
"#;
    fs::create_dir_all(environment.config.parent().unwrap()).unwrap();
    fs::write(&environment.config, old_config).unwrap();
    environment.write_codex("model = 'gpt-example'\n");
    environment.command().arg("sync").assert().success();

    let canonical_before = fs::read(&environment.config).unwrap();
    let state_before = fs::read(environment.state.join("state.toml")).unwrap();
    let target_before = fs::read(&environment.codex).unwrap();
    for name in ["bing-search", "dokploy", "github"] {
        assert!(String::from_utf8_lossy(&state_before).contains(name));
        assert!(String::from_utf8_lossy(&target_before).contains(name));
    }

    fs::remove_file(&environment.config).unwrap();
    environment
        .command()
        .arg("init")
        .assert()
        .code(4)
        .stderr(predicate::str::contains(
            "refusing to initialize an empty canonical config",
        ))
        .stderr(predicate::str::contains("bing-search"))
        .stderr(predicate::str::contains("dokploy"))
        .stderr(predicate::str::contains("github"));
    assert!(!environment.config.exists());
    assert_eq!(
        fs::read(environment.state.join("state.toml")).unwrap(),
        state_before
    );
    assert_eq!(fs::read(&environment.codex).unwrap(), target_before);

    fs::write(&environment.config, &canonical_before).unwrap();
    environment
        .command()
        .arg("diff")
        .assert()
        .success()
        .stdout(predicate::str::contains("= synchronized"))
        .stdout(predicate::str::contains("REMOVE").not())
        .stderr(predicate::str::contains("WARNING").not());
    assert_eq!(fs::read(&environment.config).unwrap(), canonical_before);
    assert_eq!(
        fs::read(environment.state.join("state.toml")).unwrap(),
        state_before
    );

    fs::write(
        &environment.config,
        "version = 1\n\n[servers]\n\n[targets]\n\n[targets.codex]\nenabled = true\n",
    )
    .unwrap();
    environment
        .command()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Canonical  0 server(s)"))
        .stderr(predicate::str::contains("canonical config has no servers"))
        .stderr(predicate::str::contains("3 managed target entry/entries"))
        .stderr(predicate::str::contains("REMOVE Codex: bing-search"));
    environment
        .command()
        .args(["--json", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"canonical_empty\": true"))
        .stdout(predicate::str::contains("\"managed_entries_at_risk\": 3"))
        .stdout(predicate::str::contains("\"server\": \"github\""));
    environment
        .command()
        .arg("diff")
        .assert()
        .success()
        .stdout(predicate::str::contains("Remove\tbing-search"))
        .stdout(predicate::str::contains("Remove\tdokploy"))
        .stdout(predicate::str::contains("Remove\tgithub"))
        .stderr(predicate::str::contains("planned managed removals"));

    let before_dry_run = snapshot_tree(environment.root());
    environment
        .command()
        .args(["sync", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("REMOVE bing-search"))
        .stdout(predicate::str::contains("REMOVE dokploy"))
        .stdout(predicate::str::contains("REMOVE github"))
        .stderr(predicate::str::contains("removals are blocked by default"));
    assert_eq!(snapshot_tree(environment.root()), before_dry_run);

    environment
        .command()
        .arg("sync")
        .assert()
        .code(4)
        .stderr(predicate::str::contains(
            "refusing to remove 3 managed MCP server(s) from Codex",
        ));
    assert_eq!(fs::read(&environment.codex).unwrap(), target_before);
    assert_eq!(
        fs::read(environment.state.join("state.toml")).unwrap(),
        state_before
    );
}

#[test]
fn target_filters_are_repeatable_and_do_not_touch_unselected_targets() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.add_context7_no_sync();
    environment.enable_codex();
    environment
        .command()
        .args(["targets", "enable", "cursor"])
        .assert()
        .success();
    let cursor = environment.home.join(".cursor/mcp.json");
    fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    fs::write(&cursor, r#"{"mcpServers":{}}"#).unwrap();
    let cursor_before = fs::read(&cursor).unwrap();

    environment
        .command()
        .args(["diff", "--target", "codex"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Codex"))
        .stdout(predicate::str::contains("Cursor").not());
    environment
        .command()
        .args(["sync", "--target", "codex"])
        .assert()
        .success();
    assert!(environment.codex.is_file());
    assert_eq!(fs::read(&cursor).unwrap(), cursor_before);

    environment
        .command()
        .args([
            "status", "--target", "codex", "--target", "cursor", "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""target": "codex""#))
        .stdout(predicate::str::contains(r#""target": "cursor""#))
        .stdout(predicate::str::contains(r#""target": "claude""#).not());
    environment
        .command()
        .args(["doctor", "--target", "codex"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Codex"))
        .stdout(predicate::str::contains("Cursor").not());
}

#[test]
fn project_overlays_are_ignored_until_trusted_and_can_be_revoked() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.add_context7_no_sync();
    let project = environment.root().join("project");
    fs::create_dir_all(project.join(".git")).unwrap();
    fs::create_dir_all(project.join(".mcpd")).unwrap();
    fs::write(
        project.join(".mcpd/config.toml"),
        r#"
version = 1

[servers.context7]
enabled = false

[servers.project-only]
transport = "stdio"
command = "project-tool"

[servers.project-only.env]
TOKEN = { secret = "project.token" }
"#,
    )
    .unwrap();

    project_command(&environment, &project)
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("context7"))
        .stdout(predicate::str::contains("project-only").not())
        .stderr(predicate::str::contains(
            "ignoring untrusted project overlay",
        ));

    project_command(&environment, &project)
        .arg("trust")
        .assert()
        .success()
        .stdout(predicate::str::contains("trusted project"));
    project_command(&environment, &project)
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("project-only"))
        .stdout(predicate::str::contains("context7").not());
    project_command(&environment, &project)
        .args(["doctor", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("project.token"))
        .stdout(predicate::str::contains(r#""trusted": true"#));

    project_command(&environment, &project)
        .args(["trust", "--revoke"])
        .arg(&project)
        .assert()
        .success();
    project_command(&environment, &project)
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("context7"))
        .stdout(predicate::str::contains("project-only").not());
}

#[test]
fn project_overlay_symlinks_are_rejected_without_following_them() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.add_context7_no_sync();
    let project = environment.root().join("project-symlink");
    fs::create_dir_all(project.join(".git")).unwrap();
    fs::create_dir_all(project.join(".mcpd")).unwrap();
    let outside = environment.root().join("outside-overlay.toml");
    fs::write(
        &outside,
        "version = 1\n[servers.outside]\ntransport = 'stdio'\ncommand = 'never-follow-this'\n",
    )
    .unwrap();
    project_command(&environment, &project)
        .arg("trust")
        .assert()
        .success();
    symlink(&outside, project.join(".mcpd/config.toml")).unwrap();
    let canonical_before = fs::read(&environment.config).unwrap();
    let outside_before = fs::read(&outside).unwrap();

    project_command(&environment, &project)
        .arg("list")
        .assert()
        .code(5)
        .stderr(predicate::str::contains("symbolic links are not followed"))
        .stderr(predicate::str::contains("never-follow-this").not());

    assert_eq!(fs::read(&environment.config).unwrap(), canonical_before);
    assert_eq!(fs::read(outside).unwrap(), outside_before);
}

#[test]
fn claude_project_and_local_scopes_require_trust_and_write_their_native_regions() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.add_context7_no_sync();
    let project = environment.root().join("scoped-project");
    fs::create_dir_all(project.join(".git")).unwrap();

    project_command(&environment, &project)
        .args(["targets", "enable", "claude-project"])
        .assert()
        .failure()
        .code(5);
    project_command(&environment, &project)
        .arg("trust")
        .assert()
        .success();
    project_command(&environment, &project)
        .arg("targets")
        .assert()
        .success()
        .stdout(predicate::str::contains("Claude Code (project)"))
        .stdout(predicate::str::contains("Claude Code (local)"));
    project_command(&environment, &project)
        .args(["targets", "enable", "claude-project"])
        .assert()
        .success();
    project_command(&environment, &project)
        .args(["sync", "--target", "claude-project"])
        .assert()
        .success();
    let project_config: serde_json::Value =
        serde_json::from_slice(&fs::read(project.join(".mcp.json")).unwrap()).unwrap();
    assert!(project_config["mcpServers"]["context7"].is_object());

    project_command(&environment, &project)
        .args(["targets", "enable", "claude-local"])
        .assert()
        .success();
    project_command(&environment, &project)
        .args(["sync", "--target", "claude-local"])
        .assert()
        .success();
    let local: serde_json::Value =
        serde_json::from_slice(&fs::read(environment.home.join(".claude.json")).unwrap()).unwrap();
    let project_key = fs::canonicalize(&project)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(local["projects"][project_key]["mcpServers"]["context7"].is_object());
}

#[test]
fn a_failed_target_does_not_prevent_later_targets_from_syncing() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.add_context7_no_sync();
    environment.enable_codex();
    environment
        .command()
        .args(["targets", "enable", "claude"])
        .assert()
        .success();
    fs::create_dir_all(&environment.home).unwrap();
    fs::write(environment.home.join(".claude.json"), b"{ malformed").unwrap();

    environment
        .command()
        .arg("sync")
        .assert()
        .failure()
        .stdout(predicate::str::contains("Codex"))
        .stderr(predicate::str::contains("Claude Code"));
    assert!(
        fs::read_to_string(&environment.codex)
            .unwrap()
            .contains("mcp_servers.context7")
    );
    assert_eq!(
        fs::read(environment.home.join(".claude.json")).unwrap(),
        b"{ malformed"
    );
}

#[test]
fn an_unavailable_enabled_adapter_does_not_prevent_other_targets_from_syncing() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.add_context7_no_sync();
    environment.enable_codex();
    fs::write(
        &environment.config,
        format!(
            "{}\n[targets.aaa-missing]\nenabled = true\n",
            fs::read_to_string(&environment.config).unwrap()
        ),
    )
    .unwrap();

    environment
        .command()
        .arg("sync")
        .assert()
        .code(3)
        .stdout(predicate::str::contains("Codex"))
        .stderr(predicate::str::contains("aaa-missing"))
        .stderr(predicate::str::contains("successful targets were kept"));

    assert_eq!(
        environment.parse_codex()["mcp_servers"]["context7"]["command"].as_str(),
        Some("npx")
    );
    environment
        .command()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("aaa-missing"))
        .stdout(predicate::str::contains("invalid"));
    environment
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("invalid aaa-missing"));
}

#[test]
fn targets_completions_get_and_version_cover_the_v1_command_surface() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.add_context7_no_sync();

    environment
        .command()
        .arg("targets")
        .assert()
        .success()
        .stdout(predicate::str::contains("Claude Code"))
        .stdout(predicate::str::contains("Cursor"))
        .stdout(predicate::str::contains("Codex"))
        .stdout(predicate::str::contains("Antigravity"))
        .stdout(predicate::str::contains("OpenChamber"));
    environment
        .command()
        .args(["get", "context7"])
        .assert()
        .success()
        .stdout(predicate::str::contains("command = \"npx\""));
    environment
        .command()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_mcpd"));
    environment
        .command()
        .args(["systemd", "generate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ExecStart=\""))
        .stdout(predicate::str::contains(" watch"));
    assert!(!environment.home.join(".config/systemd").exists());
    environment
        .command()
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains("mcpd 1.0.0"));
    for command in ["status", "diff", "sync", "doctor"] {
        environment
            .command()
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--target <TARGET>"));
    }
}

#[test]
fn add_no_sync_changes_only_canonical_configuration() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(UNMANAGED_CODEX);
    let original_codex = fs::read(&environment.codex).unwrap();
    fs::create_dir_all(&environment.state).unwrap();
    let state_marker = environment.state.join("state.toml");
    fs::write(&state_marker, b"unchanged").unwrap();

    environment
        .command()
        .args([
            "add",
            "context7",
            "--no-sync",
            "--",
            "npx",
            "-y",
            "@upstash/context7-mcp",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("synchronization skipped"))
        .stdout(predicate::str::contains("already synchronized").not());

    let canonical = environment.parse_config();
    assert_eq!(
        canonical["servers"]["context7"]["command"].as_str(),
        Some("npx")
    );
    assert_eq!(fs::read(&environment.codex).unwrap(), original_codex);
    assert_eq!(fs::read(&state_marker).unwrap(), b"unchanged");
    assert!(!environment.state.join("sync.lock").exists());
}

#[test]
fn disabled_detected_target_is_ignored_by_diff_and_sync() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(UNMANAGED_CODEX);
    environment.add_context7_no_sync();
    let original_codex = fs::read(&environment.codex).unwrap();

    environment
        .command()
        .arg("diff")
        .assert()
        .success()
        .stdout(predicate::str::contains("No enabled targets."))
        .stdout(predicate::str::contains("context7").not());
    environment
        .command()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("No enabled targets."));

    assert_eq!(fs::read(&environment.codex).unwrap(), original_codex);
    assert!(!environment.state.exists());
    assert!(!environment.state.join("backups").exists());
}

#[test]
fn enabled_target_has_pending_diff() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(UNMANAGED_CODEX);
    environment.add_context7_no_sync();
    environment.enable_codex();

    environment
        .command()
        .arg("diff")
        .assert()
        .success()
        .stdout(predicate::str::contains("Codex"))
        .stdout(predicate::str::contains("Add"))
        .stdout(predicate::str::contains("context7"));
}

#[test]
fn dry_run_reports_change_without_any_reconciliation_writes() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(UNMANAGED_CODEX);
    environment.add_context7_no_sync();
    environment.enable_codex();
    let original_codex = fs::read(&environment.codex).unwrap();

    environment
        .command()
        .args(["sync", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dry-run Codex: 1 change"));

    assert_eq!(fs::read(&environment.codex).unwrap(), original_codex);
    assert!(!environment.state.exists());
    assert!(!environment.state.join("state.toml").exists());
    assert!(!environment.state.join("sync.lock").exists());
    assert!(!environment.state.join("backups").exists());
}

#[test]
fn sync_preserves_unmanaged_servers_and_unrelated_settings() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(UNMANAGED_CODEX);
    environment.add_context7_no_sync();
    environment.enable_codex();
    let unmanaged_before: toml::Value = toml::from_str(UNMANAGED_CODEX).unwrap();

    environment.command().arg("sync").assert().success();

    let codex = environment.parse_codex();
    assert_eq!(codex["model"].as_str(), Some("gpt-example"));
    assert_eq!(codex["approval_policy"].as_str(), Some("on-request"));
    for unmanaged in ["github", "playwright", "dokploy"] {
        assert_eq!(
            codex["mcp_servers"].get(unmanaged),
            unmanaged_before["mcp_servers"].get(unmanaged),
            "unmanaged server `{unmanaged}` was modified"
        );
    }
    assert_eq!(
        codex["mcp_servers"]["context7"]["command"].as_str(),
        Some("npx")
    );
}

#[test]
fn second_sync_is_a_byte_for_byte_noop() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(UNMANAGED_CODEX);
    environment.add_context7_no_sync();
    environment.enable_codex();
    environment.command().arg("sync").assert().success();
    let target_after_first = fs::read(&environment.codex).unwrap();
    let state_after_first = fs::read(environment.state.join("state.toml")).unwrap();
    let state_tree_after_first = snapshot_tree(&environment.state);

    environment
        .command()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("already synchronized"));

    assert_eq!(fs::read(&environment.codex).unwrap(), target_after_first);
    assert_eq!(
        fs::read(environment.state.join("state.toml")).unwrap(),
        state_after_first
    );
    assert_eq!(snapshot_tree(&environment.state), state_tree_after_first);
}

#[test]
fn managed_server_removal_lifecycle_is_safe_and_dry_run_is_absolutely_pure() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(UNMANAGED_CODEX);
    environment.add_context7_no_sync();
    environment.enable_codex();
    environment.command().arg("sync").assert().success();

    environment
        .command()
        .arg("diff")
        .assert()
        .success()
        .stdout(predicate::str::contains("Codex"))
        .stdout(predicate::str::contains("= synchronized"))
        .stdout(predicate::str::contains("Add").not())
        .stdout(predicate::str::contains("Remove").not());

    let target_before_removal = fs::read(&environment.codex).unwrap();
    let state_before_removal = snapshot_tree(&environment.state);
    environment
        .command()
        .args(["remove", "context7", "--no-sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("synchronization skipped"));

    assert!(
        environment.parse_config()["servers"]
            .get("context7")
            .is_none()
    );
    assert_eq!(fs::read(&environment.codex).unwrap(), target_before_removal);
    assert_eq!(snapshot_tree(&environment.state), state_before_removal);

    environment
        .command()
        .arg("diff")
        .assert()
        .success()
        .stdout(predicate::str::contains("Remove"))
        .stdout(predicate::str::contains("context7"));

    let before_dry_run = snapshot_tree(environment.root());
    environment
        .command()
        .args(["sync", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dry-run Codex: 1 change"));
    assert_eq!(
        snapshot_tree(environment.root()),
        before_dry_run,
        "dry-run changed the isolated filesystem tree"
    );

    environment
        .command()
        .args(["sync", "--allow-removals"])
        .assert()
        .success();
    let codex = environment.parse_codex();
    let unmanaged_before: toml::Value = toml::from_str(UNMANAGED_CODEX).unwrap();
    assert!(codex["mcp_servers"].get("context7").is_none());
    assert_eq!(codex["model"].as_str(), Some("gpt-example"));
    assert_eq!(codex["approval_policy"].as_str(), Some("on-request"));
    for unmanaged in ["github", "playwright", "dokploy"] {
        assert_eq!(
            codex["mcp_servers"].get(unmanaged),
            unmanaged_before["mcp_servers"].get(unmanaged),
            "unmanaged server `{unmanaged}` was modified during managed removal"
        );
    }

    environment
        .command()
        .arg("diff")
        .assert()
        .success()
        .stdout(predicate::str::contains("= synchronized"));
}

#[test]
fn extended_diff_separates_managed_unmanaged_and_canonical_only_servers() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(UNMANAGED_CODEX);
    environment.add_context7_no_sync();
    environment.enable_codex();
    environment.command().arg("sync").assert().success();
    environment
        .command()
        .args(["add", "postgres", "--no-sync", "--", "postgres-mcp"])
        .assert()
        .success();

    environment
        .command()
        .arg("diff")
        .assert()
        .success()
        .stdout(predicate::str::contains("Add\tpostgres"))
        .stdout(predicate::str::contains("github").not())
        .stdout(predicate::str::contains("playwright").not())
        .stdout(predicate::str::contains("dokploy").not());

    let before_diff = snapshot_tree(environment.root());
    environment
        .command()
        .args(["diff", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Managed (synchronized)"))
        .stdout(predicate::str::contains("= context7"))
        .stdout(predicate::str::contains("Managed drift\n  (none)"))
        .stdout(predicate::str::contains("Only in Codex (unmanaged)"))
        .stdout(predicate::str::contains("+ github"))
        .stdout(predicate::str::contains("+ playwright"))
        .stdout(predicate::str::contains("+ dokploy"))
        .stdout(predicate::str::contains("Only in mcpd"))
        .stdout(predicate::str::contains("- postgres"));
    assert_eq!(snapshot_tree(environment.root()), before_diff);
}

#[test]
fn extended_diff_reports_managed_drift_without_mutating_target() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(UNMANAGED_CODEX);
    environment.add_context7_no_sync();
    environment.enable_codex();
    environment.command().arg("sync").assert().success();

    let target = fs::read_to_string(&environment.codex)
        .unwrap()
        .replace("command = \"npx\"", "command = \"manually-edited\"");
    fs::write(&environment.codex, target).unwrap();
    let before_diff = snapshot_tree(environment.root());

    environment
        .command()
        .args(["diff", "--include-unmanaged"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Managed drift"))
        .stdout(predicate::str::contains("~ context7"))
        .stdout(predicate::str::contains("Only in Codex (unmanaged)"))
        .stdout(predicate::str::contains("+ github"));
    assert_eq!(snapshot_tree(environment.root()), before_diff);
}

#[test]
fn removing_managed_server_preserves_unmanaged_servers() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(UNMANAGED_CODEX);
    environment.add_context7_no_sync();
    environment.enable_codex();
    environment.command().arg("sync").assert().success();

    environment
        .command()
        .args(["remove", "context7"])
        .assert()
        .success();

    let codex = environment.parse_codex();
    assert!(codex["mcp_servers"].get("context7").is_none());
    for unmanaged in ["github", "playwright", "dokploy"] {
        assert!(codex["mcp_servers"].get(unmanaged).is_some());
    }
}

#[test]
fn malformed_target_fails_without_overwrite_or_ownership_update() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex("not = [valid");
    environment.add_context7_no_sync();
    environment.enable_codex();
    let original = fs::read(&environment.codex).unwrap();

    environment
        .command()
        .arg("sync")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("malformed TOML"))
        .stderr(predicate::str::contains("never overwrites malformed"));

    assert_eq!(fs::read(&environment.codex).unwrap(), original);
    assert!(!environment.state.join("state.toml").exists());
    assert!(!environment.state.join("backups").exists());
}

#[test]
fn unmanaged_name_collision_is_not_adopted_or_overwritten() {
    let environment = TestEnvironment::new();
    environment.init();
    environment
        .write_codex("[mcp_servers.context7]\ncommand = 'manual-context7'\nargs = ['keep-me']\n");
    environment.add_context7_no_sync();
    environment.enable_codex();
    let original = fs::read(&environment.codex).unwrap();

    environment
        .command()
        .arg("sync")
        .assert()
        .code(4)
        .stderr(predicate::str::contains("unmanaged MCP server `context7`"));

    assert_eq!(fs::read(&environment.codex).unwrap(), original);
    assert!(!environment.state.join("state.toml").exists());
    assert!(!environment.state.join("backups").exists());
}

#[test]
fn ambiguous_add_fails_without_writes_and_delimited_add_succeeds() {
    let environment = TestEnvironment::new();
    environment.init();
    let original_config = fs::read(&environment.config).unwrap();

    environment
        .command()
        .args([
            "add",
            "context",
            "7",
            "--no-sync",
            "--",
            "npx",
            "-y",
            "@upstash/context7-mcp",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unexpected positional value `7`"));

    assert_eq!(fs::read(&environment.config).unwrap(), original_config);
    assert!(!environment.state.exists());

    environment
        .command()
        .args([
            "add",
            "context",
            "7",
            "--",
            "npx",
            "-y",
            "@upstash/context7-mcp",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unexpected positional value `7`"));

    assert_eq!(fs::read(&environment.config).unwrap(), original_config);
    assert!(!environment.state.exists());

    environment
        .command()
        .args([
            "add",
            "context7",
            "--no-sync",
            "--",
            "npx",
            "-y",
            "@upstash/context7-mcp",
        ])
        .assert()
        .success();
    assert!(
        environment.parse_config()["servers"]
            .get("context7")
            .is_some()
    );
}

#[test]
fn invalid_server_id_is_rejected_before_writes() {
    let environment = TestEnvironment::new();
    environment.init();
    let original_config = fs::read(&environment.config).unwrap();

    environment
        .command()
        .args(["add", "../context7", "--", "npx"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("server ID must match"));

    assert_eq!(fs::read(&environment.config).unwrap(), original_config);
    assert!(!environment.state.exists());
}

#[test]
fn invalid_config_uses_exit_code_two() {
    let environment = TestEnvironment::new();
    fs::create_dir_all(environment.config.parent().unwrap()).unwrap();
    fs::write(&environment.config, "version=999\n").unwrap();
    environment
        .command()
        .arg("list")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unsupported schema version"));
}

#[test]
fn import_specific_stdio_is_lossless_marks_ownership_and_syncs_as_noop() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(
        r#"model = "gpt-example"

[mcp_servers.github]
command = "github-mcp"
args = ["stdio"]
env_vars = ["GITHUB_TOKEN"]
cwd = "/tmp/github"

[mcp_servers.playwright]
command = "playwright-mcp"
"#,
    );
    let target_before = fs::read(&environment.codex).unwrap();

    environment
        .command()
        .args(["import", "codex", "github"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Completed import from Codex"))
        .stdout(predicate::str::contains("+ github\tstdio"))
        .stdout(predicate::str::contains(
            "Target configuration was not modified",
        ));

    assert_eq!(fs::read(&environment.codex).unwrap(), target_before);
    let canonical = environment.parse_config();
    assert_eq!(
        canonical["servers"]["github"]["command"].as_str(),
        Some("github-mcp")
    );
    assert_eq!(
        canonical["servers"]["github"]["env"]["GITHUB_TOKEN"].as_str(),
        Some("${env:GITHUB_TOKEN}")
    );
    let state: toml::Value =
        toml::from_str(&fs::read_to_string(environment.state.join("state.toml")).unwrap()).unwrap();
    assert!(state["targets"]["codex"]["managed"].get("github").is_some());

    environment.enable_codex();
    environment
        .command()
        .arg("diff")
        .assert()
        .success()
        .stdout(predicate::str::contains("= synchronized"));
    let before_sync = snapshot_tree(environment.root());
    environment
        .command()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("already synchronized"));
    assert_eq!(snapshot_tree(environment.root()), before_sync);
}

#[test]
fn import_all_supports_stdio_and_http_and_reports_inventory() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(
        r#"[mcp_servers.github]
command = "github-mcp"

[mcp_servers.docs]
url = "https://docs.example/mcp"

[mcp_servers.docs.env_http_headers]
Authorization = "DOCS_TOKEN"
"#,
    );
    let target_before = fs::read(&environment.codex).unwrap();

    environment
        .command()
        .args(["import", "codex", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Completed import from Codex"))
        .stdout(predicate::str::contains("+ docs\thttp"))
        .stdout(predicate::str::contains("+ github\tstdio"));

    assert_eq!(fs::read(&environment.codex).unwrap(), target_before);
    let canonical = environment.parse_config();
    assert_eq!(
        canonical["servers"]["docs"]["transport"].as_str(),
        Some("http")
    );
    assert_eq!(
        canonical["servers"]["docs"]["headers"]["Authorization"].as_str(),
        Some("${env:DOCS_TOKEN}")
    );
    environment
        .command()
        .args(["diff", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No enabled targets."));
    environment
        .command()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Codex     disabled  2 managed / 0 unmanaged",
        ));
}

#[test]
fn import_dry_run_performs_absolutely_no_writes() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex("[mcp_servers.github]\ncommand = 'github-mcp'\n");
    let before = snapshot_tree(environment.root());

    environment
        .command()
        .args(["import", "codex", "github", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Dry-run import from Codex"))
        .stdout(predicate::str::contains(
            "No files or ownership state were modified",
        ));

    assert_eq!(snapshot_tree(environment.root()), before);
    assert!(!environment.state.exists());
}

#[test]
fn bulk_import_skips_canonical_collisions_without_overwriting_them() {
    let environment = TestEnvironment::new();
    environment.init();
    environment
        .command()
        .args(["add", "github", "--no-sync", "--", "canonical-github"])
        .assert()
        .success();
    environment.write_codex(
        "[mcp_servers.docs]\nurl = 'https://docs.example/mcp'\n\n[mcp_servers.github]\ncommand = 'target-github'\n",
    );
    let target_before = fs::read(&environment.codex).unwrap();

    environment
        .command()
        .args(["import", "codex", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("+ docs\thttp"))
        .stdout(predicate::str::contains("Skipped"))
        .stdout(predicate::str::contains("! github\talready exists"));

    assert_eq!(fs::read(&environment.codex).unwrap(), target_before);
    assert!(environment.parse_config()["servers"].get("docs").is_some());
    assert!(
        environment.parse_config()["servers"]
            .get("github")
            .is_some()
    );
}

#[test]
fn bulk_import_is_best_effort_and_only_commits_secrets_for_imported_servers() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(
        r#"[mcp_servers.github]
command = "github-mcp"
[mcp_servers.github.env]
GITHUB_TOKEN = "github-secret-canary"

[mcp_servers.playwright]
command = "playwright-mcp"

[mcp_servers.gh_grep]
command = "gh-grep-mcp"
startup_timeout_sec = 30
[mcp_servers.gh_grep.env]
GH_TOKEN = "skipped-secret-canary"
"#,
    );
    let target_before = fs::read(&environment.codex).unwrap();

    environment
        .command()
        .args(["import", "codex", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Imported"))
        .stdout(predicate::str::contains("+ github\tstdio"))
        .stdout(predicate::str::contains("+ playwright\tstdio"))
        .stdout(predicate::str::contains("Skipped"))
        .stdout(predicate::str::contains("! gh_grep\t"))
        .stdout(predicate::str::contains(
            "unsupported field `startup_timeout_sec`",
        ))
        .stdout(predicate::str::contains("✓ github.GITHUB_TOKEN"))
        .stdout(predicate::str::contains("gh_grep.GH_TOKEN").not())
        .stdout(predicate::str::contains("github-secret-canary").not())
        .stdout(predicate::str::contains("skipped-secret-canary").not());

    assert_eq!(fs::read(&environment.codex).unwrap(), target_before);
    let canonical = environment.parse_config();
    assert!(canonical["servers"].get("github").is_some());
    assert!(canonical["servers"].get("playwright").is_some());
    assert!(canonical["servers"].get("gh_grep").is_none());
    environment
        .command()
        .arg("secret")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("github.GITHUB_TOKEN"))
        .stdout(predicate::str::contains("gh_grep.GH_TOKEN").not());
}

#[test]
fn strict_bulk_and_single_imports_abort_when_a_server_is_not_lossless() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(
        "[mcp_servers.github]\ncommand = 'github-mcp'\n\n[mcp_servers.gh_grep]\ncommand = 'gh-grep-mcp'\nstartup_timeout_sec = 30\n",
    );
    let before = snapshot_tree(environment.root());

    environment
        .command()
        .args(["import", "codex", "--all", "--strict"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "unsupported field `startup_timeout_sec`",
        ));
    assert_eq!(snapshot_tree(environment.root()), before);

    environment
        .command()
        .args(["import", "codex", "gh_grep"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "unsupported field `startup_timeout_sec`",
        ));
    assert_eq!(snapshot_tree(environment.root()), before);
}

#[test]
fn import_automatically_migrates_strongly_sensitive_field_names() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(
        "[mcp_servers.github]\ncommand = 'github-mcp'\n\n[mcp_servers.github.env]\nGITHUB_TOKEN = 'do-not-copy'\n",
    );
    environment
        .command()
        .args(["import", "codex", "github"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ github.GITHUB_TOKEN"))
        .stdout(predicate::str::contains("do-not-copy").not());
    assert_eq!(
        environment.parse_config()["servers"]["github"]["env"]["GITHUB_TOKEN"]["secret"].as_str(),
        Some("github.GITHUB_TOKEN")
    );
}

#[test]
fn bulk_import_names_secrets_per_server_and_preserves_runtime_settings() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(
        r#"[mcp_servers.github]
command = "github-mcp"
[mcp_servers.github.env]
GITHUB_PERSONAL_ACCESS_TOKEN = "github-secret-canary"
API_URL = "https://api.github.example"
NODE_ENV = "production"

[mcp_servers.dokploy]
command = "dokploy-mcp"
[mcp_servers.dokploy.env]
DOKPLOY_API_KEY = "dokploy-secret-canary"
DOKPLOY_URL = "https://dokploy.example"
HOST = "localhost"
PORT = "3000"
"#,
    );
    let target_before = fs::read(&environment.codex).unwrap();

    environment
        .command()
        .args(["import", "codex", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✓ github.GITHUB_PERSONAL_ACCESS_TOKEN",
        ))
        .stdout(predicate::str::contains("✓ dokploy.DOKPLOY_API_KEY"))
        .stdout(predicate::str::contains("github-secret-canary").not())
        .stdout(predicate::str::contains("dokploy-secret-canary").not());

    assert_eq!(fs::read(&environment.codex).unwrap(), target_before);
    let canonical = environment.parse_config();
    assert_eq!(
        canonical["servers"]["github"]["env"]["GITHUB_PERSONAL_ACCESS_TOKEN"]["secret"].as_str(),
        Some("github.GITHUB_PERSONAL_ACCESS_TOKEN")
    );
    assert_eq!(
        canonical["servers"]["dokploy"]["env"]["DOKPLOY_API_KEY"]["secret"].as_str(),
        Some("dokploy.DOKPLOY_API_KEY")
    );
    for (server, field, expected) in [
        ("github", "API_URL", "https://api.github.example"),
        ("github", "NODE_ENV", "production"),
        ("dokploy", "DOKPLOY_URL", "https://dokploy.example"),
        ("dokploy", "HOST", "localhost"),
        ("dokploy", "PORT", "3000"),
    ] {
        assert_eq!(
            canonical["servers"][server]["env"][field].as_str(),
            Some(expected)
        );
    }
    let canonical_text = fs::read_to_string(&environment.config).unwrap();
    assert!(!canonical_text.contains("github-secret-canary"));
    assert!(!canonical_text.contains("dokploy-secret-canary"));
}

#[test]
fn status_distinguishes_disabled_synced_and_drifted_with_counts() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(
        "[mcp_servers.github]\ncommand = 'github-mcp'\n\n[mcp_servers.manual]\ncommand = 'manual-mcp'\n",
    );
    environment
        .command()
        .args(["import", "codex", "github"])
        .assert()
        .success();

    environment
        .command()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("disabled"))
        .stdout(predicate::str::contains("1 managed / 1 unmanaged"));
    environment.enable_codex();
    environment
        .command()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("synced"))
        .stdout(predicate::str::contains("1 managed / 1 unmanaged"));

    let drifted = fs::read_to_string(&environment.codex)
        .unwrap()
        .replace("command = 'github-mcp'", "command = 'changed-github'");
    fs::write(&environment.codex, drifted).unwrap();
    environment
        .command()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("drifted"))
        .stdout(predicate::str::contains("1 pending"));
}

#[test]
fn repeated_bulk_import_skips_managed_entries_without_writes() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex("[mcp_servers.github]\ncommand = 'github-mcp'\n");
    environment
        .command()
        .args(["import", "codex", "--all"])
        .assert()
        .success();
    let before = snapshot_tree(environment.root());

    environment
        .command()
        .args(["import", "codex", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Skipped"))
        .stdout(predicate::str::contains(
            "! github\talready managed by mcpd",
        ));

    assert_eq!(snapshot_tree(environment.root()), before);
}

#[test]
fn malformed_target_import_fails_without_modification() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex("not = [valid");
    let before = snapshot_tree(environment.root());

    environment
        .command()
        .args(["import", "codex", "--all"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("malformed TOML"));

    assert_eq!(snapshot_tree(environment.root()), before);
    assert!(!environment.state.exists());
}

#[test]
fn secret_commands_guard_reveal_and_track_presence() {
    let environment = TestEnvironment::new();
    environment.init();
    environment
        .command()
        .args(["secret", "set", "github.token"])
        .write_stdin("canary-secret-value\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("stored secret `github.token`"))
        .stdout(predicate::str::contains("canary-secret-value").not())
        .stderr(predicate::str::contains("canary-secret-value").not());
    assert_eq!(
        fs::metadata(&environment.state)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for path in [
        environment.state.join("secrets.toml"),
        environment.state.join("test-secret-store.toml"),
    ] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    environment
        .command()
        .args(["secret", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("github.token"))
        .stdout(predicate::str::contains("canary-secret-value").not());
    environment
        .command()
        .args(["secret", "check", "github.token"])
        .assert()
        .success()
        .stdout(predicate::str::contains("present"));
    environment
        .command()
        .args(["secret", "get", "github.token"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("canary-secret-value").not())
        .stderr(predicate::str::contains("--reveal"));
    environment
        .command()
        .args(["secret", "get", "github.token", "--reveal"])
        .assert()
        .success()
        .stdout("canary-secret-value\n")
        .stderr(predicate::str::contains("Warning: revealing"));
    environment
        .command()
        .args(["--json", "secret", "get", "github.token", "--reveal"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("canary-secret-value").not())
        .stderr(predicate::str::contains("canary-secret-value").not());
    environment
        .command()
        .args(["secret", "delete", "github.token"])
        .assert()
        .success()
        .stdout(predicate::str::contains("deleted"));
    environment
        .command()
        .args(["secret", "check", "github.token"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("missing"))
        .stderr(predicate::str::contains("canary-secret-value").not());
}

#[test]
fn canonical_secret_reference_syncs_through_exec_without_target_secret() {
    let environment = TestEnvironment::new();
    environment.init();
    fs::write(
        &environment.config,
        r#"version = 1

[servers.secure]
transport = "stdio"
command = "/bin/sh"
args = ["-c", "test -n \"$ACCESS_TOKEN\""]

[servers.secure.env]
ACCESS_TOKEN = { secret = "secure.token" }

[targets.codex]
enabled = true
"#,
    )
    .unwrap();
    environment.write_codex("model = 'gpt-example'\n");
    environment
        .command()
        .args(["secret", "set", "secure.token"])
        .write_stdin("runtime-only-canary\n")
        .assert()
        .success();

    environment.command().arg("sync").assert().success();
    let target = fs::read_to_string(&environment.codex).unwrap();
    assert!(target.contains("command = \"mcpd\""));
    assert!(target.contains("args = [\"exec\", \"secure\"]"));
    assert!(!target.contains("runtime-only-canary"));
    assert!(
        !fs::read_to_string(&environment.config)
            .unwrap()
            .contains("runtime-only-canary")
    );
    let state_after_sync = snapshot_tree(&environment.state);
    environment
        .command()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("already synchronized"));
    assert_eq!(fs::read_to_string(&environment.codex).unwrap(), target);
    assert_eq!(snapshot_tree(&environment.state), state_after_sync);

    environment
        .command()
        .args(["exec", "secure"])
        .assert()
        .success()
        .stdout(predicate::str::contains("runtime-only-canary").not())
        .stderr(predicate::str::contains("runtime-only-canary").not());
}

#[test]
fn missing_exec_secret_and_doctor_are_safe_and_actionable() {
    let environment = TestEnvironment::new();
    environment.init();
    fs::write(
        &environment.config,
        r#"version = 1
[servers.secure]
transport = "stdio"
command = "/bin/true"
[servers.secure.env]
TOKEN = { secret = "missing.token" }
[targets]
"#,
    )
    .unwrap();
    environment
        .command()
        .args(["exec", "secure"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("secret `missing.token` required"))
        .stderr(predicate::str::contains("secret set missing.token"));
    environment
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("missing secret `missing.token`"));
    environment
        .command()
        .args(["secret", "set", "missing.token"])
        .write_stdin("doctor-canary\n")
        .assert()
        .success();
    environment
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("present secret `missing.token`"))
        .stdout(predicate::str::contains("doctor-canary").not());
}

#[test]
fn import_uses_deterministic_secret_name_and_preserves_non_sensitive_literals() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(
        "[mcp_servers.github]\ncommand = 'github-mcp'\n\n[mcp_servers.github.env]\nGITHUB_PERSONAL_ACCESS_TOKEN = 'import-secret-canary'\nPUBLIC_HOST = 'github.com'\n",
    );
    let target_before = fs::read(&environment.codex).unwrap();

    environment
        .command()
        .args(["import", "codex", "github"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✓ github.GITHUB_PERSONAL_ACCESS_TOKEN",
        ))
        .stdout(predicate::str::contains("import-secret-canary").not());

    assert_eq!(fs::read(&environment.codex).unwrap(), target_before);
    let canonical_text = fs::read_to_string(&environment.config).unwrap();
    assert!(canonical_text.contains("secret = \"github.GITHUB_PERSONAL_ACCESS_TOKEN\""));
    assert!(canonical_text.contains("PUBLIC_HOST = \"github.com\""));
    assert!(!canonical_text.contains("import-secret-canary"));
    assert!(
        !fs::read_to_string(environment.state.join("secrets.toml"))
            .unwrap()
            .contains("import-secret-canary")
    );
    environment
        .command()
        .args(["secret", "check", "github.GITHUB_PERSONAL_ACCESS_TOKEN"])
        .assert()
        .success();

    environment.enable_codex();
    environment.command().arg("sync").assert().success();
    let target = fs::read_to_string(&environment.codex).unwrap();
    assert!(target.contains("command = \"mcpd\""));
    assert!(!target.contains("import-secret-canary"));
}

#[test]
fn secret_import_dry_run_has_zero_writes() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(
        "[mcp_servers.github]\ncommand = 'github-mcp'\n[mcp_servers.github.env]\nGITHUB_TOKEN = 'dry-run-canary'\n",
    );
    let before = snapshot_tree(environment.root());

    environment
        .command()
        .args(["import", "codex", "github", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Dry-run import from Codex"))
        .stdout(predicate::str::contains("dry-run-canary").not());

    assert_eq!(snapshot_tree(environment.root()), before);
    assert!(!environment.state.exists());
}

#[test]
fn ordinary_environment_literals_import_without_secret_classification() {
    let environment = TestEnvironment::new();
    environment.init();
    environment.write_codex(
        "[mcp_servers.github]\ncommand='github-mcp'\n[mcp_servers.github.env]\nPUBLIC_HOST='github.example'\n",
    );
    environment
        .command()
        .args(["import", "codex", "github"])
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(&environment.codex).unwrap(),
        "[mcp_servers.github]\ncommand='github-mcp'\n[mcp_servers.github.env]\nPUBLIC_HOST='github.example'\n"
    );
    assert_eq!(
        environment.parse_config()["servers"]["github"]["env"]["PUBLIC_HOST"].as_str(),
        Some("github.example")
    );
}

#[test]
fn import_never_overwrites_an_existing_keyring_secret() {
    let environment = TestEnvironment::new();
    environment.init();
    environment
        .command()
        .args(["secret", "set", "github.GITHUB_TOKEN"])
        .write_stdin("existing-secret\n")
        .assert()
        .success();
    environment.write_codex(
        "[mcp_servers.github]\ncommand='github-mcp'\n[mcp_servers.github.env]\nGITHUB_TOKEN='different-target-secret'\n",
    );
    let before = snapshot_tree(environment.root());

    environment
        .command()
        .args([
            "import",
            "codex",
            "github",
            "--secret",
            "GITHUB_TOKEN=github.GITHUB_TOKEN",
        ])
        .assert()
        .code(4)
        .stderr(predicate::str::contains(
            "already exists with a different value",
        ))
        .stderr(predicate::str::contains("existing-secret").not())
        .stderr(predicate::str::contains("different-target-secret").not());

    assert_eq!(snapshot_tree(environment.root()), before);
    assert!(
        environment.parse_config()["servers"]
            .as_table()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn sensitive_literal_in_canonical_config_is_rejected() {
    let environment = TestEnvironment::new();
    environment.init();
    fs::write(
        &environment.config,
        "version=1\n[servers.bad]\ntransport='stdio'\ncommand='bad'\n[servers.bad.env]\nAPI_TOKEN='literal-canary'\n[targets]\n",
    )
    .unwrap();
    environment
        .command()
        .arg("list")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("looks secret-bearing"))
        .stderr(predicate::str::contains("literal-canary").not());
}

#[test]
fn http_keyring_reference_is_rejected_without_target_or_keyring_mutation() {
    let environment = TestEnvironment::new();
    environment.init();
    fs::write(
        &environment.config,
        r#"version=1
[servers.remote]
transport="http"
url="https://example.test/mcp"
[servers.remote.headers]
Authorization={secret="remote.token"}
[targets.codex]
enabled=true
"#,
    )
    .unwrap();
    environment.write_codex("model='gpt-example'\n");
    let before = snapshot_tree(environment.root());

    environment
        .command()
        .args(["sync", "--dry-run"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cannot resolve OS-keyring secret `remote.token`",
        ))
        .stderr(predicate::str::contains("does not proxy HTTP"));

    assert_eq!(snapshot_tree(environment.root()), before);
    assert!(!environment.state.exists());
}
