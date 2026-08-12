use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use crate::{
    diagnostics::{McpdError, Result},
    model::{CanonicalConfig, ConfigValue, Server},
    state::{ManagedServer, TargetState},
    sync::fs::ensure_safe_target_path,
    targets::{
        Change, ChangeKind, HttpSecretCapability, ImportSecretCandidate, ImportedServer,
        SecretCapabilities, StdioSecretCapability, TargetAdapter, TargetImport, TargetInventory,
        TargetPlan,
    },
};

pub const ADAPTER_VERSION: u32 = 1;

pub struct CodexAdapter {
    path: PathBuf,
    home: PathBuf,
}

impl CodexAdapter {
    pub fn new(path: PathBuf, home: PathBuf) -> Self {
        Self { path, home }
    }
}

impl TargetAdapter for CodexAdapter {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn adapter_version(&self) -> u32 {
        ADAPTER_VERSION
    }
    fn detect(&self) -> bool {
        self.path.exists() || executable_on_path("codex")
    }
    fn config_path(&self) -> &Path {
        &self.path
    }
    fn secret_capabilities(&self) -> SecretCapabilities {
        SecretCapabilities {
            stdio: StdioSecretCapability::RuntimeInjection,
            http: HttpSecretCapability::EnvironmentReference,
        }
    }

    fn server_names(&self) -> Result<Vec<String>> {
        let Some((_, document)) = self.read_document(true)? else {
            return Ok(Vec::new());
        };
        Ok(current_servers(&document)?.into_keys().collect())
    }

    fn import_secret_candidates(
        &self,
        selection: Option<&BTreeSet<String>>,
    ) -> Result<Vec<ImportSecretCandidate>> {
        let Some((_, document)) = self.read_document(false)? else {
            return Ok(Vec::new());
        };
        let mut candidates = Vec::new();
        for (name, item) in current_servers(&document)? {
            if selection.is_some_and(|selection| !selection.contains(&name)) {
                continue;
            }
            let value = item_value(&item)?;
            let Some(table) = value.as_table() else {
                continue;
            };
            for native_field in ["env", "http_headers"] {
                let Some(values) = table.get(native_field).and_then(toml::Value::as_table) else {
                    continue;
                };
                for field in values.keys() {
                    candidates.push(ImportSecretCandidate {
                        server: name.clone(),
                        field: field.clone(),
                        sensitive: crate::config::is_sensitive_field(field),
                    });
                }
            }
        }
        Ok(candidates)
    }

    fn import(
        &self,
        selection: Option<&BTreeSet<String>>,
        secret_mappings: &BTreeMap<String, String>,
    ) -> Result<TargetImport> {
        let Some((snapshot, document)) = self.read_document(false)? else {
            return Err(McpdError::TargetUnavailable {
                target: self.id().into(),
                message: format!("configuration {} does not exist", self.path.display()),
                hint: "configure at least one Codex MCP server before importing".into(),
            });
        };
        let current = current_servers(&document)?;
        if let Some(selection) = selection {
            if let Some(missing) = selection.iter().find(|name| !current.contains_key(*name)) {
                return Err(McpdError::InvalidInput {
                    message: format!("Codex MCP server `{missing}` does not exist"),
                    hint: "run `mcpd diff --all` to list unmanaged Codex servers".into(),
                });
            }
        }

        let mut servers = BTreeMap::new();
        let mut secret_values = BTreeMap::<String, String>::new();
        for (name, item) in current {
            if selection.is_some_and(|selection| !selection.contains(&name)) {
                continue;
            }
            if !crate::config::is_valid_server_id(&name) {
                return Err(McpdError::InvalidInput {
                    message: format!("Codex MCP server `{name}` is not a valid canonical server ID"),
                    hint: "rename the target entry to match [a-zA-Z0-9][a-zA-Z0-9._-]* before importing".into(),
                });
            }
            let (server, migrations) = import_server(&name, &item, secret_mappings)?;
            let expected = render_server(&name, &server)?;
            let rendered_hash = hash_item(&expected)?;
            let current_hash = hash_item(&item)?;
            if migrations.is_empty() && rendered_hash != current_hash {
                return Err(McpdError::InvalidInput {
                    message: format!("Codex MCP server `{name}` cannot be imported losslessly"),
                    hint: "remove unsupported or redundant native fields, then retry the import"
                        .into(),
                });
            }
            for (secret_name, secret_value) in migrations {
                if secret_values
                    .get(&secret_name)
                    .is_some_and(|existing| existing != &secret_value)
                {
                    return Err(McpdError::Conflict {
                        message: format!(
                            "multiple imported values were mapped to secret `{secret_name}`"
                        ),
                        hint: "use distinct keyring names for fields with different values".into(),
                    });
                }
                secret_values.insert(secret_name, secret_value);
            }
            servers.insert(
                name,
                ImportedServer {
                    ownership: ManagedServer {
                        canonical_hash: hash_serializable(&server)?,
                        rendered_hash: current_hash,
                    },
                    server,
                },
            );
        }
        Ok(TargetImport {
            target: self.id(),
            path: self.path.clone(),
            snapshot,
            servers,
            secret_writes: secret_values
                .into_iter()
                .map(|(name, value)| crate::secrets::SecretWrite {
                    name,
                    value: crate::secrets::SecretValue::new(value),
                })
                .collect(),
        })
    }

