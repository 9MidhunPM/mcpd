use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

use crate::{
    Paths,
    diagnostics::{McpdError, Result},
    model::{CanonicalConfig, ConfigValue, Server},
    state::{ManagedServer, TargetState},
    sync::fs::{ensure_safe_target_path, hash_bytes},
    targets::{
        Change, ChangeKind, HttpSecretCapability, ImportMode, ImportSecretCandidate, ImportSkipped,
        ImportedServer, SecretCapabilities, StdioSecretCapability, TargetAdapter, TargetImport,
        TargetInventory, TargetPlan,
    },
};

const VERSION: u32 = 1;

struct Profile {
    id: &'static str,
    command: &'static str,
    servers_path: Vec<String>,
    open_code: bool,
    remote_url: &'static str,
    env: &'static str,
    env_reference: Option<fn(&str) -> String>,
    explicit_type: bool,
}

pub struct JsonAdapter {
    profile: Profile,
    path: PathBuf,
    home: PathBuf,
}

impl JsonAdapter {
    pub fn new(id: &str, paths: &Paths) -> Result<Self> {
        let profile = match id {
            "claude" => Profile {
                id: "claude",
                command: "claude",
                servers_path: vec!["mcpServers".into()],
                open_code: false,
                remote_url: "url",
                env: "env",
                env_reference: Some(|name| format!("${{{name}}}")),
                explicit_type: true,
            },
            "cursor" => Profile {
                id: "cursor",
                command: "cursor",
                servers_path: vec!["mcpServers".into()],
                open_code: false,
                remote_url: "url",
                env: "env",
                env_reference: Some(|name| format!("${{env:{name}}}")),
                explicit_type: false,
            },
            "antigravity" => Profile {
                id: "antigravity",
                command: "agy",
                servers_path: vec!["mcpServers".into()],
                open_code: false,
                remote_url: "serverUrl",
                env: "env",
                env_reference: None,
                explicit_type: false,
            },
            "openchamber" => Profile {
                id: "openchamber",
                command: "openchamber",
                servers_path: vec!["mcp".into(), "servers".into()],
                open_code: true,
                remote_url: "url",
                env: "environment",
                env_reference: Some(|name| format!("{{env:{name}}}")),
                explicit_type: true,
            },
            "claude-project" | "claude-local" => {
                let project = crate::resolve::current_project(paths)?.ok_or_else(|| {
                    McpdError::TargetUnavailable {
                        target: id.into(),
                        message: "no current project could be discovered".into(),
                        hint: "run inside a repository or set MCPD_PROJECT_ROOT".into(),
                    }
                })?;
                if !project.trusted {
                    return Err(McpdError::Security {
                        path: project.root,
                        message: format!("{id} requires an explicitly trusted project"),
                        hint: "run `mcpd trust` in the project before enabling this scope".into(),
                    });
                }
                let local = id == "claude-local";
                Profile {
                    id: if local {
                        "claude-local"
                    } else {
                        "claude-project"
                    },
                    command: "claude",
                    servers_path: if local {
                        vec![
                            "projects".into(),
                            project.root.to_string_lossy().into_owned(),
                            "mcpServers".into(),
                        ]
                    } else {
                        vec!["mcpServers".into()]
                    },
                    open_code: false,
                    remote_url: "url",
                    env: "env",
                    env_reference: Some(|name| format!("${{{name}}}")),
                    explicit_type: true,
                }
            }
            _ => {
                return Err(McpdError::TargetUnavailable {
                    target: id.into(),
                    message: "unknown JSON target".into(),
                    hint: "use a built-in target ID".into(),
                });
            }
        };
        let path = match id {
            "claude-project" => crate::resolve::current_project(paths)?
                .map(|project| project.root.join(".mcp.json")),
            "claude-local" => paths.target_config("claude"),
            "antigravity" => Some(antigravity_path(paths)?),
            _ => paths.target_config(id),
        }
        .ok_or_else(|| McpdError::TargetUnavailable {
            target: id.into(),
            message: "could not determine a target configuration path".into(),
            hint: "set the target-specific MCPD_*_CONFIG override".into(),
        })?;
        let config_root = paths
            .config
            .parent()
            .and_then(|path| path.parent())
            .unwrap_or(paths.home.as_path());
        let allowed_root = if id == "claude-project" {
            crate::resolve::current_project(paths)?
                .map(|project| project.root)
                .ok_or_else(|| McpdError::TargetUnavailable {
                    target: id.into(),
                    message: "no current project could be discovered".into(),
                    hint: "run inside a repository or set MCPD_PROJECT_ROOT".into(),
                })?
        } else if path.starts_with(&paths.home) {
            paths.home.clone()
        } else {
            config_root.to_path_buf()
        };
        Ok(Self {
            profile,
            path,
            home: allowed_root,
        })
    }

