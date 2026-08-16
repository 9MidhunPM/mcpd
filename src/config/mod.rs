use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value, value};

use crate::{
    Paths,
    diagnostics::{McpdError, Result},
    model::{CanonicalConfig, ConfigValue, Server, TargetServerConfig},
    state,
    sync::fs::atomic_write,
};

pub const EMPTY_CONFIG: &str = "version = 1\n\n[servers]\n\n[targets]\n";

#[derive(Debug, Clone)]
pub struct AddServersPlan {
    pub before: Vec<u8>,
    pub rendered: Vec<u8>,
    pub config: CanonicalConfig,
}

pub fn load(path: &Path) -> Result<CanonicalConfig> {
    let text = fs::read_to_string(path).map_err(|source| McpdError::io(path, source))?;
    parse(&text, path)
}

pub fn parse(text: &str, path: &Path) -> Result<CanonicalConfig> {
    let mut config: CanonicalConfig =
        toml::from_str(text).map_err(|_| McpdError::InvalidInput {
            message: format!("{} is not a valid mcpd configuration", path.display()),
            hint: "fix the reported field or run `mcpd init` if this is a new configuration".into(),
        })?;
    if !config.targets.contains_key("opencode") {
        if let Some(legacy) = config.targets.remove("openchamber") {
            config.targets.insert("opencode".into(), legacy);
        }
    }
    validate(&config, path)?;
    Ok(config)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectOverlay {
    version: u32,
    #[serde(default)]
    servers: BTreeMap<String, toml::Value>,
    #[serde(default)]
    targets: BTreeMap<String, OverlayTarget>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayTarget {
    enabled: Option<bool>,
    client_version: Option<String>,
    #[serde(default)]
    servers: BTreeMap<String, TargetServerConfig>,
}

pub fn apply_overlay(global: &CanonicalConfig, path: &Path) -> Result<CanonicalConfig> {
    let text = fs::read_to_string(path).map_err(|source| McpdError::io(path, source))?;
    let overlay: ProjectOverlay =
        toml::from_str(&text).map_err(|error| McpdError::InvalidInput {
            message: format!("project overlay {} is invalid: {error}", path.display()),
            hint: "repair the trusted project overlay; the global configuration was not modified"
                .into(),
        })?;
    if overlay.version != 1 {
        return Err(McpdError::InvalidInput {
            message: format!(
                "project overlay {} uses unsupported schema version {}",
                path.display(),
                overlay.version
            ),
            hint: "mcpd 1.x supports only `version = 1` and never migrates implicitly".into(),
        });
    }

    let mut resolved = global.clone();
    for (name, value) in overlay.servers {
        if !is_valid_server_id(&name) {
            return Err(McpdError::InvalidInput {
                message: format!("project overlay server ID `{name}` is invalid"),
                hint: "use letters, digits, dots, underscores, or hyphens; start with a letter or digit".into(),
            });
        }
        let table = value.as_table().ok_or_else(|| McpdError::InvalidInput {
            message: format!("project overlay server `{name}` must be a TOML table"),
            hint: "define a complete server or use `enabled = false` to disable it".into(),
        })?;
        let enabled = table.get("enabled").and_then(toml::Value::as_bool);
        if enabled == Some(false) {
            if table.len() != 1 {
                return Err(McpdError::InvalidInput {
                    message: format!(
                        "disabled project overlay server `{name}` also defines server fields"
                    ),
                    hint: "use only `enabled = false`, or provide a complete enabled server definition".into(),
                });
            }
            resolved.servers.remove(&name);
            continue;
        }
        let mut server_value = value;
        let Some(server_table) = server_value.as_table_mut() else {
            return Err(McpdError::Operational {
                message: format!("project overlay server `{name}` changed type during validation"),
                hint: "report this as an mcpd bug".into(),
            });
        };
        server_table.remove("enabled");
        let server: Server = server_value
            .try_into()
            .map_err(|error| McpdError::InvalidInput {
                message: format!("project overlay server `{name}` is invalid: {error}"),
                hint: "define a complete stdio or HTTP server in the overlay".into(),
            })?;
        resolved.servers.insert(name, server);
    }
    for (id, overlay_target) in overlay.targets {
        let target = resolved.targets.entry(id).or_default();
        if let Some(enabled) = overlay_target.enabled {
            target.enabled = enabled;
        }
        if let Some(version) = overlay_target.client_version {
            target.client_version = Some(version);
        }
        target.servers.extend(overlay_target.servers);
    }
    validate(&resolved, path)?;
    Ok(resolved)
}

fn validate(config: &CanonicalConfig, path: &Path) -> Result<()> {
    if config.version != 1 {
        return Err(McpdError::InvalidInput {
            message: format!(
                "{} uses unsupported schema version {}",
                path.display(),
                config.version
            ),
            hint: "mcpd 1.x supports only `version = 1` and never migrates implicitly".into(),
        });
    }
    for (id, target) in &config.targets {
        if !is_valid_server_id(id) {
            return Err(McpdError::InvalidInput {
                message: format!("target ID `{id}` is invalid"),
                hint: "use letters, digits, dots, underscores, or hyphens; start with a letter or digit".into(),
            });
        }
        if let Some(version) = &target.client_version
            && (version.trim().is_empty()
                || version.len() > 128
                || version.chars().any(char::is_control))
        {
            return Err(McpdError::InvalidInput {
                message: format!("target `{id}` has an invalid client_version"),
                hint: "use the printable version reported by the client, for example `1.2.3`"
                    .into(),
            });
        }
    }
    for (name, server) in &config.servers {
        if !is_valid_server_id(name) {
            return Err(McpdError::InvalidInput {
                message: format!("server ID `{name}` is invalid"),
                hint: "use letters, digits, dots, underscores, or hyphens; start with a letter or digit"
                    .into(),
            });
        }
        match server {
            Server::Stdio { command, .. } if command.trim().is_empty() => {
                return Err(McpdError::InvalidInput {
                    message: format!("stdio server `{name}` has an empty command"),
                    hint: "set `command` to an executable name or path".into(),
                });
            }
            Server::Http { url, .. } if !matches!(url.scheme(), "http" | "https") => {
                return Err(McpdError::InvalidInput {
                    message: format!(
                        "HTTP server `{name}` uses unsupported URL scheme `{}`",
                        url.scheme()
                    ),
                    hint: "use an absolute http:// or https:// Streamable HTTP URL".into(),
                });
            }
            _ => {}
        }
        let (values, legacy_secrets) = match server {
            Server::Stdio { env, secrets, .. } => (env, secrets),
            Server::Http {
                headers, secrets, ..
            } => (headers, secrets),
        };
        for (field, value) in values {
            if let Some(secret) = value.secret_name() {
                crate::secrets::validate_name(secret)?;
            }
            if is_sensitive_field(field)
                && matches!(value, ConfigValue::Literal(literal) if !is_complete_env_reference(literal) && value.secret_name().is_none())
            {
                return Err(McpdError::InvalidInput {
                    message: format!(
                        "server `{name}` field `{field}` looks secret-bearing but contains a literal value"
                    ),
                    hint: "store the value with `mcpd secret set NAME` and use `{ secret = \"NAME\" }`"
                        .into(),
                });
            }
        }
        for secret in legacy_secrets.values() {
            crate::secrets::validate_name(secret)?;
        }
    }
    Ok(())
}

fn is_complete_env_reference(value: &str) -> bool {
    value
        .strip_prefix("${env:")
        .and_then(|value| value.strip_suffix('}'))
        .is_some_and(|name| !name.is_empty() && !name.chars().any(char::is_whitespace))
}

pub fn is_sensitive_field(name: &str) -> bool {
    let normalized = name.to_ascii_uppercase().replace('-', "_");
    normalized == "AUTHORIZATION"
        || credential_suffix(&normalized, "API_KEY")
        || credential_suffix(&normalized, "TOKEN")
        || credential_suffix(&normalized, "PASSWORD")
        || credential_suffix(&normalized, "SECRET")
        || credential_suffix(&normalized, "CREDENTIAL")
        || credential_suffix(&normalized, "PRIVATE_KEY")
}

fn credential_suffix(name: &str, suffix: &str) -> bool {
    name == suffix || name.ends_with(&format!("_{suffix}"))
}

pub fn is_valid_server_id(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

pub fn parse_server_id(value: &str) -> std::result::Result<String, String> {
    if is_valid_server_id(value) {
        Ok(value.to_owned())
    } else {
        Err("server ID must match [a-zA-Z0-9][a-zA-Z0-9._-]*".into())
    }
}

pub fn init(paths: &Paths) -> Result<()> {
    let path = &paths.config;
    if path.exists() {
        return Err(McpdError::Conflict {
            message: format!(
                "refusing to overwrite existing canonical config {}",
                path.display()
            ),
            hint: "edit the existing file or remove it explicitly before running `mcpd init`"
                .into(),
        });
    }
    let ownership = state::load(&paths.state_dir.join("state.toml"))?;
    let managed = ownership.managed_server_names();
    if !managed.is_empty() {
        return Err(McpdError::Conflict {
            message: format!(
                "refusing to initialize an empty canonical config: ownership state still tracks {} managed server(s): {}",
                managed.len(),
                managed.iter().cloned().collect::<Vec<_>>().join(", ")
            ),
            hint: "restore the canonical config from a known-good copy or recover definitions from a target; do not discard ownership state until every managed entry has been reviewed".into(),
        });
    }
    atomic_write(path, EMPTY_CONFIG.as_bytes(), Some(0o600))
}

/// Commit a prepared schema rewrite without permitting server loss.
///
/// Future schema migrations must use this transaction instead of replacing the
/// canonical file directly. The old bytes are backed up before the atomic
/// replacement, and both the old server table and ownership state constrain the
/// rendered result.
pub fn apply_schema_update(paths: &Paths, before: &[u8], rendered: &[u8]) -> Result<PathBuf> {
    let current = fs::read(&paths.config).map_err(|source| McpdError::io(&paths.config, source))?;
    if current != before {
        return Err(McpdError::Conflict {
            message: format!(
                "{} changed after the schema update was planned",
                paths.config.display()
            ),
            hint: "reload the canonical config and re-plan the migration; no file was modified"
                .into(),
        });
    }
    let old_servers = raw_server_names(before, &paths.config)?;
    let rendered_text = std::str::from_utf8(rendered).map_err(|_| McpdError::InvalidInput {
        message: "schema update rendered a non-UTF-8 canonical config".into(),
        hint: "fix the migration; no file was modified".into(),
    })?;
    let updated = parse(rendered_text, &paths.config)?;
    let updated_servers = updated.servers.keys().cloned().collect::<BTreeSet<_>>();
    let ownership = state::load(&paths.state_dir.join("state.toml"))?;
    let managed_servers = ownership.managed_server_names();
    let required = old_servers
        .union(&managed_servers)
        .cloned()
        .collect::<BTreeSet<_>>();
    let missing = required
        .difference(&updated_servers)
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(McpdError::Conflict {
            message: format!(
                "schema update would discard canonical or managed server(s): {}",
                missing.join(", ")
            ),
            hint: "fix the migration to preserve every server definition; removals must be a separate explicit user operation".into(),
        });
    }

    let backup_dir = paths.state_dir.join("backups/canonical");
    crate::sync::ensure_private_dir(&backup_dir)?;
    let backup = backup_dir.join(format!(
        "schema-{}.toml",
        crate::sync::fs::hash_bytes(before)
    ));
    if backup.exists() {
        let existing = fs::read(&backup).map_err(|source| McpdError::io(&backup, source))?;
        if existing != before {
            return Err(McpdError::Conflict {
                message: format!("canonical migration backup {} conflicts", backup.display()),
                hint: "inspect the backup directory; no canonical file was modified".into(),
            });
        }
    } else {
        atomic_write(&backup, before, Some(0o600))?;
    }
    atomic_write(&paths.config, rendered, None)?;
    Ok(backup)
}

fn raw_server_names(bytes: &[u8], path: &Path) -> Result<BTreeSet<String>> {
    let text = std::str::from_utf8(bytes).map_err(|_| McpdError::InvalidInput {
        message: format!("{} is not valid UTF-8", path.display()),
        hint: "repair the old canonical config before migrating it".into(),
    })?;
    let value: toml::Value = toml::from_str(text).map_err(|error| McpdError::InvalidInput {
        message: format!(
            "{} cannot be inspected for migration: {error}",
            path.display()
        ),
        hint: "repair the old canonical config before migrating it".into(),
    })?;
    Ok(value
        .get("servers")
        .and_then(toml::Value::as_table)
        .map(|servers| servers.keys().cloned().collect())
        .unwrap_or_default())
}

pub fn add_server(path: &Path, name: &str, server: &Server) -> Result<()> {
    mutate(path, |doc| {
        let servers = ensure_table(doc, "servers")?;
        if servers.contains_key(name) {
            return Err(McpdError::Conflict {
                message: format!("server `{name}` already exists in {}", path.display()),
                hint: "remove it first or choose a different name".into(),
            });
        }
        servers.insert(name, server_item(server));
        Ok(())
    })
}

pub fn plan_add_servers(
    path: &Path,
    additions: &BTreeMap<String, Server>,
) -> Result<AddServersPlan> {
    let before = fs::read(path).map_err(|source| McpdError::io(path, source))?;
    let text = std::str::from_utf8(&before).map_err(|_| McpdError::InvalidInput {
        message: format!("{} is not valid UTF-8", path.display()),
        hint: "repair the canonical configuration before importing".into(),
    })?;
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|_| McpdError::InvalidInput {
            message: format!("{} is not valid TOML", path.display()),
            hint: "repair the TOML before asking mcpd to modify it".into(),
        })?;
    {
        let servers = ensure_table(&mut doc, "servers")?;
        if let Some(name) = additions.keys().find(|name| servers.contains_key(name)) {
            return Err(McpdError::Conflict {
                message: format!("server `{name}` already exists in {}", path.display()),
                hint: "keep the target entry unmanaged or remove/rename the canonical entry explicitly"
                    .into(),
            });
        }
        for (name, server) in additions {
            servers.insert(name, server_item(server));
        }
    }
    let rendered_text = doc.to_string();
    let config = parse(&rendered_text, path)?;
    let rendered = rendered_text.into_bytes();
    Ok(AddServersPlan {
        before,
        rendered,
        config,
    })
}

pub fn apply_add_servers(path: &Path, plan: &AddServersPlan) -> Result<()> {
    let current = fs::read(path).map_err(|source| McpdError::io(path, source))?;
    if current != plan.before {
        return Err(McpdError::Conflict {
            message: format!("{} changed after import was planned", path.display()),
            hint: "review the external edit and rerun import; no write was performed".into(),
        });
    }
    atomic_write(path, &plan.rendered, None)
}

pub fn remove_server(path: &Path, name: &str) -> Result<()> {
    mutate(path, |doc| {
        let servers = ensure_table(doc, "servers")?;
        if servers.remove(name).is_none() {
            return Err(McpdError::InvalidInput {
                message: format!("server `{name}` does not exist"),
                hint: "run `mcpd list` to see canonical servers".into(),
            });
        }
        Ok(())
    })
}

pub fn set_target_enabled(path: &Path, target: &str, enabled: bool) -> Result<()> {
    let target = if target == "openchamber" {
        "opencode"
    } else {
        target
    };
    mutate(path, |doc| {
        let targets = ensure_table(doc, "targets")?;
        if !targets.contains_key(target) {
            targets.insert(target, Item::Table(Table::new()));
        }
        let table = targets[target]
            .as_table_mut()
            .ok_or_else(|| McpdError::InvalidInput {
                message: format!("[targets.{target}] must be a table"),
                hint: "replace the value with a TOML table".into(),
            })?;
        table.insert("enabled", value(enabled));
        Ok(())
    })
}

fn mutate(path: &Path, operation: impl FnOnce(&mut DocumentMut) -> Result<()>) -> Result<()> {
    let text = fs::read_to_string(path).map_err(|source| McpdError::io(path, source))?;
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|_| McpdError::InvalidInput {
            message: format!("{} is not valid TOML", path.display()),
            hint: "repair the TOML before asking mcpd to modify it".into(),
        })?;
    operation(&mut doc)?;
    let rendered = doc.to_string();
    parse(&rendered, path)?;
    atomic_write(path, rendered.as_bytes(), None)
}