    fn plan(
        &self,
        desired: &CanonicalConfig,
        previous: Option<&TargetState>,
    ) -> Result<TargetPlan> {
        ensure_safe_target_path(&self.path, &self.home)?;
        let before = match fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => return Err(McpdError::io(&self.path, source)),
        };
        let source = before
            .as_deref()
            .map(std::str::from_utf8)
            .transpose()
            .map_err(|_| McpdError::InvalidInput {
                message: format!("Codex config {} is not UTF-8", self.path.display()),
                hint: "restore a valid UTF-8 TOML file before syncing".into(),
            })?
            .unwrap_or("");
        let mut doc = source
            .parse::<DocumentMut>()
            .map_err(|_| McpdError::InvalidInput {
                message: format!("Codex config {} is malformed TOML", self.path.display()),
                hint: "repair the file; mcpd never overwrites malformed target configuration"
                    .into(),
            })?;

        let owned = previous
            .filter(|state| {
                state.config_path == self.path && state.adapter_version == ADAPTER_VERSION
            })
            .map(|state| &state.managed)
            .cloned()
            .unwrap_or_default();
        let desired_servers = desired_for_codex(desired)?;
        let current = current_servers(&doc)?;
        let mut changes = Vec::new();
        let mut inventory = TargetInventory::default();
        let mut managed = BTreeMap::new();

        for (name, server) in &desired_servers {
            let expected = render_server(name, server)?;
            let rendered_hash = hash_item(&expected)?;
            let canonical_hash = hash_serializable(server)?;
            match current.get(name) {
                Some(_) if !owned.contains_key(name) => return Err(McpdError::Conflict {
                    message: format!("Codex already has unmanaged MCP server `{name}`"),
                    hint: "rename the canonical server or remove/import the existing Codex entry explicitly".into(),
                }),
                Some(item) => {
                    let current_hash = hash_item(item)?;
                    if current_hash != rendered_hash {
                        let kind = if owned.get(name).is_some_and(|record| record.rendered_hash != current_hash) {
                            ChangeKind::DriftRepair
                        } else { ChangeKind::Update };
                        let change = Change { server: name.clone(), kind };
                        inventory.managed_drift.push(change.clone());
                        changes.push(change);
                        set_server(&mut doc, name, expected)?;
                    } else {
                        inventory.managed_synchronized.push(name.clone());
                    }
                }
                None => {
                    let change = Change {
                        server: name.clone(),
                        kind: ChangeKind::Add,
                    };
                    if owned.contains_key(name) {
                        inventory.managed_drift.push(change.clone());
                    }
                    changes.push(change);
                    set_server(&mut doc, name, expected)?;
                }
            }
            managed.insert(
                name.clone(),
                ManagedServer {
                    canonical_hash,
                    rendered_hash,
                },
            );
        }

        for name in owned
            .keys()
            .filter(|name| !desired_servers.contains_key(*name))
        {
            if current.contains_key(name) {
                let change = Change {
                    server: name.clone(),
                    kind: ChangeKind::Remove,
                };
                inventory.managed_drift.push(change.clone());
                changes.push(change);
                remove_server(&mut doc, name)?;
            }
        }

