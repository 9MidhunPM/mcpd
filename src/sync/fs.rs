use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::Path};

use tempfile::NamedTempFile;

use crate::diagnostics::{Result, SyncplaneError};

pub fn ensure_safe_target_path(path: &Path, home: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(SyncplaneError::Security {
            path: path.to_path_buf(),
            message: "target path is not absolute".into(),
            hint: "use an absolute target configuration path".into(),
        });
    }
    let relative = path.strip_prefix(home).map_err(|_| SyncplaneError::Security {
        path: path.to_path_buf(),
        message: format!("target is outside home directory {}", home.display()),
        hint: "syncplane only writes target configuration below its approved user configuration root"
            .into(),
    })?;
    let mut current = home.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(SyncplaneError::Security {
                path: current,
                message: "symbolic links are not followed for target configuration".into(),
                hint:
                    "replace the symlink with a regular path; explicit symlink policy is deferred"
                        .into(),
            }),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(source) => return Err(SyncplaneError::io(&current, source)),
        }
    }
    Ok(())
}

pub fn atomic_write(path: &Path, bytes: &[u8], requested_mode: Option<u32>) -> Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(SyncplaneError::Security {
            path: path.to_path_buf(),
            message: "refusing to replace a symbolic-link file".into(),
            hint: "replace the symlink with a regular file before allowing syncplane to write it"
                .into(),
        });
    }
    let parent = path.parent().ok_or_else(|| SyncplaneError::Operational {
        message: format!("{} has no parent directory", path.display()),
        hint: "use an absolute file path".into(),
    })?;
    fs::create_dir_all(parent).map_err(|source| SyncplaneError::io(parent, source))?;
    let mode = requested_mode
        .or_else(|| {
            fs::metadata(path)
                .ok()
                .map(|m| m.permissions().mode() & 0o777)
        })
        .unwrap_or(0o600);
    let mut temp =
        NamedTempFile::new_in(parent).map_err(|source| SyncplaneError::io(parent, source))?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|source| SyncplaneError::io(temp.path(), source))?;
    temp.write_all(bytes)
        .map_err(|source| SyncplaneError::io(temp.path(), source))?;
    temp.as_file()
        .sync_all()
        .map_err(|source| SyncplaneError::io(temp.path(), source))?;
    temp.persist(path)
        .map_err(|error| SyncplaneError::io(path, error.error))?;
    fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|source| SyncplaneError::io(parent, source))?;
    Ok(())
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
