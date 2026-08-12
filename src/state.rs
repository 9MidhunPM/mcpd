use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    diagnostics::{McpdError, Result},
    sync::fs::atomic_write,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StateFile {
    pub version: u32,
    #[serde(default)]
    pub targets: BTreeMap<String, TargetState>,
}

impl Default for StateFile {
    fn default() -> Self {
        Self {
            version: 1,
            targets: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetState {
    pub config_path: PathBuf,
    pub adapter_version: u32,
    pub last_success_unix_ms: u128,
    #[serde(default)]
    pub managed: BTreeMap<String, ManagedServer>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedServer {
    pub canonical_hash: String,
    pub rendered_hash: String,
}

pub fn load(path: &Path) -> Result<StateFile> {
    if !path.exists() {
        return Ok(StateFile::default());
    }
    let text = fs::read_to_string(path).map_err(|source| McpdError::io(path, source))?;
    let state: StateFile = toml::from_str(&text).map_err(|error| McpdError::InvalidInput {
        message: format!("ownership state {} is malformed: {error}", path.display()),
        hint: "restore the state file from a known-good copy; mcpd will not guess ownership".into(),
    })?;
    if state.version != 1 {
        return Err(McpdError::InvalidInput {
            message: format!(
                "ownership state {} uses unsupported version {}",
                path.display(),
                state.version
            ),
            hint: "use a compatible mcpd release or migrate the state explicitly".into(),
        });
    }
    Ok(state)
}

pub fn save(path: &Path, state: &StateFile) -> Result<()> {
    let text = toml::to_string_pretty(state).map_err(|error| McpdError::Operational {
        message: format!("could not serialize ownership state: {error}"),
        hint: "report this as an mcpd bug".into(),
    })?;
    atomic_write(path, text.as_bytes(), Some(0o600))
}
