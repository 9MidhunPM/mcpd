pub mod cli;
pub mod config;
pub mod diagnostics;
pub mod execution;
pub mod import;
pub mod model;
pub mod output;
pub mod secrets;
pub mod state;
pub mod sync;
pub mod targets;

use std::path::PathBuf;

use directories::BaseDirs;

use crate::diagnostics::{McpdError, Result};

#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub state_dir: PathBuf,
    pub codex_config: PathBuf,
    pub home: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        let base = BaseDirs::new().ok_or_else(|| McpdError::Operational {
            message: "could not determine the user home directory".into(),
            hint: "set HOME, or set MCPD_CONFIG, MCPD_STATE_DIR, and MCPD_CODEX_CONFIG".into(),
        })?;
        let home = std::env::var_os("MCPD_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| base.home_dir().to_path_buf());
        Ok(Self {
            config: std::env::var_os("MCPD_CONFIG")
                .map(PathBuf::from)
                .unwrap_or_else(|| base.config_dir().join("mcpd/config.toml")),
            state_dir: std::env::var_os("MCPD_STATE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    base.state_dir()
                        .unwrap_or_else(|| base.data_local_dir())
                        .join("mcpd")
                }),
            codex_config: std::env::var_os("MCPD_CODEX_CONFIG")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex/config.toml")),
            home,
        })
    }
}
