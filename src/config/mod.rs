use std::{collections::BTreeMap, fs, path::Path};

use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value, value};

use crate::{
    diagnostics::{McpdError, Result},
    model::{CanonicalConfig, ConfigValue, Server},
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
    let config: CanonicalConfig = toml::from_str(text).map_err(|_| McpdError::InvalidInput {
        message: format!("{} is not a valid mcpd configuration", path.display()),
        hint: "fix the reported field or run `mcpd init` if this is a new configuration".into(),
    })?;
    validate(&config, path)?;
    Ok(config)
}

fn validate(config: &CanonicalConfig, path: &Path) -> Result<()> {
    if config.version != 1 {
        return Err(McpdError::InvalidInput {
            message: format!(
                "{} uses unsupported schema version {}",
                path.display(),
                config.version
            ),
            hint: "mcpd 0.1 supports only `version = 1` and never migrates implicitly".into(),
        });
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

pub fn init(path: &Path) -> Result<()> {
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
    atomic_write(path, EMPTY_CONFIG.as_bytes(), Some(0o600))
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
    use super::*;

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
}
