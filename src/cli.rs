use std::collections::{BTreeMap, BTreeSet};

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use url::Url;

use crate::{
    Paths, config,
    diagnostics::{McpdError, Result},
    import,
    model::Server,
    output, sync,
    targets::{self, TargetAdapter, codex::CodexAdapter},
};

#[derive(Debug, Parser)]
#[command(
    name = "mcpd",
    version,
    about = "Configure MCP once. Use it everywhere."
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long, global = true)]
    pub quiet: bool,
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init,
    Add(AddCommand),
    Remove {
        name: String,
        #[arg(long)]
        no_sync: bool,
    },
    List,
    Targets {
        #[command(subcommand)]
        command: Option<TargetCommand>,
    },
    Diff {
        #[arg(long, alias = "include-unmanaged")]
        all: bool,
    },
    Sync {
        #[arg(long)]
        dry_run: bool,
    },
    /// Import existing target MCP definitions into canonical configuration.
    Import(ImportCommand),
    /// Store and inspect secret names using the operating-system keyring.
    Secret {
        #[command(subcommand)]
        command: SecretCommand,
    },
    /// Run a canonical stdio MCP server with late secret injection.
    Exec {
        #[arg(value_parser = config::parse_server_id)]
        server: String,
    },
    /// Check canonical secret references without revealing their values.
    Doctor,
    Status,
}

#[derive(Debug, Args)]
struct AddCommand {
    #[arg(value_parser = config::parse_server_id)]
    name: String,
    #[arg(long, value_enum, default_value = "stdio")]
    transport: Transport,
    #[arg(long)]
    no_sync: bool,
    #[arg(value_name = "URL")]
    url: Option<String>,
    #[arg(last = true, value_name = "COMMAND", num_args = 1..)]
    command: Vec<String>,
}

#[derive(Debug, Clone, ValueEnum)]
enum Transport {
    Stdio,
    Http,
}

#[derive(Debug, Subcommand)]
enum TargetCommand {
    Enable { target: String },
    Disable { target: String },
}

#[derive(Debug, Subcommand)]
enum SecretCommand {
    /// Prompt without echo and store a value in the OS keyring.
    Set {
        #[arg(value_parser = config::parse_server_id)]
        name: String,
    },
    /// List registered secret names. Values are never displayed.
    List,
    /// Delete a value from the OS keyring.
    Delete {
        #[arg(value_parser = config::parse_server_id)]
        name: String,
    },
    /// Report whether a named secret is present.
    Check {
        #[arg(value_parser = config::parse_server_id)]
        name: String,
    },
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("selection")
        .required(true)
        .args(["server", "all"])
))]
#[command(override_usage = "mcpd import <TARGET> (<SERVER>|--all) [OPTIONS]")]
struct ImportCommand {
    target: String,
    #[arg(value_parser = config::parse_server_id)]
    server: Option<String>,
    #[arg(long)]
    all: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long = "secret", value_name = "FIELD=KEYRING_NAME", value_parser = parse_secret_mapping)]
    secrets: Vec<(String, String)>,
}

#[derive(Serialize)]
struct StatusOutput {
    canonical_servers: usize,
    targets: Vec<TargetStatus>,
}

#[derive(Serialize)]
struct TargetStatus {
    target: &'static str,
    detected: bool,
    enabled: bool,
    status: &'static str,
    managed: usize,
    unmanaged: usize,
    pending: usize,
}

