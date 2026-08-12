use clap::Parser;
use mcpd::{Paths, cli::Cli, diagnostics::ExitCode};

fn main() {
    let cli = Cli::parse();
    let json_errors = cli.json;
    let filter = match cli.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .without_time()
        .init();

    let result = Paths::discover().and_then(|paths| mcpd::cli::run(cli, paths));
    if let Err(error) = result {
        let exit_code = ExitCode::from(&error) as i32;
        if json_errors {
            let rendered = serde_json::json!({"error": error.to_string(), "hint": error.hint(), "exit_code": exit_code});
            eprintln!(
                "{}",
                serde_json::to_string(&rendered).expect("error JSON is serializable")
            );
        } else {
            eprintln!("mcpd: {error}");
            if let Some(hint) = error.hint() {
                eprintln!("Hint: {hint}");
            }
        }
        std::process::exit(exit_code);
    }
}