        inventory.only_in_target = current
            .keys()
            .filter(|name| !owned.contains_key(*name) && !desired.servers.contains_key(*name))
            .cloned()
            .collect();
        inventory.only_in_mcpd = desired
            .servers
            .keys()
            .filter(|name| !current.contains_key(*name) && !owned.contains_key(*name))
            .cloned()
            .collect();

        let rendered = if changes.is_empty() {
            before.clone().unwrap_or_default()
        } else {
            doc.to_string().into_bytes()
        };
        let state_changed = previous.is_some_and(|state| {
            state.config_path != self.path
                || state.adapter_version != ADAPTER_VERSION
                || state.managed != managed
        });
        Ok(TargetPlan {
            target: self.id(),
            path: self.path.clone(),
            before,
            rendered,
            changes,
            inventory,
            next_state: TargetState {
                config_path: self.path.clone(),
                adapter_version: ADAPTER_VERSION,
                last_success_unix_ms: 0,
                managed,
            },
            state_changed,
        })
    }
}

impl CodexAdapter {
    fn read_document(&self, _allow_missing: bool) -> Result<Option<(Vec<u8>, DocumentMut)>> {
        ensure_safe_target_path(&self.path, &self.home)?;
        let snapshot = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(source) => return Err(McpdError::io(&self.path, source)),
        };
        let source = std::str::from_utf8(&snapshot).map_err(|_| McpdError::InvalidInput {
            message: format!("Codex config {} is not UTF-8", self.path.display()),
            hint: "restore a valid UTF-8 TOML file before importing".into(),
        })?;
        let document = source
            .parse::<DocumentMut>()
            .map_err(|_| McpdError::InvalidInput {
                message: format!("Codex config {} is malformed TOML", self.path.display()),
                hint: "repair the file; mcpd never overwrites malformed target configuration"
                    .into(),
            })?;
        Ok(Some((snapshot, document)))
    }
}

fn import_server(
    name: &str,
    item: &Item,
    secret_mappings: &BTreeMap<String, String>,
) -> Result<(Server, Vec<(String, String)>)> {
    let value = item_value(item)?;
    let table = value.as_table().ok_or_else(|| McpdError::InvalidInput {
        message: format!("Codex MCP server `{name}` must be a TOML table"),
        hint: "repair the Codex MCP entry before importing it".into(),
    })?;
    let has_command = table.contains_key("command");
    let has_url = table.contains_key("url");
    if has_command == has_url {
        return Err(McpdError::InvalidInput {
            message: format!(
                "Codex MCP server `{name}` must define exactly one of `command` or `url`"
            ),
            hint: "repair the native entry before importing it".into(),
        });
    }

    if has_command {
        reject_unknown_fields(name, table, &["command", "args", "cwd", "env", "env_vars"])?;
        let command = required_string(name, table, "command")?;
        if command.trim().is_empty() {
            return Err(McpdError::InvalidInput {
                message: format!("Codex MCP server `{name}` has an empty command"),
                hint: "set a command before importing it".into(),
            });
        }
        let args = optional_string_array(name, table, "args")?;
        let cwd = optional_string(name, table, "cwd")?.map(PathBuf::from);
        let imported = import_literal_values(name, table, "env", secret_mappings)?;
        let mut env = imported.values;
        for variable in optional_string_array(name, table, "env_vars")? {
            if variable.is_empty() || variable.chars().any(char::is_whitespace) {
                return Err(McpdError::InvalidInput {
                    message: format!(
                        "Codex MCP server `{name}` has invalid forwarded environment variable `{variable}`"
                    ),
                    hint: "use a non-empty environment variable name without whitespace".into(),
                });
            }
            env.insert(
                variable.clone(),
                ConfigValue::literal(format!("${{env:{variable}}}")),
            );
        }
        Ok((
            Server::Stdio {
                command,
                args,
                env,
                secrets: BTreeMap::new(),
                cwd,
            },
            imported.migrations,
        ))
    } else {
        reject_unknown_fields(name, table, &["url", "http_headers", "env_http_headers"])?;
        let url_text = required_string(name, table, "url")?;
        let url = url::Url::parse(&url_text).map_err(|error| McpdError::InvalidInput {
            message: format!("Codex MCP server `{name}` has an invalid URL: {error}"),
            hint: "use an absolute http:// or https:// URL before importing".into(),
        })?;
        let imported = import_literal_values(name, table, "http_headers", secret_mappings)?;
        let mut headers = imported.values;
        if let Some(values) = table.get("env_http_headers") {
            let values = values.as_table().ok_or_else(|| McpdError::InvalidInput {
                message: format!(
                    "Codex MCP server `{name}` field `env_http_headers` must be a table"
                ),
                hint: "map each HTTP header name to an environment variable name".into(),
            })?;
            for (header, source) in values {
                let source = source.as_str().ok_or_else(|| McpdError::InvalidInput {
                    message: format!(
                        "Codex MCP server `{name}` environment header `{header}` must be a string"
                    ),
                    hint: "set the value to an environment variable name".into(),
                })?;
                if source.is_empty() || source.chars().any(char::is_whitespace) {
                    return Err(McpdError::InvalidInput {
                        message: format!(
                            "Codex MCP server `{name}` has invalid environment variable `{source}`"
                        ),
                        hint: "use a non-empty environment variable name without whitespace".into(),
                    });
                }
                headers.insert(
                    header.clone(),
                    ConfigValue::literal(format!("${{env:{source}}}")),
                );
            }
        }
        Ok((
            Server::Http {
                url,
                headers,
                secrets: BTreeMap::new(),
            },
            imported.migrations,
        ))
    }
}

