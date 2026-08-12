use serde::Serialize;

use crate::diagnostics::{McpdError, Result};

pub fn json(value: &impl Serialize) -> Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(|error| McpdError::Operational {
        message: format!("could not render JSON output: {error}"),
        hint: "report this as an mcpd bug".into(),
    })?;
    println!("{text}");
    Ok(())
}