fn ensure_table<'a>(doc: &'a mut DocumentMut, key: &str) -> Result<&'a mut Table> {
    if !doc.contains_key(key) {
        doc.insert(key, Item::Table(Table::new()));
    }
    doc[key]
        .as_table_mut()
        .ok_or_else(|| McpdError::InvalidInput {
            message: format!("`{key}` must be a TOML table"),
            hint: format!("replace `{key}` with `[{key}]`"),
        })
}

pub fn server_item(server: &Server) -> Item {
    let mut table = Table::new();
    match server {
        Server::Stdio {
            command,
            args,
            env,
            secrets,
            cwd,
        } => {
            table.insert("transport", value("stdio"));
            table.insert("command", value(command.clone()));
            if !args.is_empty() {
                let mut array = Array::new();
                for arg in args {
                    array.push(arg.as_str());
                }
                table.insert("args", Item::Value(Value::Array(array)));
            }
            if let Some(cwd) = cwd {
                table.insert("cwd", value(cwd.to_string_lossy().to_string()));
            }
            insert_value_table(&mut table, "env", env);
            insert_string_table(&mut table, "secrets", secrets);
        }
        Server::Http {
            url,
            headers,
            secrets,
        } => {
            table.insert("transport", value("http"));
            table.insert("url", value(url.as_str()));
            insert_value_table(&mut table, "headers", headers);
            insert_string_table(&mut table, "secrets", secrets);
        }
    }
    Item::Table(table)
}