    fn read(&self, allow_missing: bool) -> Result<(Option<Vec<u8>>, Value)> {
        ensure_safe_target_path(&self.path, &self.home)?;
        match fs::read(&self.path) {
            Ok(bytes) => {
                let (_, value) = super::jsonc::parse(
                    &bytes,
                    crate::targets::display_name(self.id()),
                    &self.path,
                )?;
                if !value.is_object() {
                    return Err(self.bad("configuration root must be a JSON object"));
                }
                Ok((Some(bytes), value))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_missing => {
                Ok((None, json!({})))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(McpdError::TargetUnavailable {
                    target: self.id().into(),
                    message: format!("configuration {} does not exist", self.path.display()),
                    hint: format!(
                        "configure at least one {} MCP server before importing",
                        crate::targets::display_name(self.id())
                    ),
                })
            }
            Err(source) => Err(McpdError::io(&self.path, source)),
        }
    }

    fn bad(&self, message: impl Into<String>) -> McpdError {
        McpdError::InvalidInput {
            message: format!(
                "{}: {}",
                crate::targets::display_name(self.id()),
                message.into()
            ),
            hint: "repair the native MCP entry or keep it unmanaged".into(),
        }
    }

    fn servers<'a>(&self, doc: &'a Value) -> Result<Option<&'a Map<String, Value>>> {
        let value = self
            .profile
            .servers_path
            .iter()
            .try_fold(doc, |value, key| value.get(key));
        value
            .map(|value| {
                value
                    .as_object()
                    .ok_or_else(|| self.bad("MCP server collection must be an object"))
            })
            .transpose()
    }

    fn servers_mut<'a>(&self, doc: &'a mut Value) -> Result<&'a mut Map<String, Value>> {
        let root = doc
            .as_object_mut()
            .ok_or_else(|| self.bad("configuration root must be an object"))?;
        let mut current = root;
        for key in &self.profile.servers_path {
            current = current
                .entry(key)
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| self.bad(format!("`{}` must be an object", key)))?;
        }
        Ok(current)
    }

    fn render(&self, name: &str, server: &Server) -> Result<Value> {
        match server {
            Server::Stdio {
                command,
                args,
                env,
                secrets,
                cwd,
            } => {
                let requires_wrapper = !secrets.is_empty()
                    || env.values().any(|value| value.secret_name().is_some())
                    || (self.profile.env_reference.is_none()
                        && env.values().any(|value| {
                            matches!(value, ConfigValue::Literal(value) if env_reference(value).ok().flatten().is_some())
                        }));
                if requires_wrapper {
                    return Ok(if self.profile.open_code {
                        json!({"type":"local", "command":["mcpd", "exec", name]})
                    } else if self.profile.explicit_type {
                        json!({"type":"stdio", "command":"mcpd", "args":["exec", name]})
                    } else {
                        json!({"command":"mcpd", "args":["exec", name]})
                    });
                }
                let mut object = Map::new();
                if self.profile.open_code {
                    object.insert("type".into(), json!("local"));
                    let mut command_line = vec![command.clone()];
                    command_line.extend(args.clone());
                    object.insert("command".into(), json!(command_line));
                } else {
                    if self.profile.explicit_type {
                        object.insert("type".into(), json!("stdio"));
                    }
                    object.insert("command".into(), json!(command));
                    if !args.is_empty() {
                        object.insert("args".into(), json!(args));
                    }
                }
                if let Some(cwd) = cwd {
                    object.insert("cwd".into(), json!(cwd));
                }
                let mut native_env = Map::new();
                for (field, value) in env {
                    let ConfigValue::Literal(value) = value else {
                        return Err(McpdError::Operational {
                            message: format!(
                                "secret reference for `{name}` escaped runtime wrapping"
                            ),
                            hint: "report this as an mcpd bug".into(),
                        });
                    };
                    let rendered =
                        if let Some(source) = env_reference(value)? {
                            self.profile.env_reference.ok_or_else(|| McpdError::Operational {
                            message: format!(
                                "environment reference for `{name}` escaped runtime wrapping"
                            ),
                            hint: "report this as an mcpd bug".into(),
                        })?(source)
                        } else {
                            value.clone()
                        };
                    native_env.insert(field.clone(), json!(rendered));
                }
                if !native_env.is_empty() {
                    object.insert(self.profile.env.into(), Value::Object(native_env));
                }
                Ok(Value::Object(object))
            }
            Server::Http {
                url,
                headers,
                secrets,
            } => {
                if let Some(secret) = secrets
                    .values()
                    .next()
                    .map(String::as_str)
                    .or_else(|| headers.values().find_map(ConfigValue::secret_name))
                {
                    return Err(McpdError::InvalidInput {
                        message: format!(
                            "{} cannot safely inject keyring secret `{secret}` into HTTP server `{name}`",
                            crate::targets::display_name(self.id())
                        ),
                        hint:
                            "use a native environment reference; mcpd does not proxy HTTP traffic"
                                .into(),
                    });
                }
                let mut object = Map::new();
                if self.profile.explicit_type {
                    object.insert(
                        "type".into(),
                        json!(if self.profile.open_code {
                            "remote"
                        } else {
                            "http"
                        }),
                    );
                }
                object.insert(self.profile.remote_url.into(), json!(url));
                let mut native_headers = Map::new();
                for (header, value) in headers {
                    let ConfigValue::Literal(value) = value else {
                        return Err(McpdError::Operational {
                            message: format!("secret HTTP header for `{name}` escaped validation"),
                            hint: "report this as an mcpd bug".into(),
                        });
                    };
                    let rendered = if let Some(env) = env_reference(value)? {
                        self.profile.env_reference.ok_or_else(|| McpdError::InvalidInput { message: format!("{} cannot safely represent environment-backed HTTP header `{header}` for `{name}`", crate::targets::display_name(self.id())), hint: "keep this server disabled for the target or use client-native authentication".into() })?(env)
                    } else {
                        value.clone()
                    };
                    native_headers.insert(header.clone(), json!(rendered));
                }
                if !native_headers.is_empty() {
                    object.insert("headers".into(), Value::Object(native_headers));
                }
                if self.profile.open_code && !object.contains_key("oauth") {
                    object.insert("oauth".into(), json!(false));
                }
                Ok(Value::Object(object))
            }
        }
    }

    fn import_one(
        &self,
        name: &str,
        value: &Value,
        mappings: &BTreeMap<String, String>,
    ) -> Result<(Server, Vec<crate::secrets::SecretWrite>)> {
        let object = value
            .as_object()
            .ok_or_else(|| self.bad(format!("MCP server `{name}` must be an object")))?;
        let remote = object.contains_key(self.profile.remote_url);
        let allowed: &[&str] = if remote && self.profile.open_code {
            &[
                "type",
                self.profile.remote_url,
                "headers",
                "oauth",
                "disabled",
            ]
        } else if remote {
            &["type", self.profile.remote_url, "headers", "disabled"]
        } else {
            &[
                "type",
                "command",
                "args",
                "cwd",
                self.profile.env,
                "disabled",
            ]
        };
        if let Some(field) = object
            .keys()
            .find(|field| !allowed.contains(&field.as_str()))
        {
            return Err(self.bad(format!("MCP server `{name}` uses unsupported native field `{field}` and cannot be imported losslessly")));
        }
        if object.get("disabled").and_then(Value::as_bool) == Some(true) {
            return Err(self.bad(format!(
                "MCP server `{name}` is disabled natively and cannot be imported without changing behavior"
            )));
        }
        if self.profile.open_code
            && remote
            && object
                .get("oauth")
                .is_some_and(|value| value != &json!(false))
        {
            return Err(self.bad(format!(
                "MCP server `{name}` uses native OAuth settings that are not part of the canonical model"
            )));
        }
        if let Some(native_type) = object.get("type").and_then(Value::as_str) {
            let valid = if self.profile.open_code {
                native_type == if remote { "remote" } else { "local" }
            } else if remote {
                matches!(native_type, "http" | "streamable-http")
            } else {
                native_type == "stdio"
            };
            if !valid {
                return Err(self.bad(format!(
                    "MCP server `{name}` uses unsupported transport type `{native_type}`"
                )));
            }
        }
        let mut writes = Vec::new();
        if remote {
            let url = object
                .get(self.profile.remote_url)
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    self.bad(format!("MCP server `{name}` has an invalid remote URL"))
                })?;
            let url = url::Url::parse(url)
                .map_err(|_| self.bad(format!("MCP server `{name}` has an invalid remote URL")))?;
            let headers = import_values(
                &self.profile,
                name,
                object.get("headers"),
                mappings,
                &mut writes,
            )?;
            Ok((
                Server::Http {
                    url,
                    headers,
                    secrets: BTreeMap::new(),
                },
                writes,
            ))
        } else {
            let (command, args) = if self.profile.open_code {
                let values = object
                    .get("command")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        self.bad(format!("MCP server `{name}` command must be an array"))
                    })?;
                let values = string_array(values)
                    .map_err(|message| self.bad(format!("MCP server `{name}` {message}")))?;
                let (command, args) = values
                    .split_first()
                    .ok_or_else(|| self.bad(format!("MCP server `{name}` command is empty")))?;
                (command.clone(), args.to_vec())
            } else {
                let command = object
                    .get("command")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        self.bad(format!("MCP server `{name}` command must be a string"))
                    })?
                    .to_owned();
                let args = object
                    .get("args")
                    .map(|v| {
                        v.as_array()
                            .ok_or_else(|| {
                                self.bad(format!("MCP server `{name}` args must be an array"))
                            })
                            .and_then(|a| string_array(a).map_err(|m| self.bad(m)))
                    })
                    .transpose()?
                    .unwrap_or_default();
                (command, args)
            };
            let env = import_values(
                &self.profile,
                name,
                object.get(self.profile.env),
                mappings,
                &mut writes,
            )?;
            let cwd = object
                .get("cwd")
                .map(|v| {
                    v.as_str().map(PathBuf::from).ok_or_else(|| {
                        self.bad(format!("MCP server `{name}` cwd must be a string"))
                    })
                })
                .transpose()?;
            Ok((
                Server::Stdio {
                    command,
                    args,
                    env,
                    secrets: BTreeMap::new(),
                    cwd,
                },
                writes,
            ))
        }
    }
}