fn item_value(item: &Item) -> Result<toml::Value> {
    let mut document = DocumentMut::new();
    document.insert("server", item.clone());
    let root: toml::Value =
        toml::from_str(&document.to_string()).map_err(|_| McpdError::InvalidInput {
            message: "could not parse native Codex MCP entry".into(),
            hint: "repair the Codex MCP entry before importing it".into(),
        })?;
    root.get("server")
        .cloned()
        .ok_or_else(|| McpdError::Operational {
            message: "normalized Codex MCP entry disappeared".into(),
            hint: "report this as an mcpd bug".into(),
        })
}

fn reject_unknown_fields(
    name: &str,
    table: &toml::map::Map<String, toml::Value>,
    allowed: &[&str],
) -> Result<()> {
    if let Some(field) = table
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(McpdError::InvalidInput {
            message: format!(
                "Codex MCP server `{name}` uses unsupported field `{field}` and cannot be imported losslessly"
            ),
            hint: "remove the unsupported native field or keep this server unmanaged".into(),
        });
    }
    Ok(())
}

struct ImportedValues {
    values: BTreeMap<String, ConfigValue>,
    migrations: Vec<(String, String)>,
}

fn import_literal_values(
    name: &str,
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
    secret_mappings: &BTreeMap<String, String>,
) -> Result<ImportedValues> {
    let Some(value) = table.get(field) else {
        return Ok(ImportedValues {
            values: BTreeMap::new(),
            migrations: Vec::new(),
        });
    };
    let values = value.as_table().ok_or_else(|| McpdError::InvalidInput {
        message: format!("Codex MCP server `{name}` field `{field}` must be a table"),
        hint: "repair the native entry before importing it".into(),
    })?;
    let mut imported = BTreeMap::new();
    let mut migrations = Vec::new();
    for (key, value) in values {
        let value = value.as_str().ok_or_else(|| McpdError::InvalidInput {
            message: format!("Codex MCP server `{name}` field `{field}.{key}` must be a string"),
            hint: "repair the native entry before importing it".into(),
        })?;
        let scoped_key = mapping_key(name, key);
        if let Some(secret_name) = secret_mappings
            .get(&scoped_key)
            .or_else(|| secret_mappings.get(key))
        {
            crate::secrets::validate_name(secret_name)?;
            imported.insert(key.clone(), ConfigValue::secret(secret_name));
            migrations.push((secret_name.clone(), value.to_owned()));
        } else if crate::config::is_sensitive_field(key) {
            return Err(McpdError::InvalidInput {
                message: format!(
                    "Codex MCP server `{name}` field `{key}` looks secret-bearing and requires explicit migration"
                ),
                hint: format!("use `--secret {key}=SECRET_NAME` or run interactively"),
            });
        } else {
            imported.insert(key.clone(), ConfigValue::literal(value));
        }
    }
    Ok(ImportedValues {
        values: imported,
        migrations,
    })
}

