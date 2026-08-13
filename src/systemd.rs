use std::{fs, path::PathBuf, process::Command};

use crate::{
    Paths,
    diagnostics::{McpdError, Result},
    sync::fs::{atomic_write, ensure_safe_target_path},
};

const UNIT: &str = "mcpd-watch.service";

pub fn unit_path(paths: &Paths) -> PathBuf {
    paths.home.join(".config/systemd/user").join(UNIT)
}

pub fn install(paths: &Paths) -> Result<PathBuf> {
    let text = generate()?;
    let path = unit_path(paths);
    ensure_safe_target_path(&path, &paths.home)?;
    atomic_write(&path, text.as_bytes(), Some(0o600))?;
    Ok(path)
}

pub fn generate() -> Result<String> {
    let executable =
        std::env::current_exe().map_err(|source| McpdError::io("<current executable>", source))?;
    let executable = systemd_quote(&executable.to_string_lossy());
    Ok(format!(
        "[Unit]\nDescription=mcpd canonical MCP configuration watcher\n\n[Service]\nType=simple\nExecStart={} watch\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n",
        executable
    ))
}

fn systemd_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn uninstall(paths: &Paths) -> Result<bool> {
    let path = unit_path(paths);
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(McpdError::io(path, source)),
    }
}

pub fn status(paths: &Paths) -> Result<(PathBuf, bool)> {
    let path = unit_path(paths);
    Ok((path.clone(), path.is_file()))
}

pub fn reload_user_manager() -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .map_err(|source| McpdError::io("systemctl", source))?;
    if status.success() {
        Ok(())
    } else {
        Err(McpdError::Operational {
            message: "systemctl --user daemon-reload failed".into(),
            hint: "check `systemctl --user status` and your user session bus".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn unit_install_and_uninstall_are_isolated_and_safe() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let paths = Paths {
            config: home.join(".config/mcpd/config.toml"),
            state_dir: home.join(".local/state/mcpd"),
            codex_config: home.join(".codex/config.toml"),
            home,
        };
        let path = install(&paths).unwrap();
        let unit = fs::read_to_string(&path).unwrap();
        assert!(unit.contains("ExecStart=\""));
        assert!(unit.contains(" watch\n"));
        assert_eq!(status(&paths).unwrap(), (path.clone(), true));
        assert!(uninstall(&paths).unwrap());
        assert!(!path.exists());
    }
}