fn insert_string_table(
    table: &mut Table,
    key: &str,
    values: &std::collections::BTreeMap<String, String>,
) {
    if values.is_empty() {
        return;
    }
    let mut child = Table::new();
    for (name, value_) in values {
        child.insert(name, value(value_.clone()));
    }
    table.insert(key, Item::Table(child));
}

fn insert_value_table(table: &mut Table, key: &str, values: &BTreeMap<String, ConfigValue>) {
    if values.is_empty() {
        return;
    }
    let mut child = Table::new();
    for (name, value_) in values {
        match value_ {
            ConfigValue::Literal(literal) => child.insert(name, value(literal.clone())),
            ConfigValue::Secret { secret } => {
                let mut reference = InlineTable::new();
                reference.insert("secret", secret.clone().into());
                child.insert(name, Item::Value(Value::InlineTable(reference)))
            }
        };
    }
    table.insert(key, Item::Table(child));
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;

    fn test_paths(temp: &TempDir) -> Paths {
        let home = temp.path().join("home");
        Paths {
            config: temp.path().join("config/mcpd/config.toml"),
            state_dir: temp.path().join("state/mcpd"),
            codex_config: home.join(".codex/config.toml"),
            home,
        }
    }

    fn ownership(paths: &Paths, names: &[&str]) -> state::StateFile {
        let managed = names
            .iter()
            .map(|name| {
                (
                    (*name).to_owned(),
                    state::ManagedServer {
                        canonical_hash: format!("canonical-{name}"),
                        rendered_hash: format!("rendered-{name}"),
                    },
                )
            })
            .collect();
        state::StateFile {
            version: 1,
            targets: BTreeMap::from([(
                "codex".into(),
                state::TargetState {
                    config_path: paths.codex_config.clone(),
                    adapter_version: 1,
                    last_success_unix_ms: 1,
                    managed,
                },
            )]),
        }
    }

    #[test]
    fn rejects_unknown_version() {
        let error = parse("version = 2", Path::new("test.toml")).unwrap_err();
        assert!(error.to_string().contains("unsupported schema version 2"));
    }

    #[test]
    fn rejects_fields_from_wrong_transport() {
        let error = parse(
            "version=1\n[servers.x]\ntransport='http'\nurl='https://example.com'\ncommand='bad'",
            Path::new("test.toml"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("not a valid mcpd configuration"));
    }

    #[test]
    fn validates_server_ids() {
        for valid in ["context7", "Context_7", "github.com", "server-name"] {
            assert!(is_valid_server_id(valid), "expected `{valid}` to be valid");
        }
        for invalid in ["", " context", "context 7", "/context", "../context", "a/b"] {
            assert!(
                !is_valid_server_id(invalid),
                "expected `{invalid}` to be invalid"
            );
        }
    }

    #[test]
    fn parses_structured_secret_references_and_rejects_sensitive_literals() {
        let config = parse(
            "version=1\n[servers.x]\ntransport='stdio'\ncommand='x'\n[servers.x.env]\nAPI_TOKEN={secret='x.token'}\n[targets]\n",
            Path::new("test.toml"),
        )
        .unwrap();
        let Server::Stdio { env, .. } = &config.servers["x"] else {
            panic!("expected stdio server")
        };
        assert_eq!(env["API_TOKEN"].secret_name(), Some("x.token"));

        let error = parse(
            "version=1\n[servers.x]\ntransport='stdio'\ncommand='x'\n[servers.x.env]\nAPI_TOKEN='literal-value'\n[targets]\n",
            Path::new("test.toml"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("looks secret-bearing"));
        assert!(!error.to_string().contains("literal-value"));
    }

    #[test]
    fn secret_detection_is_conservative() {
        for sensitive in [
            "API_KEY",
            "DOKPLOY_API_KEY",
            "GITHUB_TOKEN",
            "DB_PASSWORD",
            "CLIENT_SECRET",
            "AWS_CREDENTIAL",
            "SSH_PRIVATE_KEY",
            "Authorization",
        ] {
            assert!(
                is_sensitive_field(sensitive),
                "expected {sensitive} to be sensitive"
            );
        }
        for literal in [
            "DOKPLOY_URL",
            "API_URL",
            "HOST",
            "PORT",
            "NODE_ENV",
            "TOKEN_ENDPOINT",
            "PASSWORD_POLICY",
        ] {
            assert!(
                !is_sensitive_field(literal),
                "expected {literal} to remain literal"
            );
        }
    }

    #[test]
    fn init_refuses_to_replace_missing_canonical_state_with_an_empty_config() {
        let temp = TempDir::new().unwrap();
        let paths = test_paths(&temp);
        let old_state = ownership(&paths, &["bing-search", "dokploy", "github"]);
        state::save(&paths.state_dir.join("state.toml"), &old_state).unwrap();
        let state_before = fs::read(paths.state_dir.join("state.toml")).unwrap();

        let error = init(&paths).unwrap_err();

        assert!(error.to_string().contains("refusing to initialize"));
        for name in ["bing-search", "dokploy", "github"] {
            assert!(error.to_string().contains(name));
        }
        assert!(!paths.config.exists());
        assert_eq!(
            fs::read(paths.state_dir.join("state.toml")).unwrap(),
            state_before
        );
    }

    #[test]
    fn schema_update_backs_up_and_preserves_servers_and_ownership() {
        let temp = TempDir::new().unwrap();
        let paths = test_paths(&temp);
        let before = br#"version = 1

[servers.bing-search]
transport = "stdio"
command = "bing"

[servers.dokploy]
transport = "http"
url = "https://dokploy.example/mcp"

[servers.github]
transport = "stdio"
command = "github"

[targets.codex]
enabled = true
"#;
        let rendered = br#"version = 1

[servers.bing-search]
transport = "stdio"
command = "bing"

[servers.dokploy]
transport = "http"
url = "https://dokploy.example/mcp"

[servers.github]
transport = "stdio"
command = "github"

[targets.codex]
enabled = true
client_version = "1.0"
"#;
        atomic_write(&paths.config, before, Some(0o640)).unwrap();
        let old_state = ownership(&paths, &["bing-search", "dokploy", "github"]);
        state::save(&paths.state_dir.join("state.toml"), &old_state).unwrap();
        let state_before = fs::read(paths.state_dir.join("state.toml")).unwrap();

        let backup = apply_schema_update(&paths, before, rendered).unwrap();

        assert_eq!(fs::read(&paths.config).unwrap(), rendered);
        assert_eq!(fs::read(&backup).unwrap(), before);
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&paths.config).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(
            fs::read(paths.state_dir.join("state.toml")).unwrap(),
            state_before
        );
        let updated = load(&paths.config).unwrap();
        assert_eq!(
            updated.servers.keys().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "bing-search".to_owned(),
                "dokploy".to_owned(),
                "github".to_owned(),
            ])
        );
    }

    #[test]
    fn schema_update_rejects_any_canonical_or_owned_server_loss() {
        let temp = TempDir::new().unwrap();
        let paths = test_paths(&temp);
        let before = br#"version = 1
[servers.github]
transport = "stdio"
command = "github"
[targets.codex]
enabled = true
"#;
        let unsafe_rendered = b"version = 1\n[servers]\n[targets.codex]\nenabled = true\n";
        atomic_write(&paths.config, before, Some(0o600)).unwrap();
        state::save(
            &paths.state_dir.join("state.toml"),
            &ownership(&paths, &["github"]),
        )
        .unwrap();
        let state_before = fs::read(paths.state_dir.join("state.toml")).unwrap();

        let error = apply_schema_update(&paths, before, unsafe_rendered).unwrap_err();

        assert!(error.to_string().contains("would discard"));
        assert!(error.to_string().contains("github"));
        assert_eq!(fs::read(&paths.config).unwrap(), before);
        assert_eq!(
            fs::read(paths.state_dir.join("state.toml")).unwrap(),
            state_before
        );
        assert!(!paths.state_dir.join("backups/canonical").exists());
    }
}