fn mapping_key(server: &str, field: &str) -> String {
    format!("{server}\0{field}")
}

fn required_string(
    name: &str,
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
) -> Result<String> {
    table
        .get(field)
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| McpdError::InvalidInput {
            message: format!("Codex MCP server `{name}` field `{field}` must be a string"),
            hint: "repair the native entry before importing it".into(),
        })
}

fn optional_string(
    name: &str,
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
) -> Result<Option<String>> {
    table
        .get(field)
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| McpdError::InvalidInput {
                    message: format!("Codex MCP server `{name}` field `{field}` must be a string"),
                    hint: "repair the native entry before importing it".into(),
                })
        })
        .transpose()
}

fn optional_string_array(
    name: &str,
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
) -> Result<Vec<String>> {
    let Some(value) = table.get(field) else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or_else(|| McpdError::InvalidInput {
        message: format!("Codex MCP server `{name}` field `{field}` must be an array"),
        hint: "repair the native entry before importing it".into(),
    })?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| McpdError::InvalidInput {
                    message: format!(
                        "Codex MCP server `{name}` field `{field}` must contain only strings"
                    ),
                    hint: "repair the native entry before importing it".into(),
                })
        })
        .collect()
}

fn desired_for_codex(config: &CanonicalConfig) -> Result<BTreeMap<String, Server>> {
    let target = config.targets.get("codex");
    if !target.is_some_and(|target| target.enabled) {
        return Ok(BTreeMap::new());
    }
    let mut result = BTreeMap::new();
    for (name, server) in &config.servers {
        if target
            .and_then(|target| target.servers.get(name))
            .is_some_and(|server| !server.enabled)
        {
            continue;
        }
        result.insert(name.clone(), server.clone());
    }
    Ok(result)
}

fn render_server(name: &str, server: &Server) -> Result<Item> {
    let mut table = Table::new();
    match server {
        Server::Stdio {
            command,
            args,
            env,
            cwd,
            secrets,
        } => {
            if env.values().any(|value| value.secret_name().is_some()) || !secrets.is_empty() {
                table.insert("command", value("mcpd"));
                let mut wrapper_args = Array::new();
                wrapper_args.push("exec");
                wrapper_args.push(name);
                table.insert("args", Item::Value(Value::Array(wrapper_args)));
                return Ok(Item::Table(table));
            }
            table.insert("command", value(command.clone()));
            if !args.is_empty() {
                let mut array = Array::new();
                for arg in args {
                    array.push(arg.as_str());
                }
                table.insert("args", Item::Value(Value::Array(array)));
            }
            let mut static_env = BTreeMap::new();
            let mut forwarded_env = Vec::new();
            for (key, value_) in env {
                let ConfigValue::Literal(value_) = value_ else {
                    unreachable!("secret references use the mcpd exec wrapper")
                };
                if let Some(source) = env_reference(value_)? {
                    if source != key {
                        return Err(McpdError::InvalidInput {
                            message: format!("Codex cannot map environment `{source}` to `{key}` for server `{name}`"),
                            hint: "use the same source and destination name, or provide a non-secret literal value".into(),
                        });
                    }
                    forwarded_env.push(source);
                } else {
                    static_env.insert(key, value_);
                }
            }
            if !static_env.is_empty() {
                let mut env_table = Table::new();
                for (name, value_) in static_env {
                    env_table.insert(name, value(value_.clone()));
                }
                table.insert("env", Item::Table(env_table));
            }
            if !forwarded_env.is_empty() {
                let mut array = Array::new();
                for variable in forwarded_env {
                    array.push(variable);
                }
                table.insert("env_vars", Item::Value(Value::Array(array)));
            }
            if let Some(cwd) = cwd {
                table.insert("cwd", value(cwd.to_string_lossy().to_string()));
            }
        }
        Server::Http {
            url,
            headers,
            secrets,
        } => {
            if let Some(secret) = secrets.values().next() {
                return Err(McpdError::InvalidInput {
                    message: format!(
                        "Codex cannot resolve OS-keyring secret `{secret}` for HTTP server `{name}`"
                    ),
                    hint: "use an environment reference supported by Codex; mcpd does not proxy HTTP MCP traffic"
                        .into(),
                });
            }
            table.insert("url", value(url.as_str()));
            let mut static_headers = BTreeMap::new();
            let mut forwarded_headers = BTreeMap::new();
            for (header, value_) in headers {
                if let Some(secret) = value_.secret_name() {
                    return Err(McpdError::InvalidInput {
                        message: format!(
                            "Codex cannot resolve OS-keyring secret `{secret}` for HTTP server `{name}`"
                        ),
                        hint: "use an environment reference supported by Codex; mcpd does not proxy HTTP MCP traffic"
                            .into(),
                    });
                }
                let ConfigValue::Literal(value_) = value_ else {
                    unreachable!("structured secret reference handled above")
                };
                if let Some(source) = env_reference(value_)? {
                    forwarded_headers.insert(header, source);
                } else {
                    static_headers.insert(header, value_);
                }
            }
            if !static_headers.is_empty() {
                let mut header_table = Table::new();
                for (name, value_) in static_headers {
                    header_table.insert(name, value(value_.clone()));
                }
                table.insert("http_headers", Item::Table(header_table));
            }
            if !forwarded_headers.is_empty() {
                let mut header_table = Table::new();
                for (header, source) in forwarded_headers {
                    header_table.insert(header, value(source));
                }
                table.insert("env_http_headers", Item::Table(header_table));
            }
        }
    }
    Ok(Item::Table(table))
}

