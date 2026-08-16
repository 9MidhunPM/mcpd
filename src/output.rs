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

/// Render a compact, deterministic table for human-facing commands.  We keep
/// this dependency-free and deliberately emit no ANSI sequences: output stays
/// readable when piped and automatically honours `NO_COLOR`.
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths = headers
        .iter()
        .map(|header| display_width(header))
        .collect::<Vec<_>>();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(display_width(cell));
            }
        }
    }
    render_row(
        &headers
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>(),
        &widths,
    );
    render_separator(&widths);
    for row in rows {
        render_row(row, &widths);
    }
}

fn render_row(row: &[String], widths: &[usize]) {
    print!("| ");
    for (index, width) in widths.iter().enumerate() {
        let value = row.get(index).map(String::as_str).unwrap_or("");
        print!("{value}");
        print!("{}", " ".repeat(width.saturating_sub(display_width(value))));
        if index + 1 == widths.len() {
            println!(" |");
        } else {
            print!(" | ");
        }
    }
}

fn render_separator(widths: &[usize]) {
    print!("|-");
    for (index, width) in widths.iter().enumerate() {
        print!("{}", "-".repeat(*width));
        if index + 1 == widths.len() {
            println!("-|");
        } else {
            print!("-|-");
        }
    }
}

fn display_width(value: &str) -> usize {
    value.chars().count()
}
