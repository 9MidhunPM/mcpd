use std::{sync::mpsc, time::Duration};

use notify::{RecursiveMode, Watcher};

use crate::{
    Paths, config,
    diagnostics::{McpdError, Result},
    sync,
};

pub fn run(paths: &Paths) -> Result<()> {
    let (sender, receiver) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(sender).map_err(watch_error)?;
    let parent = paths
        .config
        .parent()
        .ok_or_else(|| McpdError::Operational {
            message: format!("{} has no parent directory", paths.config.display()),
            hint: "use an absolute canonical config path".into(),
        })?;
    watcher
        .watch(parent, RecursiveMode::NonRecursive)
        .map_err(watch_error)?;
    loop {
        receiver
            .recv()
            .map_err(|error| McpdError::Operational {
                message: format!("watch channel closed: {error}"),
                hint: "restart mcpd watch".into(),
            })?
            .map_err(watch_error)?;
        while receiver.recv_timeout(Duration::from_millis(350)).is_ok() {}
        let canonical = config::load(&paths.config)?;
        sync::sync_enabled_targets(&canonical, paths, false)?;
    }
}

fn watch_error(error: notify::Error) -> McpdError {
    McpdError::Operational {
        message: format!("filesystem watch failed: {error}"),
        hint: "check that the canonical config exists and is readable".into(),
    }
}
