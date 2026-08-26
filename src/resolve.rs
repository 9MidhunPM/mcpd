use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    Paths, config,
    diagnostics::{Result, SyncplaneError},
    model::CanonicalConfig,
    sync::{self, fs::atomic_write},
};

const TRUST_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub config: CanonicalConfig,
    pub project: Option<ProjectContext>,
}

#[derive(Debug, Clone)]
pub struct ProjectContext {
    pub root: PathBuf,
    pub overlay: PathBuf,
    pub trusted: bool,
    pub present: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustFile {
    version: u32,
    #[serde(default)]
    projects: BTreeSet<PathBuf>,
}

pub fn load(paths: &Paths) -> Result<ResolvedConfig> {
    let mut canonical = config::load(&paths.config)?;
    let project = current_project(paths)?;
    if let Some(context) = &project {
        if context.present && context.trusted {
            canonical = config::apply_overlay(&canonical, &context.overlay)?;
        } else if context.present {
            tracing::warn!(
                project = %context.root.display(),
                overlay = %context.overlay.display(),
                "ignoring untrusted project overlay; run `syncplane trust` in this project to enable it"
            );
        }
    }
    Ok(ResolvedConfig {
        config: canonical,
        project,
    })
}

pub fn current_project(paths: &Paths) -> Result<Option<ProjectContext>> {
    let Some(root) = discover_project_root()? else {
        return Ok(None);
    };
    let overlay = root.join(".syncplane/config.toml");
    sync::fs::ensure_safe_target_path(&overlay, &root)?;
    let trusted = trusted_projects(paths)?.contains(&root);
    Ok(Some(ProjectContext {
        present: overlay_present(&overlay)?,
        root,
        overlay,
        trusted,
    }))
}

fn overlay_present(path: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(SyncplaneError::io(path, source)),
    };
    if metadata.file_type().is_symlink() {
        return Err(SyncplaneError::Security {
            path: path.to_path_buf(),
            message: "project overlay is a symbolic link".into(),
            hint: "replace it with a regular file inside the trusted project; overlay symlinks are not followed".into(),
        });
    }
    if !metadata.is_file() {
        return Err(SyncplaneError::InvalidInput {
            message: format!("project overlay {} is not a regular file", path.display()),
            hint: "replace it with a regular TOML file or remove the path".into(),
        });
    }
    Ok(true)
}

pub fn discover_project_root() -> Result<Option<PathBuf>> {
    if let Some(root) = env::var_os("SYNCPLANE_PROJECT_ROOT") {
        return canonical_directory(Path::new(&root)).map(Some);
    }
    let current =
        env::current_dir().map_err(|source| SyncplaneError::io("<current directory>", source))?;
    for candidate in current.ancestors() {
        if candidate.join(".git").exists() || candidate.join(".syncplane").exists() {
            return canonical_directory(candidate).map(Some);
        }
    }
    Ok(None)
}

pub fn trust(paths: &Paths, project: &Path) -> Result<PathBuf> {
    let project = canonical_directory(project)?;
    sync::with_sync_lock(paths, || {
        let mut trust = load_trust(paths)?;
        trust.projects.insert(project.clone());
        save_trust(paths, &trust)
    })?;
    Ok(project)
}

pub fn revoke(paths: &Paths, project: &Path) -> Result<(PathBuf, bool)> {
    let project = canonical_directory(project)?;
    let removed = sync::with_sync_lock(paths, || {
        let mut trust = load_trust(paths)?;
        let removed = trust.projects.remove(&project);
        if removed {
            save_trust(paths, &trust)?;
        }
        Ok(removed)
    })?;
    Ok((project, removed))
}

pub fn trusted_projects(paths: &Paths) -> Result<BTreeSet<PathBuf>> {
    Ok(load_trust(paths)?.projects)
}

fn canonical_directory(path: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path).map_err(|source| SyncplaneError::io(path, source))?;
    let metadata =
        fs::metadata(&canonical).map_err(|source| SyncplaneError::io(&canonical, source))?;
    if !metadata.is_dir() {
        return Err(SyncplaneError::InvalidInput {
            message: format!("project path {} is not a directory", canonical.display()),
            hint: "pass a repository or project directory".into(),
        });
    }
    Ok(canonical)
}

fn trust_path(paths: &Paths) -> PathBuf {
    paths.state_dir.join("trust.toml")
}

fn load_trust(paths: &Paths) -> Result<TrustFile> {
    let path = trust_path(paths);
    if !path.exists() {
        return Ok(TrustFile {
            version: TRUST_VERSION,
            projects: BTreeSet::new(),
        });
    }
    let text = fs::read_to_string(&path).map_err(|source| SyncplaneError::io(&path, source))?;
    let trust: TrustFile = toml::from_str(&text).map_err(|error| SyncplaneError::InvalidInput {
        message: format!(
            "project trust state {} is malformed: {error}",
            path.display()
        ),
        hint: "repair the trust file; syncplane will not guess which projects are trusted".into(),
    })?;
    if trust.version != TRUST_VERSION {
        return Err(SyncplaneError::InvalidInput {
            message: format!("unsupported project trust state version {}", trust.version),
            hint: "use a compatible syncplane release or migrate the trust state explicitly".into(),
        });
    }
    Ok(trust)
}

fn save_trust(paths: &Paths, trust: &TrustFile) -> Result<()> {
    let path = trust_path(paths);
    let text = toml::to_string_pretty(trust).map_err(|error| SyncplaneError::Operational {
        message: format!("could not serialize project trust state: {error}"),
        hint: "report this as a Syncplane bug".into(),
    })?;
    atomic_write(&path, text.as_bytes(), Some(0o600))
}
