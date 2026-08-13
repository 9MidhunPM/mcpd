use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    Paths,
    diagnostics::{McpdError, Result},
    model::{CanonicalConfig, ConfigValue, Server},
    state::{ManagedServer, TargetState},
    sync::fs::{ensure_safe_target_path, hash_bytes},
    targets::{
        Change, ChangeKind, HttpSecretCapability, ImportMode, ImportSecretCandidate,
        SecretCapabilities, StdioSecretCapability, TargetAdapter, TargetImport, TargetInventory,
        TargetPlan,
    },
};

const ADAPTER_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    id: String,
    name: String,
    #[serde(default)]
    platforms: Vec<String>,
    #[serde(default)]
    detect: Detect,
    config: Config,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Detect {
    #[serde(default)]
    commands: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    path: String,
    format: String,
    servers_path: String,
}

pub struct DeclarativeAdapter {
    id: String,
    name: String,
    path: PathBuf,
    home: PathBuf,
    commands: Vec<String>,
    servers_path: Vec<String>,
    platform_supported: bool,
}

pub fn discover(paths: &Paths) -> Result<Vec<DeclarativeAdapter>> {
    let Some(config_root) = paths.config.parent() else {
        return Ok(Vec::new());
    };
    let directory = config_root.join("targets");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(McpdError::io(&directory, source)),
    };
    let mut manifests = entries
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|source| McpdError::io(&directory, source))?;
    manifests.sort_by_key(std::fs::DirEntry::file_name);
    let mut adapters = Vec::new();
    let mut ids = BTreeSet::new();
    for entry in manifests {
        let manifest_path = entry.path();
        if manifest_path.extension().and_then(|value| value.to_str()) != Some("toml") {
            continue;
        }
        let text = fs::read_to_string(&manifest_path)
            .map_err(|source| McpdError::io(&manifest_path, source))?;
        let manifest: Manifest =
            toml::from_str(&text).map_err(|error| McpdError::InvalidInput {
                message: format!(
                    "declarative target manifest {} is invalid: {error}",
                    manifest_path.display()
                ),
                hint: "repair or remove the manifest before using target commands".into(),
            })?;
        validate_manifest(&manifest, &manifest_path)?;
        if crate::targets::TARGET_IDS.contains(&manifest.id.as_str())
            || !ids.insert(manifest.id.clone())
        {
            return Err(McpdError::Conflict {
                message: format!(
                    "duplicate target ID `{}` in {}",
                    manifest.id,
                    manifest_path.display()
                ),
                hint: "choose a unique custom target ID that does not shadow a built-in adapter"
                    .into(),
            });
        }
        let path = expand_path(&manifest.config.path, &paths.home, &manifest_path)?;
        adapters.push(DeclarativeAdapter {
            id: manifest.id,
            name: manifest.name,
            path,
            home: paths.home.clone(),
            commands: manifest.detect.commands,
            servers_path: manifest
                .config
                .servers_path
                .split('.')
                .map(str::to_owned)
                .collect(),
            platform_supported: manifest.platforms.is_empty()
                || manifest
                    .platforms
                    .iter()
                    .any(|value| value == std::env::consts::OS),
        });
    }
    Ok(adapters)
}

fn validate_manifest(manifest: &Manifest, path: &Path) -> Result<()> {
    if !crate::config::is_valid_server_id(&manifest.id) {
        return Err(McpdError::InvalidInput {
            message: format!(
                "declarative target ID `{}` in {} is invalid",
                manifest.id,
                path.display()
            ),
            hint:
                "use letters, digits, dots, underscores, or hyphens; start with a letter or digit"
                    .into(),
        });
    }
    if manifest.name.trim().is_empty() {
        return Err(McpdError::InvalidInput {
            message: format!("declarative target `{}` has an empty name", manifest.id),
            hint: "set a human-readable target name".into(),
        });
    }
    if !matches!(manifest.config.format.as_str(), "json" | "jsonc") {
        return Err(McpdError::InvalidInput {
            message: format!(
                "declarative target `{}` uses unsupported format `{}`",
                manifest.id, manifest.config.format
            ),
            hint: "v1 declarative adapters support `json` and `jsonc`".into(),
        });
    }
    if manifest.config.servers_path.split('.').any(str::is_empty) {
        return Err(McpdError::InvalidInput {
            message: format!(
                "declarative target `{}` has an invalid servers_path",
                manifest.id
            ),
            hint: "use a dot-separated object path such as `mcp.servers`".into(),
        });
    }
    if manifest
        .detect
        .commands
        .iter()
        .any(|command| command.trim().is_empty())
    {
        return Err(McpdError::InvalidInput {
            message: format!(
                "declarative target `{}` has an empty detection command",
                manifest.id
            ),
            hint: "remove the empty command name".into(),
        });
    }
    Ok(())
}