fn antigravity_path(paths: &Paths) -> Result<PathBuf> {
    if std::env::var_os("MCPD_ANTIGRAVITY_CONFIG").is_some() {
        return paths
            .target_config("antigravity")
            .ok_or_else(|| McpdError::TargetUnavailable {
                target: "antigravity".into(),
                message: "MCPD_ANTIGRAVITY_CONFIG did not resolve to a path".into(),
                hint: "set it to an absolute configuration file path".into(),
            });
    }
    let candidates = [
        paths.home.join(".gemini/config/mcp_config.json"),
        paths.home.join(".gemini/antigravity/mcp_config.json"),
        paths.home.join(".gemini/antigravity-cli/mcp_config.json"),
    ];
    let existing = candidates
        .iter()
        .filter(|path| path.exists())
        .cloned()
        .collect::<Vec<_>>();
    match existing.as_slice() {
        [] => Ok(candidates[0].clone()),
        [path] => Ok(path.clone()),
        _ => Err(McpdError::Conflict {
            message: format!(
                "multiple Antigravity MCP configuration paths exist: {}",
                existing
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            hint: "set MCPD_ANTIGRAVITY_CONFIG to the active client configuration explicitly"
                .into(),
        }),
    }
}

impl TargetAdapter for JsonAdapter {
    fn id(&self) -> &str {
        self.profile.id
    }
    fn adapter_version(&self) -> u32 {
        VERSION
    }
    fn detect(&self) -> bool {
        self.path.exists()
            || executable_on_path(self.profile.command)
            || match self.id() {
                "cursor" => executable_on_path("cursor-agent"),
                "antigravity" => executable_on_path("antigravity"),
                "openchamber" => executable_on_path("opencode") || executable_on_path("opencode2"),
                _ => false,
            }
    }
    fn config_path(&self) -> &Path {
        &self.path
    }
    fn secret_capabilities(&self) -> SecretCapabilities {
        SecretCapabilities {
            stdio: StdioSecretCapability::RuntimeInjection,
            http: if self.profile.env_reference.is_some() {
                HttpSecretCapability::EnvironmentReference
            } else {
                HttpSecretCapability::Unsupported
            },
        }
    }
    fn native_schema(&self) -> &'static str {
        match self.id() {
            "claude" | "claude-project" | "claude-local" => "claude-json/mcpServers",
            "cursor" => "cursor-jsonc/mcpServers",
            "antigravity" => "antigravity-json/mcpServers",
            "openchamber" => "opencode-json/mcp.servers",
            _ => "json/mcpServers",
        }
    }
    fn server_names(&self) -> Result<Vec<String>> {
        let (_, doc) = self.read(true)?;
        Ok(self
            .servers(&doc)?
            .map(|s| s.keys().cloned().collect())
            .unwrap_or_default())
    }
    fn import_secret_candidates(
        &self,
        selection: Option<&BTreeSet<String>>,
    ) -> Result<Vec<ImportSecretCandidate>> {
        let (_, doc) = self.read(false)?;
        let mut result = Vec::new();
        for (name, server) in self.servers(&doc)?.into_iter().flatten() {
            if selection.is_some_and(|set| !set.contains(name)) {
                continue;
            }
            let Some(object) = server.as_object() else {
                continue;
            };
            for field_name in [self.profile.env, "headers"] {
                if let Some(values) = object.get(field_name).and_then(Value::as_object) {
                    result.extend(values.iter().map(|(field, value)| ImportSecretCandidate {
                        server: name.clone(),
                        field: field.clone(),
                        sensitive: crate::config::is_sensitive_field(field)
                            && !value.as_str().is_some_and(|value| {
                                native_env_reference(&self.profile, value).is_some()
                            }),
                    }));
                }
            }
        }
        Ok(result)
    }
    fn import(
        &self,
        selection: Option<&BTreeSet<String>>,
        mappings: &BTreeMap<String, String>,
        mode: ImportMode,
    ) -> Result<TargetImport> {
        let (snapshot, doc) = self.read(false)?;
        let snapshot = snapshot.ok_or_else(|| McpdError::TargetUnavailable {
            target: self.id().into(),
            message: format!("configuration {} does not exist", self.path.display()),
            hint: "configure at least one native MCP server before importing".into(),
        })?;
        let current = self.servers(&doc)?.cloned().unwrap_or_default();
        if let Some(missing) =
            selection.and_then(|set| set.iter().find(|name| !current.contains_key(*name)))
        {
            return Err(self.bad(format!("MCP server `{missing}` does not exist")));
        }
        let mut servers = BTreeMap::new();
        let mut writes = Vec::new();
        let mut secret_values = BTreeMap::<String, String>::new();
        let mut skipped = Vec::new();
        for (name, value) in current {
            if selection.is_some_and(|set| !set.contains(&name)) {
                continue;
            }
            let imported =
                self.import_one(&name, &value, mappings)
                    .and_then(|(server, secret_writes)| {
                        let rendered = self.render(&name, &server)?;
                        if secret_writes.is_empty()
                            && normalize(&self.profile, &rendered)
                                != normalize(&self.profile, &value)
                        {
                            return Err(self.bad(format!(
                                "MCP server `{name}` cannot be imported losslessly"
                            )));
                        }
                        Ok((server, secret_writes))
                    });
            match imported {
                Ok((server, mut secret_writes)) => {
                    if let Some(conflict) = secret_writes.iter().find(|write| {
                        secret_values
                            .get(&write.name)
                            .is_some_and(|value| value != write.value.expose())
                    }) {
                        let error = McpdError::Conflict {
                            message: format!(
                                "multiple imported values were mapped to secret `{}`",
                                conflict.name
                            ),
                            hint: "use distinct keyring names for fields with different values"
                                .into(),
                        };
                        if mode == ImportMode::BestEffort {
                            skipped.push(ImportSkipped {
                                name,
                                reason: error.to_string(),
                            });
                            continue;
                        }
                        return Err(error);
                    }
                    let rendered = self.render(&name, &server)?;
                    servers.insert(
                        name,
                        ImportedServer {
                            ownership: ManagedServer {
                                canonical_hash: hash_serializable(&server)?,
                                rendered_hash: hash_json(&rendered)?,
                            },
                            server,
                        },
                    );
                    for write in &secret_writes {
                        secret_values.insert(write.name.clone(), write.value.expose().to_owned());
                    }
                    writes.append(&mut secret_writes);
                }
                Err(error) if mode == ImportMode::BestEffort => skipped.push(ImportSkipped {
                    name,
                    reason: error.to_string(),
                }),
                Err(error) => return Err(error),
            }
        }
        Ok(TargetImport {
            target: self.id().into(),
            path: self.path.clone(),
            snapshot,
            servers,
            skipped,
            secret_writes: writes,
        })
    }
    fn plan(
        &self,
        desired: &CanonicalConfig,
        previous: Option<&TargetState>,
    ) -> Result<TargetPlan> {
        let (before, mut doc) = self.read(true)?;
        let current = self.servers(&doc)?.cloned().unwrap_or_default();
        let owned = previous
            .filter(|s| s.config_path == self.path && s.adapter_version == VERSION)
            .map(|s| s.managed.clone())
            .unwrap_or_default();
        let target = desired.targets.get(self.id());
        let mut wanted = BTreeMap::new();
        if target.is_some_and(|t| t.enabled) {
            for (name, server) in &desired.servers {
                if !target
                    .and_then(|t| t.servers.get(name))
                    .is_some_and(|s| !s.enabled)
                {
                    wanted.insert(name.clone(), server);
                }
            }
        }
        let mut changes = Vec::new();
        let mut inventory = TargetInventory::default();
        let mut managed = BTreeMap::new();
        for (name, server) in wanted {
            let rendered = self.render(&name, server)?;
            let rendered_hash = hash_json(&rendered)?;
            match current.get(&name) {
                Some(_) if !owned.contains_key(&name) => return Err(McpdError::Conflict {
                    message: format!(
                        "{} already has unmanaged MCP server `{name}`",
                        crate::targets::display_name(self.id())
                    ),
                    hint:
                        "rename the canonical server or import/remove the target entry explicitly"
                            .into(),
                }),
                Some(value) if hash_json(value)? != rendered_hash => {
                    let change = Change {
                        server: name.clone(),
                        kind: ChangeKind::DriftRepair,
                    };
                    changes.push(change.clone());
                    inventory.managed_drift.push(change);
                    self.servers_mut(&mut doc)?.insert(name.clone(), rendered);
                }
                Some(_) => inventory.managed_synchronized.push(name.clone()),
                None => {
                    let change = Change {
                        server: name.clone(),
                        kind: ChangeKind::Add,
                    };
                    changes.push(change.clone());
                    if owned.contains_key(&name) {
                        inventory.managed_drift.push(change);
                    }
                    self.servers_mut(&mut doc)?.insert(name.clone(), rendered);
                }
            }
            managed.insert(
                name,
                ManagedServer {
                    canonical_hash: hash_serializable(server)?,
                    rendered_hash,
                },
            );
        }
        for name in owned.keys().filter(|name| !managed.contains_key(*name)) {
            if current.contains_key(name) {
                let change = Change {
                    server: name.clone(),
                    kind: ChangeKind::Remove,
                };
                changes.push(change.clone());
                inventory.managed_drift.push(change);
                self.servers_mut(&mut doc)?.remove(name);
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
        } else if let Some(source) = before.as_deref() {
            let source = std::str::from_utf8(source).map_err(|_| McpdError::InvalidInput {
                message: format!("{} is not UTF-8", self.path.display()),
                hint: "repair the target configuration before syncing".into(),
            })?;
            let final_servers = self.servers(&doc)?.cloned().unwrap_or_default();
            let changed = changes
                .iter()
                .filter(|change| change.kind != ChangeKind::Remove)
                .map(|change| change.server.clone())
                .collect::<Vec<_>>();
            let removed = changes
                .iter()
                .filter(|change| change.kind == ChangeKind::Remove)
                .map(|change| change.server.clone())
                .collect::<Vec<_>>();
            super::jsonc::patch_servers(
                source,
                &self.profile.servers_path,
                &final_servers,
                &changed,
                &removed,
            )?
        } else {
            serde_json::to_vec_pretty(&doc).map_err(|e| self.bad(e.to_string()))?
        };
        let next_state = TargetState {
            config_path: self.path.clone(),
            adapter_version: VERSION,
            last_success_unix_ms: 0,
            managed,
        };
        let state_changed = previous.is_some_and(|state| {
            state.config_path != next_state.config_path
                || state.adapter_version != VERSION
                || state.managed != next_state.managed
        });
        Ok(TargetPlan {
            target: self.id().into(),
            path: self.path.clone(),
            before,
            rendered,
            changes,
            inventory,
            next_state,
            state_changed,
        })
    }
}

fn import_values(
    profile: &Profile,
    name: &str,
    value: Option<&Value>,
    mappings: &BTreeMap<String, String>,
    writes: &mut Vec<crate::secrets::SecretWrite>,
) -> Result<BTreeMap<String, ConfigValue>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let object = value.as_object().ok_or_else(|| McpdError::InvalidInput {
        message: format!("MCP server `{name}` environment/headers must be an object"),
        hint: "use string values".into(),
    })?;
    let mut result = BTreeMap::new();
    for (field, value) in object {
        let value = value.as_str().ok_or_else(|| McpdError::InvalidInput {
            message: format!("MCP server `{name}` field `{field}` must be a string"),
            hint: "repair the target entry".into(),
        })?;
        if let Some(source) = native_env_reference(profile, value) {
            result.insert(
                field.clone(),
                ConfigValue::literal(format!("${{env:{source}}}")),
            );
            continue;
        }
        let scoped = format!("{name}\0{field}");
        if let Some(secret) = mappings.get(&scoped).or_else(|| mappings.get(field)) {
            result.insert(field.clone(), ConfigValue::secret(secret));
            writes.push(crate::secrets::SecretWrite {
                name: secret.clone(),
                value: crate::secrets::SecretValue::new(value.into()),
            });
        } else if crate::config::is_sensitive_field(field) {
            return Err(McpdError::InvalidInput {
                message: format!(
                    "MCP server `{name}` field `{field}` looks secret-bearing and requires migration"
                ),
                hint: "run import normally to use its deterministic keyring name".into(),
            });
        } else {
            result.insert(field.clone(), ConfigValue::literal(value));
        }
    }
    Ok(result)
}

