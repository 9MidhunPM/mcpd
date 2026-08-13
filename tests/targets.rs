use std::{collections::BTreeMap, fs, path::PathBuf};

use mcpd::{
    Paths, config, import,
    model::{ConfigValue, Server},
    sync,
};
use tempfile::TempDir;
use url::Url;

fn paths(temp: &TempDir) -> Paths {
    let home = temp.path().join("home");
    Paths {
        config: temp.path().join("config/mcpd/config.toml"),
        state_dir: temp.path().join("state/mcpd"),
        codex_config: home.join(".codex/config.toml"),
        home,
    }
}

#[test]
fn every_json_adapter_syncs_stdio_and_http_preserves_unmanaged_and_is_idempotent() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    config::init(&paths.config).unwrap();
    config::add_server(
        &paths.config,
        "local",
        &Server::Stdio {
            command: "node".into(),
            args: vec!["server.js".into()],
            env: BTreeMap::new(),
            secrets: BTreeMap::new(),
            cwd: None,
        },
    )
    .unwrap();
    config::add_server(
        &paths.config,
        "remote",
        &Server::Http {
            url: Url::parse("https://example.com/mcp").unwrap(),
            headers: BTreeMap::from([("X-Client".into(), ConfigValue::literal("mcpd"))]),
            secrets: BTreeMap::new(),
        },
    )
    .unwrap();
    for target in ["claude", "cursor", "antigravity", "openchamber"] {
        config::set_target_enabled(&paths.config, target, true).unwrap();
        let path = paths.target_config(target).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let initial = if target == "openchamber" {
            r#"{"model":"keep/me","mcp":{"servers":{"manual":{"type":"local","command":["manual"]}}}}"#
        } else {
            r#"{"theme":"keep-me","mcpServers":{"manual":{"type":"stdio","command":"manual"}}}"#
        };
        fs::write(path, initial).unwrap();
    }
    let canonical = config::load(&paths.config).unwrap();
    let reports = sync::sync_enabled_targets(&canonical, &paths, false).unwrap();
    assert_eq!(reports.len(), 4);
    assert!(reports.iter().all(|report| report.changes.len() == 2));
    for target in ["claude", "cursor", "antigravity", "openchamber"] {
        let path = paths.target_config(target).unwrap();
        let before = fs::read(&path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&before).unwrap();
        if target == "openchamber" {
            assert_eq!(value["model"], "keep/me");
            assert!(value["mcp"]["servers"]["manual"].is_object());
            assert!(value["mcp"]["servers"]["local"].is_object());
            assert!(value["mcp"]["servers"]["remote"].is_object());
        } else {
            assert_eq!(value["theme"], "keep-me");
            assert!(value["mcpServers"]["manual"].is_object());
            assert!(value["mcpServers"]["local"].is_object());
            assert!(value["mcpServers"]["remote"].is_object());
        }
        let second = sync::sync_enabled_targets(&canonical, &paths, false).unwrap();
        assert!(
            second
                .iter()
                .find(|report| report.target == target)
                .unwrap()
                .changes
                .is_empty()
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

#[test]
fn every_json_adapter_imports_native_stdio_and_remote_without_modifying_source() {
    for target in ["claude", "cursor", "antigravity", "openchamber"] {
        let temp = TempDir::new().unwrap();
        let paths = paths(&temp);
        config::init(&paths.config).unwrap();
        let path = paths.target_config(target).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let native = match target { "openchamber" => r#"{"other":true,"mcp":{"servers":{"local":{"type":"local","command":["node","server.js"],"environment":{"LOG_LEVEL":"info"}},"remote":{"type":"remote","url":"https://example.com/mcp","oauth":false,"headers":{"Authorization":"{env:REMOTE_TOKEN}"}}}}}"#.to_owned(), "antigravity" => r#"{"other":true,"mcpServers":{"local":{"command":"node","args":["server.js"],"env":{"LOG_LEVEL":"info"}},"remote":{"serverUrl":"https://example.com/mcp","headers":{"X-Client":"mcpd"}}}}"#.to_owned(), "cursor" => r#"{"other":true,"mcpServers":{"local":{"command":"node","args":["server.js"],"env":{"LOG_LEVEL":"info"}},"remote":{"url":"https://example.com/mcp","headers":{"Authorization":"${env:REMOTE_TOKEN}"}}}}"#.to_owned(), _ => r#"{"other":true,"mcpServers":{"local":{"type":"stdio","command":"node","args":["server.js"],"env":{"LOG_LEVEL":"info"}},"remote":{"type":"http","url":"https://example.com/mcp","headers":{"Authorization":"${REMOTE_TOKEN}"}}}}"#.to_owned() };
        fs::write(&path, &native).unwrap();
        let before = fs::read(&path).unwrap();
        let report =
            import::import_target(target, None, &paths, false, false, BTreeMap::new()).unwrap();
        assert_eq!(report.imported.len(), 2, "{target}: {:?}", report.skipped);
        assert_eq!(fs::read(&path).unwrap(), before);
        let canonical = config::load(&paths.config).unwrap();
        assert!(canonical.servers.contains_key("local"));
        assert!(canonical.servers.contains_key("remote"));
        let Server::Stdio { env, .. } = &canonical.servers["local"] else {
            panic!()
        };
        assert_eq!(env["LOG_LEVEL"], ConfigValue::literal("info"));
        let Server::Http { headers, .. } = &canonical.servers["remote"] else {
            panic!()
        };
        if target == "antigravity" {
            assert_eq!(headers["X-Client"], ConfigValue::literal("mcpd"));
        } else {
            assert_eq!(
                headers["Authorization"],
                ConfigValue::literal("${env:REMOTE_TOKEN}")
            );
        }
    }
}

#[test]
fn all_target_dry_run_has_no_writes_and_malformed_json_fails_safely() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    config::init(&paths.config).unwrap();
    config::add_server(
        &paths.config,
        "local",
        &Server::Stdio {
            command: "local".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            secrets: BTreeMap::new(),
            cwd: None,
        },
    )
    .unwrap();
    for target in ["claude", "cursor", "antigravity", "openchamber"] {
        config::set_target_enabled(&paths.config, target, true).unwrap();
    }
    let canonical = config::load(&paths.config).unwrap();
    let before = tree(&temp);
    let reports = sync::sync_enabled_targets(&canonical, &paths, true).unwrap();
    assert_eq!(reports.len(), 4);
    assert_eq!(tree(&temp), before);
    let cursor = paths.target_config("cursor").unwrap();
    fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    fs::write(&cursor, b"{ malformed").unwrap();
    let before = fs::read(&cursor).unwrap();
    assert!(sync::sync_enabled_targets(&canonical, &paths, false).is_err());
    assert_eq!(fs::read(cursor).unwrap(), before);
}

#[test]
fn adapter_import_never_discards_native_oauth_or_disabled_semantics() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    config::init(&paths.config).unwrap();
    let path = paths.target_config("openchamber").unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, r#"{"mcp":{"servers":{"oauth":{"type":"remote","url":"https://example.com/mcp","oauth":{"client_id":"native"}},"disabled":{"type":"local","command":["disabled"],"disabled":true},"safe":{"type":"local","command":["safe"]}}}}"#).unwrap();
    let before = fs::read(&path).unwrap();
    let report =
        import::import_target("openchamber", None, &paths, false, false, BTreeMap::new()).unwrap();
    assert_eq!(
        report
            .imported
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["safe"]
    );
    assert_eq!(report.skipped.len(), 2);
    assert!(
        report
            .skipped
            .iter()
            .any(|entry| entry.reason.contains("native OAuth"))
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|entry| entry.reason.contains("disabled natively"))
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn jsonc_comments_trailing_commas_and_unmanaged_servers_survive_sync() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    config::init(&paths.config).unwrap();
    config::add_server(
        &paths.config,
        "managed",
        &Server::Stdio {
            command: "managed-command".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            secrets: BTreeMap::new(),
            cwd: None,
        },
    )
    .unwrap();
    config::set_target_enabled(&paths.config, "cursor", true).unwrap();
    let cursor = paths.target_config("cursor").unwrap();
    fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    fs::write(
        &cursor,
        r#"{
  // keep outer
  "mcpServers": {
    // keep unmanaged
    "manual": { "command": "manual", },
  },
}"#,
    )
    .unwrap();

    let canonical = config::load(&paths.config).unwrap();
    sync::sync_enabled_targets(&canonical, &paths, false).unwrap();
    let first = fs::read_to_string(&cursor).unwrap();
    assert!(first.contains("// keep outer"));
    assert!(first.contains("// keep unmanaged"));
    assert!(first.contains(r#""manual": { "command": "manual", }"#));
    assert!(first.contains("managed-command"));
    sync::sync_enabled_targets(&canonical, &paths, false).unwrap();
    assert_eq!(fs::read_to_string(&cursor).unwrap(), first);
}

#[test]
fn declarative_jsonc_adapter_is_discovered_merge_safe_and_idempotent() {
    let temp = TempDir::new().unwrap();
    let paths = paths(&temp);
    config::init(&paths.config).unwrap();
    let manifests = paths.config.parent().unwrap().join("targets");
    fs::create_dir_all(&manifests).unwrap();
    fs::write(
        manifests.join("banana.toml"),
        r#"
id = "banana"
name = "Banana Code"
platforms = ["linux"]

[detect]
commands = ["banana"]

[config]
path = "~/.banana/config.jsonc"
format = "jsonc"
servers_path = "mcp.servers"
"#,
    )
    .unwrap();
    config::add_server(
        &paths.config,
        "managed",
        &Server::Stdio {
            command: "banana-mcp".into(),
            args: vec!["--safe".into()],
            env: BTreeMap::new(),
            secrets: BTreeMap::new(),
            cwd: None,
        },
    )
    .unwrap();
    config::set_target_enabled(&paths.config, "banana", true).unwrap();
    let target = paths.home.join(".banana/config.jsonc");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(
        &target,
        r#"{
  // custom target setting
  "theme": "yellow",
  "mcp": { "servers": { "manual": { "command": "manual" } } }
}"#,
    )
    .unwrap();

    let canonical = config::load(&paths.config).unwrap();
    let first = sync::sync_enabled_targets(&canonical, &paths, false).unwrap();
    assert_eq!(first.len(), 1);
    let rendered = fs::read_to_string(&target).unwrap();
    assert!(rendered.contains("// custom target setting"));
    assert!(rendered.contains(r#""manual": { "command": "manual" }"#));
    assert!(rendered.contains("banana-mcp"));
    let second = sync::sync_enabled_targets(&canonical, &paths, false).unwrap();
    assert!(second[0].changes.is_empty());
    assert_eq!(fs::read_to_string(target).unwrap(), rendered);
}

fn tree(temp: &TempDir) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &std::path::Path, at: &std::path::Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if let Ok(entries) = fs::read_dir(at) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit(root, &path, out);
                } else {
                    out.insert(
                        path.strip_prefix(root).unwrap().to_owned(),
                        fs::read(path).unwrap(),
                    );
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(temp.path(), temp.path(), &mut out);
    out
}
