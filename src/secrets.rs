use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::{
    Paths,
    diagnostics::{Result, SyncplaneError},
    sync,
};

const SERVICE: &str = "syncplane";

pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

pub trait SecretBackend {
    fn get(&self, name: &str) -> Result<Option<SecretValue>>;
    fn set(&self, name: &str, value: &SecretValue) -> Result<()>;
    fn delete(&self, name: &str) -> Result<()>;
}

struct OsKeyring;

impl OsKeyring {
    fn entry(name: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, name).map_err(|error| keyring_error("open", name, error))
    }
}

impl SecretBackend for OsKeyring {
    fn get(&self, name: &str) -> Result<Option<SecretValue>> {
        match Self::entry(name)?.get_password() {
            Ok(value) => Ok(Some(SecretValue::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keyring_error("read", name, error)),
        }
    }

    fn set(&self, name: &str, value: &SecretValue) -> Result<()> {
        Self::entry(name)?
            .set_password(value.expose())
            .map_err(|error| keyring_error("store", name, error))
    }

    fn delete(&self, name: &str) -> Result<()> {
        match Self::entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keyring_error("delete", name, error)),
        }
    }
}

fn keyring_error(action: &str, name: &str, error: keyring::Error) -> SyncplaneError {
    SyncplaneError::Operational {
        message: format!("could not {action} secret `{name}` in the OS keyring: {error}"),
        hint: "ensure a Secret Service or KWallet-compatible keyring is available and unlocked"
            .into(),
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Registry {
    version: u32,
    #[serde(default)]
    names: BTreeSet<String>,
}

pub struct SecretStore {
    backend: Box<dyn SecretBackend>,
    registry_path: PathBuf,
}

pub struct SecretWrite {
    pub name: String,
    pub value: SecretValue,
}

pub struct SecretRollback(Vec<(String, Option<SecretValue>)>);

impl SecretStore {
    pub fn discover(paths: &Paths) -> Result<Self> {
        let backend: Box<dyn SecretBackend> =
            match std::env::var("SYNCPLANE_SECRET_BACKEND").ok().as_deref() {
                Some("mock-file") if cfg!(debug_assertions) => Box::new(MockFileBackend {
                    path: paths.state_dir.join("test-secret-store.toml"),
                }),
                Some("mock-file") => {
                    return Err(SyncplaneError::Security {
                        path: paths.state_dir.clone(),
                        message: "the mock secret backend is disabled in release builds".into(),
                        hint: "use the operating-system keyring".into(),
                    });
                }
                Some(value) => {
                    return Err(SyncplaneError::InvalidInput {
                        message: format!("unsupported SYNCPLANE_SECRET_BACKEND `{value}`"),
                        hint: "unset it to use the operating-system keyring".into(),
                    });
                }
                None => Box::new(OsKeyring),
            };
        Ok(Self {
            backend,
            registry_path: paths.state_dir.join("secrets.toml"),
        })
    }

    #[cfg(test)]
    pub fn in_memory(registry_path: PathBuf) -> Self {
        Self {
            backend: Box::new(MemoryBackend::default()),
            registry_path,
        }
    }

    pub fn get(&self, name: &str) -> Result<Option<SecretValue>> {
        validate_name(name)?;
        self.backend.get(name)
    }

    pub fn set(&self, name: &str, value: SecretValue) -> Result<()> {
        validate_name(name)?;
        if value.expose().is_empty() {
            return Err(SyncplaneError::InvalidInput {
                message: format!("secret `{name}` cannot be empty"),
                hint: "enter a non-empty value".into(),
            });
        }
        if let Some(parent) = self.registry_path.parent() {
            sync::ensure_private_dir(parent)?;
        }
        let previous = self.backend.get(name)?;
        let mut registry = self.load_registry()?;
        self.backend.set(name, &value)?;
        registry.names.insert(name.into());
        if let Err(error) = self.save_registry(&registry) {
            restore(&*self.backend, name, previous)?;
            return Err(error);
        }
        Ok(())
    }

    pub fn delete(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        let previous = self.backend.get(name)?;
        let Some(previous) = previous else {
            return Ok(false);
        };
        let mut registry = self.load_registry()?;
        self.backend.delete(name)?;
        registry.names.remove(name);
        if let Err(error) = self.save_registry(&registry) {
            self.backend.set(name, &previous)?;
            return Err(error);
        }
        Ok(true)
    }

    pub fn names(&self) -> Result<Vec<String>> {
        Ok(self.load_registry()?.names.into_iter().collect())
    }

    pub fn contains(&self, name: &str) -> Result<bool> {
        Ok(self.get(name)?.is_some())
    }

    pub fn write_batch(&self, writes: Vec<SecretWrite>) -> Result<SecretRollback> {
        let mut rollback = Vec::new();
        for write in writes {
            let previous = match self.get(&write.name) {
                Ok(previous) => previous,
                Err(error) => {
                    self.rollback(SecretRollback(rollback))?;
                    return Err(error);
                }
            };
            if let Err(error) = self.set(&write.name, write.value) {
                self.rollback(SecretRollback(rollback))?;
                return Err(error);
            }
            rollback.push((write.name, previous));
        }
        Ok(SecretRollback(rollback))
    }

    pub fn rollback(&self, rollback: SecretRollback) -> Result<()> {
        for (name, previous) in rollback.0.into_iter().rev() {
            match previous {
                Some(value) => self.set(&name, value)?,
                None => {
                    self.delete(&name)?;
                }
            }
        }
        Ok(())
    }

    fn load_registry(&self) -> Result<Registry> {
        if !self.registry_path.exists() {
            return Ok(Registry {
                version: 1,
                names: BTreeSet::new(),
            });
        }
        let text = fs::read_to_string(&self.registry_path)
            .map_err(|source| SyncplaneError::io(&self.registry_path, source))?;
        let registry: Registry =
            toml::from_str(&text).map_err(|error| SyncplaneError::InvalidInput {
                message: format!(
                    "secret registry {} is malformed: {error}",
                    self.registry_path.display()
                ),
                hint: "repair the registry; it contains names only, never secret values".into(),
            })?;
        if registry.version != 1 {
            return Err(SyncplaneError::InvalidInput {
                message: format!("unsupported secret registry version {}", registry.version),
                hint: "use a compatible syncplane release".into(),
            });
        }
        Ok(registry)
    }

    fn save_registry(&self, registry: &Registry) -> Result<()> {
        let text =
            toml::to_string_pretty(registry).map_err(|error| SyncplaneError::Operational {
                message: format!("could not serialize the secret-name registry: {error}"),
                hint: "report this as a Syncplane bug".into(),
            })?;
        sync::fs::atomic_write(&self.registry_path, text.as_bytes(), Some(0o600))
    }
}

fn restore(backend: &dyn SecretBackend, name: &str, previous: Option<SecretValue>) -> Result<()> {
    match previous {
        Some(value) => backend.set(name, &value),
        None => backend.delete(name),
    }
}

pub fn prompt_value(name: &str) -> Result<SecretValue> {
    if cfg!(debug_assertions)
        && std::env::var("SYNCPLANE_SECRET_BACKEND").ok().as_deref() == Some("mock-file")
    {
        use std::io::BufRead;
        let mut value = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut value)
            .map_err(|source| SyncplaneError::io("<test-stdin>", source))?;
        while value.ends_with(['\n', '\r']) {
            value.pop();
        }
        return Ok(SecretValue::new(value));
    }
    rpassword::prompt_password(format!("Enter secret `{name}`: "))
        .map(SecretValue::new)
        .map_err(|source| SyncplaneError::Io {
            path: PathBuf::from("<terminal>"),
            source,
        })
}

pub fn validate_name(name: &str) -> Result<()> {
    if crate::config::is_valid_server_id(name) {
        Ok(())
    } else {
        Err(SyncplaneError::InvalidInput {
            message: format!("secret name `{name}` is invalid"),
            hint:
                "use letters, digits, dots, underscores, or hyphens; start with a letter or digit"
                    .into(),
        })
    }
}

#[cfg(test)]
#[derive(Default)]
struct MemoryBackend {
    values: std::sync::Mutex<BTreeMap<String, String>>,
}

#[cfg(test)]
impl SecretBackend for MemoryBackend {
    fn get(&self, name: &str) -> Result<Option<SecretValue>> {
        Ok(self
            .values
            .lock()
            .expect("memory secret lock poisoned")
            .get(name)
            .cloned()
            .map(SecretValue::new))
    }
    fn set(&self, name: &str, value: &SecretValue) -> Result<()> {
        self.values
            .lock()
            .expect("memory secret lock poisoned")
            .insert(name.into(), value.expose().into());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<()> {
        self.values
            .lock()
            .expect("memory secret lock poisoned")
            .remove(name);
        Ok(())
    }
}

struct MockFileBackend {
    path: PathBuf,
}

impl MockFileBackend {
    fn load(&self) -> Result<BTreeMap<String, String>> {
        if !self.path.exists() {
            return Ok(BTreeMap::new());
        }
        let text = fs::read_to_string(&self.path)
            .map_err(|source| SyncplaneError::io(&self.path, source))?;
        toml::from_str(&text).map_err(|error| SyncplaneError::Operational {
            message: format!("isolated test secret backend is malformed: {error}"),
            hint: "delete the isolated test directory and retry".into(),
        })
    }
    fn save(&self, values: &BTreeMap<String, String>) -> Result<()> {
        let text = toml::to_string(values).map_err(|error| SyncplaneError::Operational {
            message: format!("could not serialize isolated test secrets: {error}"),
            hint: "report this as a Syncplane bug".into(),
        })?;
        sync::fs::atomic_write(&self.path, text.as_bytes(), Some(0o600))
    }
}

impl SecretBackend for MockFileBackend {
    fn get(&self, name: &str) -> Result<Option<SecretValue>> {
        Ok(self.load()?.remove(name).map(SecretValue::new))
    }
    fn set(&self, name: &str, value: &SecretValue) -> Result<()> {
        let mut values = self.load()?;
        values.insert(name.into(), value.expose().into());
        self.save(&values)
    }
    fn delete(&self, name: &str) -> Result<()> {
        let mut values = self.load()?;
        values.remove(name);
        self.save(&values)
    }
}

pub fn referenced_names(config: &crate::model::CanonicalConfig) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for server in config.servers.values() {
        match server {
            crate::model::Server::Stdio { env, secrets, .. } => {
                names.extend(
                    env.values()
                        .filter_map(|value| value.secret_name())
                        .map(str::to_owned),
                );
                names.extend(secrets.values().cloned());
            }
            crate::model::Server::Http {
                headers, secrets, ..
            } => {
                names.extend(
                    headers
                        .values()
                        .filter_map(|value| value.secret_name())
                        .map(str::to_owned),
                );
                names.extend(secrets.values().cloned());
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn memory_store_tracks_names_without_exposing_values() {
        let temp = TempDir::new().unwrap();
        let store = SecretStore::in_memory(temp.path().join("registry.toml"));
        store
            .set("github.token", SecretValue::new("super-secret".into()))
            .unwrap();
        assert_eq!(store.names().unwrap(), vec!["github.token"]);
        assert!(store.contains("github.token").unwrap());
        let registry = fs::read_to_string(temp.path().join("registry.toml")).unwrap();
        assert!(!registry.contains("super-secret"));
        assert!(store.delete("github.token").unwrap());
        assert!(!store.contains("github.token").unwrap());
    }
}
