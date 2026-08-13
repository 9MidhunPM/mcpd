use std::{sync::mpsc, time::Duration};

use notify::{RecursiveMode, Watcher};

use crate::{
    Paths,
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
    if let Some(project) = crate::resolve::current_project(paths)?
        && project.trusted
    {
        if let Some(parent) = project.overlay.parent() {
            std::fs::create_dir_all(parent).map_err(|source| McpdError::io(parent, source))?;
            watcher
                .watch(parent, RecursiveMode::NonRecursive)
                .map_err(watch_error)?;
        }
    }
    loop {
        receiver
            .recv()
            .map_err(|error| McpdError::Operational {
                message: format!("watch channel closed: {error}"),
                hint: "restart mcpd watch".into(),
            })?
            .map_err(watch_error)?;
        while receiver.recv_timeout(Duration::from_millis(350)).is_ok() {}
        match crate::resolve::load(paths)
            .and_then(|resolved| sync::sync_enabled_targets(&resolved.config, paths, false))
        {
            Ok(reports) => {
                tracing::info!(targets = reports.len(), "synchronization completed");
            }
            Err(error) => {
                tracing::error!(%error, "watch synchronization failed; continuing to watch");
            }
        }
    }
}

fn watch_error(error: notify::Error) -> McpdError {
    McpdError::Operational {
        message: format!("filesystem watch failed: {error}"),
        hint: "check that the canonical config exists and is readable".into(),
    }
}