fn string_array(values: &[Value]) -> std::result::Result<Vec<String>, String> {
    values
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "array must contain strings".into())
        })
        .collect()
}
fn normalize(profile: &Profile, value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(o) = value.as_object_mut() {
        o.remove("disabled");
        if !profile.explicit_type {
            o.remove("type");
        } else if o.get("type").and_then(Value::as_str) == Some("streamable-http") {
            o.insert("type".into(), json!("http"));
        }
    }
    value
}
fn hash_json(value: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|error| McpdError::Operational {
        message: format!("could not normalize JSON target value: {error}"),
        hint: "report this as an mcpd bug".into(),
    })?;
    Ok(hash_bytes(&bytes))
}

fn hash_serializable(value: &impl serde::Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|error| McpdError::Operational {
        message: format!("could not normalize canonical server: {error}"),
        hint: "report this as an mcpd bug".into(),
    })?;
    Ok(hash_bytes(&bytes))
}
fn env_reference(value: &str) -> Result<Option<&str>> {
    if let Some(name) = value
        .strip_prefix("${env:")
        .and_then(|v| v.strip_suffix('}'))
    {
        return Ok(Some(name));
    }
    if value.contains("${env:") {
        return Err(McpdError::InvalidInput {
            message: "invalid environment reference".into(),
            hint: "use `${env:NAME}` as the complete value".into(),
        });
    }
    Ok(None)
}
fn native_env_reference<'a>(profile: &Profile, value: &'a str) -> Option<&'a str> {
    match profile.id {
        "claude" => value
            .strip_prefix("${")
            .and_then(|value| value.strip_suffix('}'))
            .filter(|name| !name.contains(':')),
        "cursor" => value
            .strip_prefix("${env:")
            .and_then(|value| value.strip_suffix('}')),
        "openchamber" => value
            .strip_prefix("{env:")
            .and_then(|value| value.strip_suffix('}')),
        _ => None,
    }
}
fn executable_on_path(command: &str) -> bool {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .any(|d| d.join(command).is_file())
}