pub fn run(cli: Cli, paths: Paths) -> Result<()> {
    match &cli.command {
        Command::Init => {
            config::init(&paths.config)?;
            message(
                &cli,
                format!(
                    "mcpd initialized\nCanonical config: {}\nNo clients were modified.",
                    paths.config.display()
                ),
            )
        }
        Command::Add(command) => {
            let server = add_server(command)?;
            config::add_server(&paths.config, &command.name, &server)?;
            if command.no_sync {
                return message(
                    &cli,
                    format!(
                        "added `{}` to canonical configuration; synchronization skipped (--no-sync)",
                        command.name
                    ),
                );
            }
            let canonical = config::load(&paths.config)?;
            let reports = sync::sync_enabled_targets(&canonical, &paths, false)?;
            render_reports(&cli, &reports, false)
        }
        Command::Remove { name, no_sync } => {
            config::remove_server(&paths.config, name)?;
            if *no_sync {
                return message(
                    &cli,
                    format!(
                        "removed `{name}` from canonical configuration; synchronization skipped (--no-sync)"
                    ),
                );
            }
            let canonical = config::load(&paths.config)?;
            let reports = sync::sync_enabled_targets(&canonical, &paths, false)?;
            render_reports(&cli, &reports, false)
        }
        Command::List => {
            let canonical = config::load(&paths.config)?;
            if cli.json {
                output::json(&canonical.servers)
            } else if cli.quiet {
                Ok(())
            } else {
                if canonical.servers.is_empty() {
                    println!("No canonical MCP servers configured.");
                }
                for (name, server) in canonical.servers {
                    let transport = match server {
                        Server::Stdio { .. } => "stdio",
                        Server::Http { .. } => "http",
                    };
                    println!("{name}\t{transport}");
                }
                Ok(())
            }
        }
        Command::Targets { command } => match command {
            Some(TargetCommand::Enable { target }) => target_enabled(&cli, &paths, target, true),
            Some(TargetCommand::Disable { target }) => target_enabled(&cli, &paths, target, false),
            None => {
                let canonical = config::load(&paths.config)?;
                let adapter = CodexAdapter::new(paths.codex_config, paths.home);
                let row = serde_json::json!({"id":"codex", "detected":adapter.detect(), "enabled":canonical.targets.get("codex").is_some_and(|v| v.enabled)});
                if cli.json {
                    output::json(&row)
                } else if cli.quiet {
                    Ok(())
                } else {
                    println!(
                        "{}\tCodex\t{}",
                        if adapter.detect() {
                            "installed"
                        } else {
                            "not detected"
                        },
                        if canonical.targets.get("codex").is_some_and(|v| v.enabled) {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    );
                    Ok(())
                }
            }
        },
        Command::Diff { all } => {
            let canonical = config::load(&paths.config)?;
            let plans = sync::plan_enabled_targets(&canonical, &paths)?;
            if cli.json {
                output::json(
                    &plans
                        .iter()
                        .map(|plan| {
                            if *all {
                                serde_json::json!({
                                    "target": plan.target,
                                    "changes": plan.changes,
                                    "inventory": plan.inventory,
                                })
                            } else {
                                serde_json::json!({
                                    "target": plan.target,
                                    "changes": plan.changes,
                                })
                            }
                        })
                        .collect::<Vec<_>>(),
                )
            } else if cli.quiet {
                Ok(())
            } else {
                if plans.is_empty() {
                    println!("No enabled targets.");
                    return Ok(());
                }
                for plan in plans {
                    println!("{}", target_display_name(plan.target));
                    if *all {
                        render_inventory(&plan);
                    } else {
                        if plan.changes.is_empty() {
                            println!("  = synchronized");
                        }
                        for change in plan.changes {
                            println!("  {:?}\t{}", change.kind, change.server);
                        }
                    }
                }
                Ok(())
            }
        }
        Command::Sync { dry_run } => {
            let canonical = config::load(&paths.config)?;
            let reports = sync::sync_enabled_targets(&canonical, &paths, *dry_run)?;
            render_reports(&cli, &reports, *dry_run)
        }
        Command::Import(command) => {
            let selection = command.server.iter().cloned().collect::<BTreeSet<_>>();
            let selection = (!command.all).then_some(&selection);
            let mappings = command.secrets.iter().cloned().collect();
            let report = import::import_target(
                &command.target,
                selection,
                &paths,
                command.dry_run,
                mappings,
            )?;
            render_import_report(&cli, &report)
        }
        Command::Secret { command } => run_secret_command(&cli, command, &paths),
        Command::Exec { server } => crate::execution::execute(server, &paths),
        Command::Doctor => run_doctor(&cli, &paths),
        Command::Status => {
            let canonical = config::load(&paths.config)?;
            let adapter = targets::adapter("codex", &paths)?;
            let detected = adapter.detect();
            let enabled = canonical
                .targets
                .get("codex")
                .is_some_and(|value| value.enabled);
            let names = adapter.server_names()?;
            let state_file = crate::state::load(&paths.state_dir.join("state.toml"))?;
            let managed_names = state_file
                .targets
                .get("codex")
                .filter(|state| {
                    state.config_path == adapter.config_path()
                        && state.adapter_version == adapter.adapter_version()
                })
                .map(|state| state.managed.keys().collect::<BTreeSet<_>>())
                .unwrap_or_default();
            let managed = managed_names.len();
            let unmanaged = names
                .iter()
                .filter(|name| !managed_names.contains(name))
                .count();
            let pending = if enabled {
                sync::plan_enabled_targets(&canonical, &paths)?
                    .into_iter()
                    .find(|plan| plan.target == "codex")
                    .map_or(0, |plan| plan.changes.len())
            } else {
                0
            };
            let target_status = TargetStatus {
                target: "codex",
                detected,
                enabled,
                status: if !enabled {
                    "disabled"
                } else if pending == 0 {
                    "synced"
                } else {
                    "drifted"
                },
                managed,
                unmanaged,
                pending,
            };
            let status = StatusOutput {
                canonical_servers: canonical.servers.len(),
                targets: vec![target_status],
            };
            if cli.json {
                output::json(&status)
            } else if cli.quiet {
                Ok(())
            } else {
                println!("Canonical  {} server(s)", status.canonical_servers);
                for target in status.targets {
                    let pending = if target.pending == 0 {
                        String::new()
                    } else {
                        format!(" / {} pending", target.pending)
                    };
                    println!(
                        "{:<9} {:<9} {} managed / {} unmanaged{}",
                        target_display_name(target.target),
                        target.status,
                        target.managed,
                        target.unmanaged,
                        pending
                    );
                }
                Ok(())
            }
        }
    }
}

fn run_secret_command(cli: &Cli, command: &SecretCommand, paths: &Paths) -> Result<()> {
    let store = crate::secrets::SecretStore::discover(paths)?;
    match command {
        SecretCommand::Set { name } => {
            let value = crate::secrets::prompt_value(name)?;
            store.set(name, value)?;
            message(cli, format!("stored secret `{name}` in the OS keyring"))
        }
        SecretCommand::List => {
            let names = store.names()?;
            if cli.json {
                output::json(&names)
            } else if cli.quiet {
                Ok(())
            } else {
                if names.is_empty() {
                    println!("No mcpd secrets registered.");
                }
                for name in names {
                    println!("{name}");
                }
                Ok(())
            }
        }
        SecretCommand::Delete { name } => {
            if !store.delete(name)? {
                return Err(McpdError::InvalidInput {
                    message: format!("secret `{name}` does not exist"),
                    hint: "run `mcpd secret list` to see registered secret names".into(),
                });
            }
            message(cli, format!("deleted secret `{name}` from the OS keyring"))
        }
        SecretCommand::Check { name } => {
            if !store.contains(name)? {
                return Err(McpdError::Operational {
                    message: format!("secret `{name}` is missing"),
                    hint: format!("run `mcpd secret set {name}`"),
                });
            }
            message(cli, format!("secret `{name}` is present"))
        }
    }
}

fn run_doctor(cli: &Cli, paths: &Paths) -> Result<()> {
    let canonical = config::load(&paths.config)?;
    let store = crate::secrets::SecretStore::discover(paths)?;
    let checks = crate::secrets::referenced_names(&canonical)
        .into_iter()
        .map(|name| store.contains(&name).map(|present| (name, present)))
        .collect::<Result<Vec<_>>>()?;
    if cli.json {
        let rows = checks
            .iter()
            .map(|(name, present)| serde_json::json!({"secret": name, "present": present}))
            .collect::<Vec<_>>();
        output::json(&rows)
    } else if cli.quiet {
        Ok(())
    } else {
        if checks.is_empty() {
            println!("No canonical secret references.");
        }
        for (name, present) in checks {
            println!(
                "{} secret `{name}`",
                if present { "present" } else { "missing" }
            );
        }
        Ok(())
    }
}

fn render_import_report(cli: &Cli, report: &import::ImportReport) -> Result<()> {
    if cli.json {
        return output::json(report);
    }
    if cli.quiet {
        return Ok(());
    }
    let action = if report.dry_run {
        "would import"
    } else {
        "imported"
    };
    println!(
        "{} {} server(s) from {}",
        action,
        report.imported.len(),
        target_display_name(&report.target)
    );
    if !report.migrated_secrets.is_empty() {
        for name in &report.migrated_secrets {
            println!(
                "{} {name}",
                if report.dry_run {
                    "would store"
                } else {
                    "✓ Stored"
                }
            );
        }
    }
    for entry in &report.imported {
        if report.dry_run {
            println!("  + {}\t{}", entry.name, entry.transport);
        } else {
            println!("✓ Imported {}\t{}", entry.name, entry.transport);
        }
    }
    if !report.skipped_managed.is_empty() {
        println!(
            "skipped {} already managed server(s)",
            report.skipped_managed.len()
        );
        for name in &report.skipped_managed {
            println!("  = {name}");
        }
    }
    if report.dry_run {
        println!("No files or ownership state were modified.");
    } else if !report.imported.is_empty() {
        println!("Target configuration was not modified.");
    }
    Ok(())
}

fn parse_secret_mapping(value: &str) -> std::result::Result<(String, String), String> {
    let (field, secret) = value
        .split_once('=')
        .ok_or_else(|| "expected FIELD=KEYRING_NAME".to_owned())?;
    if field.is_empty() {
        return Err("secret field name cannot be empty".into());
    }
    crate::secrets::validate_name(secret).map_err(|error| error.to_string())?;
    Ok((field.into(), secret.into()))
}

fn add_server(command: &AddCommand) -> Result<Server> {
    match command.transport {
        Transport::Stdio => {
            if let Some(unexpected) = &command.url {
                return Err(McpdError::InvalidInput {
                    message: format!(
                        "unexpected positional value `{unexpected}` before the stdio command"
                    ),
                    hint: "use `mcpd add SERVER_ID [--no-sync] -- COMMAND [ARG ...]`".into(),
                });
            }
            let (program, args) =
                command
                    .command
                    .split_first()
                    .ok_or_else(|| McpdError::InvalidInput {
                        message: "stdio servers require a command after `--`".into(),
                        hint: "use `mcpd add SERVER_ID -- COMMAND [ARG ...]`".into(),
                    })?;
            Ok(Server::Stdio {
                command: program.clone(),
                args: args.to_vec(),
                env: BTreeMap::new(),
                secrets: BTreeMap::new(),
                cwd: None,
            })
        }
        Transport::Http => {
            if !command.command.is_empty() {
                return Err(McpdError::InvalidInput {
                    message: "HTTP servers do not accept a command after `--`".into(),
                    hint: "use `mcpd add NAME --transport http URL`".into(),
                });
            }
            let value = command
                .url
                .as_deref()
                .ok_or_else(|| McpdError::InvalidInput {
                    message: "HTTP servers require exactly one URL".into(),
                    hint: "use `mcpd add NAME --transport http URL`".into(),
                })?;
            let url = Url::parse(value).map_err(|error| McpdError::InvalidInput {
                message: format!("invalid HTTP server URL: {error}"),
                hint: "use an absolute http:// or https:// URL".into(),
            })?;
            Ok(Server::Http {
                url,
                headers: BTreeMap::new(),
                secrets: BTreeMap::new(),
            })
        }
    }
}

fn target_enabled(cli: &Cli, paths: &Paths, target: &str, enabled: bool) -> Result<()> {
    if target != "codex" {
        return Err(McpdError::TargetUnavailable {
            target: target.into(),
            message: "only the Codex adapter exists in this milestone".into(),
            hint: "supported target: codex".into(),
        });
    }
    config::set_target_enabled(&paths.config, target, enabled)?;
    message(
        cli,
        format!(
            "{} `{target}`",
            if enabled { "enabled" } else { "disabled" }
        ),
    )
}

fn render_reports(cli: &Cli, reports: &[sync::SyncReport], dry_run: bool) -> Result<()> {
    if cli.json {
        return output::json(&reports);
    }
    if cli.quiet {
        return Ok(());
    }
    if reports.is_empty() {
        println!("No enabled targets.");
        return Ok(());
    }
    for report in reports {
        if report.changes.is_empty() {
            println!(
                "= {} already synchronized",
                target_display_name(report.target)
            );
        } else {
            println!(
                "{} {}: {} change(s)",
                if dry_run { "dry-run" } else { "synchronized" },
                target_display_name(report.target),
                report.changes.len()
            );
        }
    }
    Ok(())
}

fn target_display_name(target: &str) -> &str {
    match target {
        "codex" => "Codex",
        other => other,
    }
}

fn render_inventory(plan: &crate::targets::TargetPlan) {
    println!("\nManaged (synchronized)");
    render_names('=', &plan.inventory.managed_synchronized);

    println!("\nManaged drift");
    if plan.inventory.managed_drift.is_empty() {
        println!("  (none)");
    } else {
        for change in &plan.inventory.managed_drift {
            let symbol = match change.kind {
                crate::targets::ChangeKind::Add => '+',
                crate::targets::ChangeKind::Update | crate::targets::ChangeKind::DriftRepair => '~',
                crate::targets::ChangeKind::Remove => '-',
            };
            println!("  {symbol} {}", change.server);
        }
    }

    println!("\nOnly in {} (unmanaged)", target_display_name(plan.target));
    render_names('+', &plan.inventory.only_in_target);

    println!("\nOnly in mcpd");
    render_names('-', &plan.inventory.only_in_mcpd);
}

fn render_names(symbol: char, names: &[String]) {
    if names.is_empty() {
        println!("  (none)");
    } else {
        for name in names {
            println!("  {symbol} {name}");
        }
    }
}

fn message(cli: &Cli, text: String) -> Result<()> {
    if cli.json {
        output::json(&serde_json::json!({"message": text}))?;
    } else if !cli.quiet {
        println!("{text}");
    }
    Ok(())
}
