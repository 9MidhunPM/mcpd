use std::{io, path::PathBuf};

use thiserror::Error;

pub type Result<T> = std::result::Result<T, McpdError>;

#[derive(Debug, Error)]
pub enum McpdError {
    #[error("{message}")]
    InvalidInput { message: String, hint: String },
    #[error("{message}")]
    Operational { message: String, hint: String },
    #[error("target `{target}` is unavailable: {message}")]
    TargetUnavailable {
        target: String,
        message: String,
        hint: String,
    },
    #[error("{message}")]
    Conflict { message: String, hint: String },
    #[error("security check failed for {}: {message}", path.display())]
    Security {
        path: PathBuf,
        message: String,
        hint: String,
    },
    #[error("could not access {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl McpdError {
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::InvalidInput { hint, .. }
            | Self::Operational { hint, .. }
            | Self::TargetUnavailable { hint, .. }
            | Self::Conflict { hint, .. }
            | Self::Security { hint, .. } => Some(hint),
            Self::Io { .. } => None,
        }
    }

    pub fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

#[repr(i32)]
pub enum ExitCode {
    Operational = 1,
    InvalidInput = 2,
    TargetUnavailable = 3,
    Conflict = 4,
    Security = 5,
}

impl From<&McpdError> for ExitCode {
    fn from(value: &McpdError) -> Self {
        match value {
            McpdError::InvalidInput { .. } => Self::InvalidInput,
            McpdError::TargetUnavailable { .. } => Self::TargetUnavailable,
            McpdError::Conflict { .. } => Self::Conflict,
            McpdError::Security { .. } => Self::Security,
            McpdError::Operational { .. } | McpdError::Io { .. } => Self::Operational,
        }
    }
}