fn env_reference(value: &str) -> Result<Option<&str>> {
    if let Some(inner) = value
        .strip_prefix("${env:")
        .and_then(|value| value.strip_suffix('}'))
    {
        if !inner.is_empty() && !inner.chars().any(char::is_whitespace) {
            return Ok(Some(inner));
        }
    }
    if value.contains("${env:") {
        return Err(McpdError::InvalidInput {
            message: format!("invalid or embedded environment reference `{value}`"),
            hint: "use the complete form `${env:NAME}` as the entire value".into(),
        });
    }
    Ok(None)
}

fn current_servers(doc: &DocumentMut) -> Result<BTreeMap<String, Item>> {
    let Some(item) = doc.get("mcp_servers") else {
        return Ok(BTreeMap::new());
    };
    let table = item.as_table().ok_or_else(|| McpdError::InvalidInput {
        message: "Codex `mcp_servers` must be a TOML table".into(),
        hint: "repair the Codex configuration before syncing".into(),
    })?;
    Ok(table
        .iter()
        .map(|(name, item)| (name.to_owned(), item.clone()))
        .collect())
}

fn servers_table(doc: &mut DocumentMut) -> Result<&mut Table> {
    if !doc.contains_key("mcp_servers") {
        doc.insert("mcp_servers", Item::Table(Table::new()));
    }
    doc["mcp_servers"]
        .as_table_mut()
        .ok_or_else(|| McpdError::InvalidInput {
            message: "Codex `mcp_servers` must be a TOML table".into(),
            hint: "repair the Codex configuration before syncing".into(),
        })
}

fn set_server(doc: &mut DocumentMut, name: &str, item: Item) -> Result<()> {
    servers_table(doc)?.insert(name, item);
    Ok(())
}

fn remove_server(doc: &mut DocumentMut, name: &str) -> Result<()> {
    servers_table(doc)?.remove(name);
    Ok(())
}

fn hash_item(item: &Item) -> Result<String> {
    let mut document = DocumentMut::new();
    document.insert("server", item.clone());
    let value: toml::Value =
        toml::from_str(&document.to_string()).map_err(|error| McpdError::Operational {
            message: format!("could not normalize rendered Codex entry: {error}"),
            hint: "report this as an mcpd bug".into(),
        })?;
    hash_serializable(&value)
}

fn hash_serializable(value: &impl serde::Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|error| McpdError::Operational {
        message: format!("could not normalize configuration: {error}"),
        hint: "report this as an mcpd bug".into(),
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn executable_on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|path| path.join(name).is_file()))
}
