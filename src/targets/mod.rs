pub mod codex;

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

pub struct TargetImport {
    pub target: &'static str,
    pub path: PathBuf,
    pub snapshot: Vec<u8>,
    pub servers: BTreeMap<String, ImportedServer>,
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
    pub target: &'static str,
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

impl TargetPlan {
    pub fn is_noop(&self) -> bool {
        self.changes.is_empty() && !self.state_changed
    }
}

pub trait TargetAdapter {
    fn id(&self) -> &'static str;
    fn adapter_version(&self) -> u32;
    fn detect(&self) -> bool;
    fn config_path(&self) -> &Path;
    fn secret_capabilities(&self) -> SecretCapabilities;
    fn server_names(&self) -> Result<Vec<String>>;
    fn import_secret_candidates(
        &self,
        selection: Option<&BTreeSet<String>>,
    ) -> Result<Vec<ImportSecretCandidate>>;
    fn import(
        &self,
        selection: Option<&BTreeSet<String>>,
        secret_mappings: &BTreeMap<String, String>,
    ) -> Result<TargetImport>;
    fn plan(&self, desired: &CanonicalConfig, state: Option<&TargetState>) -> Result<TargetPlan>;
}

pub fn adapter(id: &str, paths: &Paths) -> Result<Box<dyn TargetAdapter>> {
    match id {
        "codex" => Ok(Box::new(codex::CodexAdapter::new(
            paths.codex_config.clone(),
            paths.home.clone(),
        ))),
        _ => Err(McpdError::TargetUnavailable {
            target: id.into(),
            message: "no built-in adapter is available in this milestone".into(),
            hint: "supported target: codex".into(),
        }),
    }
}
