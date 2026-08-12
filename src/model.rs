use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalConfig {
    pub version: u32,
    #[serde(default)]
    pub servers: BTreeMap<String, Server>,
    #[serde(default)]
    pub targets: BTreeMap<String, TargetConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase", deny_unknown_fields)]
pub enum Server {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, ConfigValue>,
        #[serde(default)]
        secrets: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
    },
    Http {
        url: Url,
        #[serde(default)]
        headers: BTreeMap<String, ConfigValue>,
        #[serde(default)]
        secrets: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    Literal(String),
    Secret { secret: String },
}

impl ConfigValue {
    pub fn literal(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    pub fn secret(name: impl Into<String>) -> Self {
        Self::Secret {
            secret: name.into(),
        }
    }

    pub fn secret_name(&self) -> Option<&str> {
        match self {
            Self::Secret { secret } => Some(secret),
            Self::Literal(value) => value
                .strip_prefix("${secret:")
                .and_then(|value| value.strip_suffix('}')),
        }
    }
}

impl Server {
    pub fn secrets(&self) -> &BTreeMap<String, String> {
        match self {
            Self::Stdio { secrets, .. } | Self::Http { secrets, .. } => secrets,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub servers: BTreeMap<String, TargetServerConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetServerConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

const fn default_true() -> bool {
    true
}
