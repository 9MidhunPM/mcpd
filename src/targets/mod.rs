pub mod codex;
mod declarative;
pub mod json;
mod jsonc;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    Paths,
    diagnostics::{McpdError, Result},
    model::{CanonicalConfig, Server},
    state::{ManagedServer, TargetState},
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Add,
    Update,
    Remove,
    DriftRepair,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Change {
    pub server: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct TargetInventory {
    pub managed_synchronized: Vec<String>,
    pub managed_drift: Vec<Change>,
    pub only_in_target: Vec<String>,
    pub only_in_mcpd: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ImportedServer {
    pub server: Server,
    pub ownership: ManagedServer,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ImportSkipped {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    Strict,
    BestEffort,
}

pub struct TargetImport {
    pub target: String,
    pub path: PathBuf,
    pub snapshot: Vec<u8>,
    pub servers: BTreeMap<String, ImportedServer>,
    pub skipped: Vec<ImportSkipped>,
    pub secret_writes: Vec<crate::secrets::SecretWrite>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSecretCandidate {
    pub server: String,
    pub field: String,
    pub sensitive: bool,
}

#[derive(Debug, Clone)]
pub struct TargetPlan {
    pub target: String,
    pub path: PathBuf,
    pub before: Option<Vec<u8>>,
    pub rendered: Vec<u8>,
    pub changes: Vec<Change>,
    pub inventory: TargetInventory,
    pub next_state: TargetState,
    pub state_changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioSecretCapability {
    RuntimeInjection,
    EnvironmentReference,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpSecretCapability {
    EnvironmentReference,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretCapabilities {
    pub stdio: StdioSecretCapability,
    pub http: HttpSecretCapability,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Compatibility {
    pub adapter_version: u32,
    pub native_schema: &'static str,
    pub client_version: Option<String>,
    pub status: &'static str,
}

impl TargetPlan {
    pub fn is_noop(&self) -> bool {
        self.changes.is_empty() && !self.state_changed
    }
}

pub trait TargetAdapter {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str {
        display_name(self.id())
    }
    fn adapter_version(&self) -> u32;
    fn detect(&self) -> bool;
    fn config_path(&self) -> &Path;
    fn secret_capabilities(&self) -> SecretCapabilities;
    fn native_schema(&self) -> &'static str;
    fn compatibility(&self, client_version: Option<&str>) -> Compatibility {
        Compatibility {
            adapter_version: self.adapter_version(),
            native_schema: self.native_schema(),
            client_version: client_version.map(str::to_owned),
            status: if client_version.is_some() {
                "configured"
            } else {
                "schema_checked"
            },
        }
    }
    fn server_names(&self) -> Result<Vec<String>>;
    fn import_secret_candidates(
        &self,
        selection: Option<&BTreeSet<String>>,
    ) -> Result<Vec<ImportSecretCandidate>>;
    fn import(
        &self,
        selection: Option<&BTreeSet<String>>,
        secret_mappings: &BTreeMap<String, String>,
        mode: ImportMode,
    ) -> Result<TargetImport>;
    fn plan(&self, desired: &CanonicalConfig, state: Option<&TargetState>) -> Result<TargetPlan>;
}

pub const PRIMARY_TARGET_IDS: &[&str] =
    &["claude", "cursor", "codex", "antigravity", "openchamber"];
pub const TARGET_IDS: &[&str] = &[
    "claude",
    "claude-project",
    "claude-local",
    "cursor",
    "codex",
    "antigravity",
    "openchamber",
];

pub fn adapter(id: &str, paths: &Paths) -> Result<Box<dyn TargetAdapter>> {
    match id {
        "codex" => Ok(Box::new(codex::CodexAdapter::new(
            paths.codex_config.clone(),
            paths.home.clone(),
        ))),
        "claude" | "claude-project" | "claude-local" | "cursor" | "antigravity" | "openchamber" => {
            Ok(Box::new(json::JsonAdapter::new(id, paths)?))
        }
        _ => declarative::discover(paths)?
            .into_iter()
            .find(|adapter| adapter.id() == id)
            .map(|adapter| Box::new(adapter) as Box<dyn TargetAdapter>)
            .ok_or_else(|| McpdError::TargetUnavailable {
                target: id.into(),
                message: "no built-in or declarative adapter is available".into(),
                hint: format!(
                    "supported built-in targets: {}; custom manifests belong in {}/targets",
                    TARGET_IDS.join(", "),
                    paths
                        .config
                        .parent()
                        .unwrap_or(Path::new("<config-dir>"))
                        .display()
                ),
            }),
    }
}

pub fn adapters(paths: &Paths) -> Result<Vec<Box<dyn TargetAdapter>>> {
    let mut adapters = PRIMARY_TARGET_IDS
        .iter()
        .map(|id| adapter(id, paths))
        .collect::<Result<Vec<_>>>()?;
    if crate::resolve::current_project(paths)?.is_some_and(|project| project.trusted) {
        adapters.push(adapter("claude-project", paths)?);
        adapters.push(adapter("claude-local", paths)?);
    }
    adapters.extend(
        declarative::discover(paths)?
            .into_iter()
            .map(|adapter| Box::new(adapter) as Box<dyn TargetAdapter>),
    );
    Ok(adapters)
}

pub fn display_name(id: &str) -> &str {
    match id {
        "claude" => "Claude Code",
        "claude-project" => "Claude Code (project)",
        "claude-local" => "Claude Code (local)",
        "cursor" => "Cursor",
        "codex" => "Codex",
        "antigravity" => "Antigravity",
        "openchamber" => "OpenChamber",
        other => other,
    }
}