fn expand_path(value: &str, home: &Path, manifest: &Path) -> Result<PathBuf> {
    let path = if value == "~" {
        home.to_path_buf()
    } else if let Some(relative) = value.strip_prefix("~/") {
        home.join(relative)
    } else {
        PathBuf::from(value)
    };
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(McpdError::Security {
            path: manifest.to_path_buf(),
            message: format!("declarative target path `{value}` is not a safe absolute/home path"),
            hint: "use `~/...` or an absolute path below the user home directory without `..`"
                .into(),
        });
    }
    ensure_safe_target_path(&path, home)?;
    Ok(path)
}

impl DeclarativeAdapter {
    fn read(&self, allow_missing: bool) -> Result<(Option<Vec<u8>>, Value)> {
        ensure_safe_target_path(&self.path, &self.home)?;
        match fs::read(&self.path) {
            Ok(bytes) => {
                let (_, value) = super::jsonc::parse(&bytes, &self.name, &self.path)?;
                if !value.is_object() {
                    return Err(McpdError::InvalidInput {
                        message: format!("{} configuration root must be an object", self.name),
                        hint: "repair the target configuration before syncing".into(),
                    });
                }
                Ok((Some(bytes), value))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_missing => {
                Ok((None, json!({})))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(McpdError::TargetUnavailable {
                    target: self.id.clone(),
                    message: format!("configuration {} does not exist", self.path.display()),
                    hint: "create or synchronize the target before importing".into(),
                })
            }
            Err(source) => Err(McpdError::io(&self.path, source)),
        }
    }

    fn servers<'a>(&self, document: &'a Value) -> Result<Option<&'a Map<String, Value>>> {
        let value = self
            .servers_path
            .iter()
            .try_fold(document, |value, key| value.get(key));
        value
            .map(|value| {
                value.as_object().ok_or_else(|| McpdError::InvalidInput {
                    message: format!("{} MCP server collection must be an object", self.name),
                    hint: "repair the declarative target configuration before syncing".into(),
                })
            })
            .transpose()
    }

    fn servers_mut<'a>(&self, document: &'a mut Value) -> Result<&'a mut Map<String, Value>> {
        let mut current = document
            .as_object_mut()
            .ok_or_else(|| McpdError::InvalidInput {
                message: format!("{} configuration root must be an object", self.name),
                hint: "repair the target configuration before syncing".into(),
            })?;
        for key in &self.servers_path {
            current = current
                .entry(key)
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| McpdError::InvalidInput {
                    message: format!("{} path component `{key}` must be an object", self.name),
                    hint: "repair the target configuration before syncing".into(),
                })?;
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
                if !secrets.is_empty()
                    || env.values().any(|value| {
                        value.secret_name().is_some()
                            || matches!(value, ConfigValue::Literal(value) if value.starts_with("${env:"))
                    })
                {
                    return Ok(json!({"command":"mcpd", "args":["exec", name]}));
                }
                let mut rendered = Map::from_iter([("command".into(), json!(command))]);
                if !args.is_empty() {
                    rendered.insert("args".into(), json!(args));
                }
                if !env.is_empty() {
                    let values = env
                        .iter()
                        .map(|(key, value)| match value {
                            ConfigValue::Literal(value) => Ok((key.clone(), json!(value))),
                            ConfigValue::Secret { .. } => Err(McpdError::Operational {
                                message: "secret reference escaped declarative wrapper planning"
                                    .into(),
                                hint: "report this as an mcpd bug".into(),
                            }),
                        })
                        .collect::<Result<Map<_, _>>>()?;
                    rendered.insert("env".into(), Value::Object(values));
                }
                if let Some(cwd) = cwd {
                    rendered.insert("cwd".into(), json!(cwd));
                }
                Ok(Value::Object(rendered))
            }
            Server::Http {
                url,
                headers,
                secrets,
            } => {
                if !secrets.is_empty()
                    || headers.values().any(|value| {
                        value.secret_name().is_some()
                            || matches!(value, ConfigValue::Literal(value) if value.starts_with("${env:"))
                    })
                {
                    return Err(McpdError::InvalidInput {
                        message: format!(
                            "{} cannot safely represent secret/environment-backed HTTP server `{name}`",
                            self.name
                        ),
                        hint: "keep the server disabled for this custom target; declarative adapters do not guess client placeholder syntax".into(),
                    });
                }
                let mut rendered = Map::from_iter([("url".into(), json!(url))]);
                if !headers.is_empty() {
                    let values = headers
                        .iter()
                        .map(|(key, value)| match value {
                            ConfigValue::Literal(value) => Ok((key.clone(), json!(value))),
                            ConfigValue::Secret { .. } => Err(McpdError::Operational {
                                message: "secret reference escaped declarative HTTP validation"
                                    .into(),
                                hint: "report this as an mcpd bug".into(),
                            }),
                        })
                        .collect::<Result<Map<_, _>>>()?;
                    rendered.insert("headers".into(), Value::Object(values));
                }
                Ok(Value::Object(rendered))
            }
        }
    }
}

impl TargetAdapter for DeclarativeAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn adapter_version(&self) -> u32 {
        ADAPTER_VERSION
    }

    fn detect(&self) -> bool {
        self.platform_supported
            && (self.path.exists()
                || self
                    .commands
                    .iter()
                    .any(|command| executable_on_path(command)))
    }

    fn config_path(&self) -> &Path {
        &self.path
    }

    fn secret_capabilities(&self) -> SecretCapabilities {
        SecretCapabilities {
            stdio: StdioSecretCapability::RuntimeInjection,
            http: HttpSecretCapability::Unsupported,
        }
    }

    fn native_schema(&self) -> &'static str {
        "declarative-json/v1"
    }

    fn server_names(&self) -> Result<Vec<String>> {
        let (_, document) = self.read(true)?;
        Ok(self
            .servers(&document)?
            .map(|servers| servers.keys().cloned().collect())
            .unwrap_or_default())
    }

    fn import_secret_candidates(
        &self,
        _selection: Option<&BTreeSet<String>>,
    ) -> Result<Vec<ImportSecretCandidate>> {
        Ok(Vec::new())
    }

    fn import(
        &self,
        _selection: Option<&BTreeSet<String>>,
        _secret_mappings: &BTreeMap<String, String>,
        _mode: ImportMode,
    ) -> Result<TargetImport> {
        Err(McpdError::TargetUnavailable {
            target: self.id.clone(),
            message: "v1 declarative adapters are export-only".into(),
            hint:
                "import through a built-in adapter or add the server to canonical config explicitly"
                    .into(),
        })
    }

    fn plan(
        &self,
        desired: &CanonicalConfig,
        previous: Option<&TargetState>,
    ) -> Result<TargetPlan> {
        if !self.platform_supported {
            return Err(McpdError::TargetUnavailable {
                target: self.id.clone(),
                message: format!(
                    "the manifest does not support platform `{}`",
                    std::env::consts::OS
                ),
                hint: "update the manifest platforms list or use the target on a supported system"
                    .into(),
            });
        }
        let (before, mut document) = self.read(true)?;
        let current = self.servers(&document)?.cloned().unwrap_or_default();
        let owned = previous
            .filter(|state| {
                state.config_path == self.path && state.adapter_version == ADAPTER_VERSION
            })
            .map(|state| state.managed.clone())
            .unwrap_or_default();
        let target = desired.targets.get(self.id());
        let mut wanted = BTreeMap::new();
        if target.is_some_and(|target| target.enabled) {
            for (name, server) in &desired.servers {
                if !target
                    .and_then(|target| target.servers.get(name))
                    .is_some_and(|server| !server.enabled)
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
            let rendered_hash = hash_value(&rendered)?;
            match current.get(&name) {
                Some(_) if !owned.contains_key(&name) => {
                    return Err(McpdError::Conflict {
                        message: format!("{} already has unmanaged MCP server `{name}`", self.name),
                        hint: "rename the canonical server or remove the unmanaged collision explicitly".into(),
                    });
                }
                Some(value) if hash_value(value)? != rendered_hash => {
                    let change = Change {
                        server: name.clone(),
                        kind: ChangeKind::DriftRepair,
                    };
                    changes.push(change.clone());
                    inventory.managed_drift.push(change);
                    self.servers_mut(&mut document)?
                        .insert(name.clone(), rendered);
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
                    self.servers_mut(&mut document)?
                        .insert(name.clone(), rendered);
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
                self.servers_mut(&mut document)?.remove(name);
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
            let final_servers = self.servers(&document)?.cloned().unwrap_or_default();
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
                &self.servers_path,
                &final_servers,
                &changed,
                &removed,
            )?
        } else {
            serde_json::to_vec_pretty(&document).map_err(|error| McpdError::Operational {
                message: format!("could not serialize {} configuration: {error}", self.name),
                hint: "report this as an mcpd bug".into(),
            })?
        };
        let next_state = TargetState {
            config_path: self.path.clone(),
            adapter_version: ADAPTER_VERSION,
            last_success_unix_ms: 0,
            managed,
        };
        let state_changed = previous.is_some_and(|state| {
            state.config_path != next_state.config_path
                || state.adapter_version != ADAPTER_VERSION
                || state.managed != next_state.managed
        });
        Ok(TargetPlan {
            target: self.id.clone(),
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

fn hash_value(value: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|error| McpdError::Operational {
        message: format!("could not normalize declarative target value: {error}"),
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

fn executable_on_path(command: &str) -> bool {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .any(|path| path.join(command).is_file())
}
