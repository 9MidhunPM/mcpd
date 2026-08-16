use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, IsTerminal, Write},
    path::PathBuf,
};

use clap::{ArgGroup, Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use serde::Serialize;
use url::Url;

use crate::{
    Paths, config,
    diagnostics::{McpdError, Result},
    import,
    model::Server,
    output, sync, targets,
};

#[derive(Debug, Parser)]
#[command(
    name = "mcpd",
    version,
    about = "Configure MCP once. Use it everywhere."
)]
pub struct Cli {
    /// Emit machine-readable JSON where the command has structured output.
    #[arg(long, global = true, conflicts_with = "quiet")]
    pub json: bool,
    /// Suppress successful human-readable output.
    #[arg(long, global = true)]
    pub quiet: bool,
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create an empty global canonical configuration without touching clients.
    Init,
    /// Add a canonical stdio or Streamable HTTP server.
    Add(AddCommand),
    /// Remove a canonical server and synchronize enabled targets.
    Remove {
        name: String,
        #[arg(long)]
        no_sync: bool,
    },
    /// List resolved canonical servers for the current project context.
    List,
    /// Show one resolved canonical server with secret references still symbolic.
    Get {
        #[arg(value_parser = config::parse_server_id)]
        name: String,
    },
    /// Discover, enable, or disable target adapters.
    Targets {
        #[command(subcommand)]
        command: Option<TargetCommand>,
    },
    /// Compare resolved canonical state with enabled targets without writing.
    Diff {
        #[arg(long, alias = "include-unmanaged")]
        all: bool,
        #[command(flatten)]
        targets: TargetFilter,
    },
    /// Synchronize enabled targets using backups and atomic writes.
    Sync {
        #[arg(long)]
        dry_run: bool,
        /// Permit deletion of previously managed target entries.
        #[arg(long)]
        allow_removals: bool,
        #[command(flatten)]
        targets: TargetFilter,
    },
    /// Import existing target MCP definitions into canonical configuration.
    Import(ImportCommand),
    /// Adopt an equivalent existing target MCP into mcpd ownership.
    Adopt(AdoptCommand),
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
    Doctor {
        #[command(flatten)]
        targets: TargetFilter,
        /// Perform explicit network reachability checks for HTTP MCP servers.
        #[arg(long)]
        network: bool,
    },
    /// Summarize configured servers, target enablement, drift, and parse health.
    Status {
        #[command(flatten)]
        targets: TargetFilter,
    },
    /// Trust, list, or revoke project-local mcpd overlays.
    Trust(TrustCommand),
    /// Watch canonical configuration and synchronize enabled targets after a debounce.
    Watch,
    /// Manage the optional user-level systemd watch service.
    Systemd {
        #[command(subcommand)]
        command: SystemdCommand,
    },
    /// Generate shell completion scripts on stdout.
    Completions { shell: Shell },
    /// Print the mcpd version.
    Version,
}

#[derive(Debug, Subcommand)]
enum SystemdCommand {
    /// Print the user unit without writing it.
    Generate,
    Install,
    Uninstall,
    Status,
}

#[derive(Debug, Args, Default)]
struct TargetFilter {
    /// Limit the command to a target; repeat for multiple targets.
    #[arg(long = "target", value_name = "TARGET")]
    targets: Vec<String>,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("trust_action")
        .args(["list", "revoke"])
        .multiple(false)
))]
struct TrustCommand {
    /// Project directory to trust (defaults to the current project).
    #[arg(value_name = "PATH", conflicts_with_all = ["list", "revoke"])]
    path: Option<PathBuf>,
    /// List trusted project directories.
    #[arg(long)]
    list: bool,
    /// Revoke trust for a project directory.
    #[arg(long, value_name = "PATH")]
    revoke: Option<PathBuf>,
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
    /// Reveal a value only with an explicit unsafe acknowledgement.
    Get {
        #[arg(value_parser = config::parse_server_id)]
        name: String,
        /// Print the raw value to stdout (unsafe for logs and terminal history).
        #[arg(long)]
        reveal: bool,
    },
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("selection").args(["server", "all"])))]
#[command(override_usage = "mcpd import (<TARGET> (<SERVER>|--all)|all) [OPTIONS]")]
struct ImportCommand {
    target: String,
    #[arg(value_parser = config::parse_server_id)]
    server: Option<String>,
    #[arg(long)]
    all: bool,
    #[arg(long)]
    dry_run: bool,
    /// Abort an --all import if any selected server cannot be imported safely.
    #[arg(long)]
    strict: bool,
    /// Limit `mcpd import all` to one or more detected targets.
    #[arg(long = "target")]
    targets: Vec<String>,
    /// Accept the deterministic first discovered definition for conflicts during `import all`.
    #[arg(long)]
    yes: bool,
    /// Import canonical configuration but defer adoption and synchronization during `import all`.
    #[arg(long)]
    no_sync: bool,
    #[arg(long = "secret", value_name = "FIELD=KEYRING_NAME", value_parser = parse_secret_mapping)]
    secrets: Vec<(String, String)>,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("adopt_selection").required(true).args(["server", "all"])))]
#[command(override_usage = "mcpd adopt <TARGET> (<SERVER>|--all) [OPTIONS]")]
struct AdoptCommand {
    target: String,
    #[arg(value_parser = config::parse_server_id)]
    server: Option<String>,
    /// Adopt every canonical server atomically; if any cannot be adopted, no ownership changes are made.
    #[arg(long)]
    all: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    replace: bool,
    #[arg(long, requires = "replace")]
    yes: bool,
}

#[derive(Serialize)]
struct StatusOutput {
    canonical_servers: usize,
    targets: Vec<TargetStatus>,
    removal_safety: Option<RemovalSafetyOutput>,
}

#[derive(Debug, Clone, Serialize)]
struct PlannedRemovalOutput {
    target: String,
    display_name: String,
    server: String,
}

#[derive(Debug, Clone, Serialize)]
struct RemovalSafetyOutput {
    canonical_empty: bool,
    missing_managed_servers: Vec<String>,
    managed_entries_at_risk: usize,
    planned_removals: Vec<PlannedRemovalOutput>,
    removals_require_explicit_authorization: bool,
}

#[derive(Serialize)]
struct TargetStatus {
    target: String,
    display_name: String,
    detected: bool,
    enabled: bool,
    status: &'static str,
    managed: usize,
    unmanaged: usize,
    pending: usize,
    diagnostic: Option<String>,
    warnings: Vec<String>,
    #[serde(skip)]
    unmanaged_servers: Vec<String>,
    compatibility: targets::Compatibility,
}

type AdapterInventory = (Vec<Box<dyn targets::TargetAdapter>>, Vec<(String, String)>);

pub fn run(cli: Cli, paths: Paths) -> Result<()> {
    match &cli.command {
        Command::Init => {
            config::init(&paths)?;
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
            let canonical = crate::resolve::load(&paths)?.config;
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
            let canonical = crate::resolve::load(&paths)?.config;
            let reports = sync::sync_enabled_targets_with_policy(
                &canonical,
                &paths,
                false,
                &sync::RemovalPolicy::AllowServers(BTreeSet::from([name.clone()])),
            )?;
            render_reports(&cli, &reports, false)
        }
        Command::List => {
            let canonical = crate::resolve::load(&paths)?.config;
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
        Command::Get { name } => {
            let canonical = crate::resolve::load(&paths)?.config;
            let server = canonical
                .servers
                .get(name)
                .ok_or_else(|| McpdError::InvalidInput {
                    message: format!("server `{name}` does not exist"),
                    hint: "run `mcpd list` to see canonical servers".into(),
                })?;
            if cli.json {
                output::json(server)
            } else if cli.quiet {
                Ok(())
            } else {
                println!(
                    "{}",
                    toml::to_string_pretty(server).map_err(|error| McpdError::Operational {
                        message: format!("could not render server: {error}"),
                        hint: "report this as an mcpd bug".into(),
                    })?
                );
                Ok(())
            }
        }
        Command::Targets { command } => match command {
            Some(TargetCommand::Enable { target }) => target_enabled(&cli, &paths, target, true),
            Some(TargetCommand::Disable { target }) => target_enabled(&cli, &paths, target, false),
            None => {
                let canonical = config::load(&paths.config)?;
                let rows = targets::adapters(&paths)?.into_iter().map(|adapter| serde_json::json!({"id":adapter.id(), "detected":adapter.detect(), "enabled":canonical.targets.get(adapter.id()).is_some_and(|v| v.enabled), "config": adapter.config_path(), "warnings": adapter.warnings()})).collect::<Vec<_>>();
                if cli.json {
                    output::json(&rows)
                } else if cli.quiet {
                    Ok(())
                } else {
                    let mut table_rows = Vec::new();
                    for adapter in targets::adapters(&paths)? {
                        table_rows.push(vec![
                            adapter.display_name().into(),
                            if adapter.detect() {
                                "✓ detected"
                            } else {
                                "! unavailable"
                            }
                            .into(),
                            if canonical
                                .targets
                                .get(adapter.id())
                                .is_some_and(|v| v.enabled)
                            {
                                "enabled".into()
                            } else {
                                "disabled".into()
                            },
                        ]);
                        render_adapter_warnings(&adapter.warnings());
                    }
                    println!("Targets");
                    output::table(&["Target", "Availability", "Configuration"], &table_rows);
                    Ok(())
                }
            }
        },
        Command::Diff { all, targets } => {
            let canonical = crate::resolve::load(&paths)?.config;
            let plans = sync::plan_selected_targets(&canonical, &paths, &targets.targets)?;
            let state_file = crate::state::load(&paths.state_dir.join("state.toml"))?;
            let safety = removal_safety_from_plans(&canonical, &state_file, &plans);
            if cli.json {
                let targets = plans
                    .iter()
                    .map(|plan| {
                        if *all {
                            serde_json::json!({
                                "target": plan.target,
                                "changes": plan.changes,
                                "inventory": plan.inventory,
                                "warnings": plan.warnings,
                            })
                        } else {
                            serde_json::json!({
                                "target": plan.target,
                                "changes": plan.changes,
                                "warnings": plan.warnings,
                            })
                        }
                    })
                    .collect::<Vec<_>>();
                output::json(&serde_json::json!({
                    "targets": targets,
                    "removal_safety": safety,
                }))
            } else if cli.quiet {
                Ok(())
            } else {
                if let Some(safety) = &safety {
                    render_removal_safety(safety);
                }
                if plans.is_empty() {
                    println!("No enabled targets.");
                    return Ok(());
                }
                if *all {
                    render_diff_matrix(&canonical, &plans);
                } else {
                    render_change_table(&plans, "Changes");
                }
                for plan in plans {
                    render_adapter_warnings(&plan.warnings);
                }
                Ok(())
            }
        }
        Command::Sync {
            dry_run,
            allow_removals,
            targets,
        } => {
            let canonical = crate::resolve::load(&paths)?.config;
            let policy = if *allow_removals {
                sync::RemovalPolicy::AllowAll
            } else {
                sync::RemovalPolicy::Deny
            };
            let batch = sync::sync_selected_targets_isolated_with_policy(
                &canonical,
                &paths,
                &targets.targets,
                *dry_run,
                &policy,
            )?;
            let state_file = crate::state::load(&paths.state_dir.join("state.toml"))?;
            let safety = removal_safety_from_reports(&canonical, &state_file, &batch.reports);
            if cli.json {
                output::json(&serde_json::json!({
                    "sync": &batch,
                    "removal_safety": &safety,
                }))?;
            } else if !batch.reports.is_empty() {
                if let Some(safety) = &safety {
                    render_removal_safety(safety);
                }
                render_reports(&cli, &batch.reports, *dry_run)?;
            } else if batch.failures.is_empty() && !cli.quiet {
                println!("No enabled targets.");
            }
            batch.into_result().map(|_| ())
        }
        Command::Import(command) => {
            if command.target == "all" {
                if command.server.is_some()
                    || command.all
                    || command.strict
                    || !command.secrets.is_empty()
                {
                    return Err(McpdError::InvalidInput {
                        message: "`mcpd import all` does not accept a server selection, --strict, or --secret".into(),
                        hint: "use `mcpd import <target> <server>` for advanced per-target imports".into(),
                    });
                }
                let report = crate::onboard::import_all(
                    &paths,
                    &command.targets,
                    command.dry_run,
                    command.yes,
                    command.no_sync,
                )?;
                return render_onboard_report(&cli, &report);
            }
            if command.server.is_none() && !command.all {
                return Err(McpdError::InvalidInput {
                    message: "target imports require <SERVER> or --all".into(),
                    hint: "use `mcpd import all` for first-time onboarding".into(),
                });
            }
            if !command.targets.is_empty() || command.yes || command.no_sync {
                return Err(McpdError::InvalidInput {
                    message: "--target, --yes, and --no-sync are only valid with `mcpd import all`"
                        .into(),
                    hint: "use `mcpd import all --help` for onboarding options".into(),
                });
            }
            let selection = command.server.iter().cloned().collect::<BTreeSet<_>>();
            let selection = (!command.all).then_some(&selection);
            let mappings = command.secrets.iter().cloned().collect();
            let report = import::import_target(
                &command.target,
                selection,
                &paths,
                command.dry_run,
                command.strict,
                mappings,
            )?;
            render_import_report(&cli, &report)
        }
        Command::Adopt(command) => {
            if command.replace && !command.yes && !command.dry_run {
                if io::stdin().is_terminal() {
                    eprint!("Replace conflicting target definitions with canonical state? [y/N] ");
                    io::stderr()
                        .flush()
                        .map_err(|source| McpdError::io("<terminal>", source))?;
                    let mut answer = String::new();
                    io::stdin()
                        .read_line(&mut answer)
                        .map_err(|source| McpdError::io("<terminal>", source))?;
                    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                        // Explicit interactive confirmation.
                    } else {
                        return Err(McpdError::Conflict {
                            message: "replacement was not confirmed".into(),
                            hint: "rerun with `--replace --yes` when intentional".into(),
                        });
                    }
                } else {
                    return Err(McpdError::InvalidInput {
                        message: "--replace requires --yes in non-interactive use".into(),
                        hint: "review the semantic diff, then rerun with `--replace --yes`".into(),
                    });
                }
            }
            let selection = command.server.iter().cloned().collect::<BTreeSet<_>>();
            let report = crate::adopt::adopt(
                &command.target,
                (!command.all).then_some(&selection),
                &paths,
                command.dry_run,
                command.replace,
            )?;
            if cli.json {
                return output::json(&report);
            }
            if !cli.quiet {
                println!(
                    "{} adoption — {}",
                    if report.dry_run {
                        "Dry-run"
                    } else {
                        "Completed"
                    },
                    target_display_name(&report.target)
                );
                let rows = report
                    .adopted
                    .iter()
                    .map(|entry| {
                        vec![
                            if entry.replaced { "replace" } else { "adopt" }.into(),
                            entry.name.clone(),
                            if entry.replaced {
                                "canonical definition will be used"
                            } else {
                                "equivalent definition"
                            }
                            .into(),
                        ]
                    })
                    .chain(report.skipped.iter().map(|entry| {
                        vec!["! skipped".into(), entry.name.clone(), entry.reason.clone()]
                    }))
                    .collect::<Vec<_>>();
                if rows.is_empty() {
                    println!("✓ Nothing needed adoption.");
                } else {
                    output::table(&["Action", "Server", "Result"], &rows);
                }
                if report.dry_run {
                    println!("No files or ownership state were modified.");
                }
            }
            Ok(())
        }
        Command::Secret { command } => run_secret_command(&cli, command, &paths),
        Command::Exec { server } => crate::execution::execute(server, &paths),
        Command::Doctor { targets, network } => {
            run_doctor(&cli, &paths, &targets.targets, *network)
        }
        Command::Status { targets } => {
            let resolved = crate::resolve::load(&paths)?;
            let canonical = resolved.config;
            let state_file = crate::state::load(&paths.state_dir.join("state.toml"))?;
            let enabled_selection = targets
                .targets
                .iter()
                .filter(|id| {
                    let id = if id.as_str() == "openchamber" {
                        "opencode"
                    } else {
                        id
                    };
                    canonical
                        .targets
                        .get(id)
                        .is_some_and(|target| target.enabled)
                })
                .map(|id| {
                    if id == "openchamber" {
                        "opencode".into()
                    } else {
                        id.clone()
                    }
                })
                .collect::<Vec<_>>();
            let batch = if targets.targets.is_empty() {
                sync::sync_selected_targets_isolated(&canonical, &paths, &[], true)?
            } else if enabled_selection.is_empty() {
                sync::SyncBatch::default()
            } else {
                sync::sync_selected_targets_isolated(&canonical, &paths, &enabled_selection, true)?
            };
            let (adapters, unavailable) = if targets.targets.is_empty() {
                adapters_with_enabled_failures(&paths, &canonical)?
            } else {
                (selected_adapters(&paths, &targets.targets)?, Vec::new())
            };
            let mut target_rows = unavailable
                .into_iter()
                .map(|(target, diagnostic)| TargetStatus {
                    display_name: target_display_name(&target).into(),
                    compatibility: targets::Compatibility {
                        adapter_version: 0,
                        native_schema: "unavailable",
                        client_version: canonical
                            .targets
                            .get(&target)
                            .and_then(|target| target.client_version.clone()),
                        status: "unavailable",
                    },
                    target,
                    detected: false,
                    enabled: true,
                    status: "invalid",
                    managed: 0,
                    unmanaged: 0,
                    pending: 0,
                    diagnostic: Some(diagnostic),
                    warnings: Vec::new(),
                    unmanaged_servers: Vec::new(),
                })
                .collect::<Vec<_>>();
            for adapter in adapters {
                let enabled = canonical
                    .targets
                    .get(adapter.id())
                    .is_some_and(|v| v.enabled);
                let names_result = adapter.server_names();
                let names = names_result.as_deref().unwrap_or_default();
                let managed_names = state_file
                    .targets
                    .get(adapter.id())
                    .filter(|state| {
                        state.config_path == adapter.config_path()
                            && state.adapter_version == adapter.adapter_version()
                    })
                    .map(|state| state.managed.keys().collect::<BTreeSet<_>>())
                    .unwrap_or_default();
                let pending = batch
                    .reports
                    .iter()
                    .find(|report| report.target == adapter.id())
                    .map_or(0, |report| report.changes.len());
                let sync_failure = batch
                    .failures
                    .iter()
                    .find(|failure| failure.target == adapter.id());
                let diagnostic = names_result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .or_else(|| sync_failure.map(|failure| failure.error.clone()));
                let unmanaged_servers = names
                    .iter()
                    .filter(|name| !managed_names.contains(name))
                    .map(|name| (*name).clone())
                    .collect::<Vec<_>>();
                target_rows.push(TargetStatus {
                    target: adapter.id().into(),
                    display_name: adapter.display_name().into(),
                    detected: adapter.detect(),
                    enabled,
                    status: if diagnostic.is_some() {
                        "invalid"
                    } else if !enabled {
                        "disabled"
                    } else if pending == 0 {
                        "synced"
                    } else {
                        "drifted"
                    },
                    managed: managed_names.len(),
                    unmanaged: unmanaged_servers.len(),
                    pending,
                    diagnostic,
                    warnings: adapter.warnings(),
                    unmanaged_servers,
                    compatibility: adapter.compatibility(
                        canonical
                            .targets
                            .get(adapter.id())
                            .and_then(|target| target.client_version.as_deref()),
                    ),
                });
            }
            let status = StatusOutput {
                canonical_servers: canonical.servers.len(),
                targets: target_rows,
                removal_safety: removal_safety_from_reports(
                    &canonical,
                    &state_file,
                    &batch.reports,
                ),
            };
            if cli.json {
                output::json(&status)
            } else if cli.quiet {
                Ok(())
            } else {
                if let Some(safety) = &status.removal_safety {
                    render_removal_safety(safety);
                }
                render_status(&status);
                Ok(())
            }
        }
        Command::Trust(command) => run_trust_command(&cli, command, &paths),
        Command::Watch => crate::watch::run(&paths),
        Command::Systemd { command } => match command {
            SystemdCommand::Generate => {
                let unit = crate::systemd::generate()?;
                message(&cli, unit)
            }
            SystemdCommand::Install => {
                let path = crate::systemd::install(&paths)?;
                crate::systemd::reload_user_manager()?;
                message(
                    &cli,
                    format!(
                        "installed user service {}\nRun `systemctl --user enable --now mcpd-watch.service` to start it.",
                        path.display()
                    ),
                )
            }
            SystemdCommand::Uninstall => {
                let removed = crate::systemd::uninstall(&paths)?;
                crate::systemd::reload_user_manager()?;
                message(
                    &cli,
                    if removed {
                        "uninstalled mcpd user service".into()
                    } else {
                        "mcpd user service is not installed".into()
                    },
                )
            }
            SystemdCommand::Status => {
                let (path, installed) = crate::systemd::status(&paths)?;
                message(
                    &cli,
                    format!(
                        "{}\t{}",
                        if installed {
                            "installed"
                        } else {
                            "not installed"
                        },
                        path.display()
                    ),
                )
            }
        },
        Command::Completions { shell } => {
            clap_complete::generate(*shell, &mut Cli::command(), "mcpd", &mut std::io::stdout());
            Ok(())
        }
        Command::Version => message(&cli, format!("mcpd {}", env!("CARGO_PKG_VERSION"))),
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
        SecretCommand::Get { name, reveal } => {
            if !reveal {
                return Err(McpdError::InvalidInput {
                    message: format!("refusing to reveal secret `{name}` without `--reveal`"),
                    hint: format!("run `mcpd secret get {name} --reveal` only in a safe terminal"),
                });
            }
            if cli.json || cli.quiet {
                return Err(McpdError::InvalidInput {
                    message: "secret reveal is unavailable with --json or --quiet".into(),
                    hint: "run the explicit reveal command without machine-output flags".into(),
                });
            }
            let value = store.get(name)?.ok_or_else(|| McpdError::InvalidInput {
                message: format!("secret `{name}` does not exist"),
                hint: "run `mcpd secret list` to see registered secret names".into(),
            })?;
            eprintln!(
                "Warning: revealing `{name}` may expose it in terminal scrollback or captured output."
            );
            println!("{}", value.expose());
            Ok(())
        }
    }
}

fn run_doctor(cli: &Cli, paths: &Paths, selected: &[String], network: bool) -> Result<()> {
    let resolved = crate::resolve::load(paths)?;
    let canonical = resolved.config;
    let state_file = crate::state::load(&paths.state_dir.join("state.toml"))?;
    let store = crate::secrets::SecretStore::discover(paths)?;
    let checks = crate::secrets::referenced_names(&canonical)
        .into_iter()
        .map(|name| store.contains(&name).map(|present| (name, present)))
        .collect::<Result<Vec<_>>>()?;
    let (adapters, unavailable) = if selected.is_empty() {
        adapters_with_enabled_failures(paths, &canonical)?
    } else {
        (selected_adapters(paths, selected)?, Vec::new())
    };
    let mut target_checks = unavailable
        .into_iter()
        .map(|(target, diagnostic)| {
            serde_json::json!({
                "target": target,
                "display_name": target_display_name(&target),
                "detected": false,
                "config": null,
                "valid": false,
                "servers": 0,
                "diagnostic": diagnostic,
                "warnings": [],
                "compatibility": {
                    "adapter_version": 0,
                    "native_schema": "unavailable",
                    "client_version": canonical.targets.get(&target).and_then(|target| target.client_version.as_deref()),
                    "status": "unavailable",
                },
            })
        })
        .collect::<Vec<_>>();
    target_checks.extend(adapters
        .into_iter()
        .map(|adapter| {
            let result = adapter.server_names();
            let warnings = adapter.warnings();
            let runtime_diagnostic = result.as_ref().ok().and_then(|_| {
                match adapter.doctor_diagnostic(&canonical, state_file.targets.get(adapter.id())) {
                    Ok(diagnostic) => diagnostic,
                    Err(error) => Some(error.to_string()),
                }
            });
            serde_json::json!({
                "target": adapter.id(),
                "display_name": adapter.display_name(),
                "detected": adapter.detect(),
                "config": adapter.config_path(),
                "valid": result.is_ok() && runtime_diagnostic.is_none(),
                "servers": result.as_ref().map_or(0, Vec::len),
                "diagnostic": result.err().map(|error| error.to_string()).or(runtime_diagnostic),
                "warnings": warnings,
                "compatibility": adapter.compatibility(
                    canonical.targets.get(adapter.id()).and_then(|target| target.client_version.as_deref())
                ),
            })
        })
        .collect::<Vec<_>>());
    let command_checks = canonical
        .servers
        .iter()
        .filter_map(|(name, server)| match server {
            Server::Stdio { command, .. } => Some(serde_json::json!({
                "server": name,
                "command": command,
                "available": executable_available(command),
            })),
            Server::Http { .. } => None,
        })
        .collect::<Vec<_>>();
    let network_checks = if network {
        canonical
            .servers
            .iter()
            .filter_map(|(name, server)| match server {
                Server::Http { url, .. } => Some(check_http_reachability(name, url)),
                Server::Stdio { .. } => None,
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    #[cfg(unix)]
    let state_permissions = std::fs::metadata(&paths.state_dir).ok().map(|metadata| {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    });
    #[cfg(not(unix))]
    let state_permissions: Option<u32> = None;
    let systemd_available = executable_available("systemctl");
    let sync_lock = sync::lock_status(paths)?;
    if cli.json {
        let secrets = checks
            .iter()
            .map(|(name, present)| serde_json::json!({"secret": name, "present": present}))
            .collect::<Vec<_>>();
        output::json(&serde_json::json!({
            "project": resolved.project.as_ref().map(|project| serde_json::json!({
                "root": project.root,
                "overlay": project.overlay,
                "overlay_present": project.present,
                "trusted": project.trusted,
            })),
            "secrets": secrets,
            "targets": target_checks,
            "commands": command_checks,
            "network_checks_requested": network,
            "network": network_checks,
            "state_directory": {
                "path": paths.state_dir,
                "permissions": state_permissions,
                "private": state_permissions.is_none_or(|mode| mode & 0o077 == 0),
            },
            "systemd_available": systemd_available,
            "sync_lock": sync_lock,
        }))
    } else if cli.quiet {
        Ok(())
    } else {
        if let Some(project) = &resolved.project {
            println!(
                "Project overlay: {}{}",
                if project.present {
                    project.overlay.display().to_string()
                } else {
                    "not present".into()
                },
                if project.present {
                    if project.trusted {
                        " (trusted)"
                    } else {
                        " (ignored: untrusted)"
                    }
                } else {
                    ""
                }
            );
        }
        println!("Secrets");
        let secret_rows = checks
            .iter()
            .map(|(name, present)| {
                vec![
                    name.clone(),
                    if *present { "✓ present" } else { "! missing" }.into(),
                ]
            })
            .collect::<Vec<_>>();
        if secret_rows.is_empty() {
            println!("✓ No canonical secret references.");
        } else {
            output::table(&["Secret reference", "Status"], &secret_rows);
        }
        println!("\nCommands");
        let command_rows = command_checks
            .iter()
            .map(|command| {
                vec![
                    command["server"].as_str().unwrap_or("unknown").into(),
                    command["command"].as_str().unwrap_or("unknown").into(),
                    if command["available"].as_bool() == Some(true) {
                        "✓ available"
                    } else {
                        "! missing"
                    }
                    .into(),
                ]
            })
            .collect::<Vec<_>>();
        if command_rows.is_empty() {
            println!("✓ No stdio commands configured.");
        } else {
            output::table(&["Server", "Command", "Status"], &command_rows);
        }
        if network {
            println!("\nNetwork");
            if network_checks.is_empty() {
                println!("ok no HTTP servers configured");
            }
            for check in network_checks {
                println!(
                    "{} {}{}",
                    if check["reachable"].as_bool() == Some(true) {
                        "ok"
                    } else {
                        "unreachable"
                    },
                    check["server"].as_str().unwrap_or("unknown"),
                    check["diagnostic"]
                        .as_str()
                        .map_or(String::new(), |diagnostic| format!(" ({diagnostic})"))
                );
            }
        }
        println!(
            "{} state directory permissions{}",
            if state_permissions.is_none_or(|mode| mode & 0o077 == 0) {
                "ok"
            } else {
                "unsafe"
            },
            state_permissions.map_or(String::new(), |mode| format!(" ({mode:o})"))
        );
        println!(
            "{} systemd user tooling",
            if systemd_available {
                "ok"
            } else {
                "unavailable"
            }
        );
        println!("{sync_lock} sync lock");
        println!("\nTargets");
        let target_rows = target_checks
            .iter()
            .map(|target| {
                vec![
                    target["display_name"].as_str().unwrap_or("unknown").into(),
                    if target["valid"].as_bool() == Some(true) {
                        "✓ valid"
                    } else {
                        "! invalid"
                    }
                    .into(),
                    target["servers"].as_u64().unwrap_or(0).to_string(),
                ]
            })
            .collect::<Vec<_>>();
        output::table(&["Target", "Status", "Servers"], &target_rows);
        for target in target_checks {
            if let Some(diagnostic) = target["diagnostic"].as_str() {
                println!(
                    "! {}: {diagnostic}",
                    target["display_name"].as_str().unwrap_or("unknown")
                );
            }
            if target["target"].as_str() == Some("opencode") {
                if let Some(config) = target["config"].as_str() {
                    println!("  active OpenCode config: {config}");
                }
            }
            if let Some(warnings) = target["warnings"].as_array() {
                render_adapter_warnings(
                    &warnings
                        .iter()
                        .filter_map(|warning| warning.as_str().map(str::to_owned))
                        .collect::<Vec<_>>(),
                );
            }
        }
        Ok(())
    }
}

fn check_http_reachability(name: &str, url: &Url) -> serde_json::Value {
    use std::{net::ToSocketAddrs, time::Duration};

    let port = url.port_or_known_default();
    let result = url
        .host_str()
        .zip(port)
        .ok_or_else(|| "URL has no host or known port".to_owned())
        .and_then(|(host, port)| {
            (host, port)
                .to_socket_addrs()
                .map_err(|error| format!("DNS resolution failed: {error}"))?
                .next()
                .ok_or_else(|| "DNS resolution returned no addresses".to_owned())
        })
        .and_then(|address| {
            std::net::TcpStream::connect_timeout(&address, Duration::from_secs(3))
                .map(|_| ())
                .map_err(|error| format!("connection failed: {error}"))
        });
    serde_json::json!({
        "server": name,
        "url": url,
        "reachable": result.is_ok(),
        "diagnostic": result.err(),
    })
}

fn executable_available(command: &str) -> bool {
    let path = std::path::Path::new(command);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .any(|path| path.join(command).is_file())
}

fn selected_adapters(
    paths: &Paths,
    selected: &[String],
) -> Result<Vec<Box<dyn targets::TargetAdapter>>> {
    if selected.is_empty() {
        return targets::adapters(paths);
    }
    let mut unique = BTreeSet::new();
    let mut adapters = Vec::new();
    for id in selected {
        let id = if id == "openchamber" {
            "opencode"
        } else {
            id.as_str()
        };
        if unique.insert(id) {
            adapters.push(targets::adapter(id, paths)?);
        }
    }
    Ok(adapters)
}

fn adapters_with_enabled_failures(
    paths: &Paths,
    canonical: &crate::model::CanonicalConfig,
) -> Result<AdapterInventory> {
    let mut adapters = targets::adapters(paths)?;
    let mut seen = adapters
        .iter()
        .map(|adapter| adapter.id().to_owned())
        .collect::<BTreeSet<_>>();
    let mut unavailable = Vec::new();
    for id in canonical
        .targets
        .iter()
        .filter(|(_, target)| target.enabled)
        .map(|(id, _)| id)
    {
        if !seen.insert(id.clone()) {
            continue;
        }
        match targets::adapter(id, paths) {
            Ok(adapter) => adapters.push(adapter),
            Err(error) => unavailable.push((id.clone(), error.to_string())),
        }
    }
    Ok((adapters, unavailable))
}

fn run_trust_command(cli: &Cli, command: &TrustCommand, paths: &Paths) -> Result<()> {
    if command.list {
        let projects = crate::resolve::trusted_projects(paths)?;
        if cli.json {
            return output::json(&projects);
        }
        if !cli.quiet {
            if projects.is_empty() {
                println!("No trusted projects.");
            } else {
                for project in projects {
                    println!("{}", project.display());
                }
            }
        }
        return Ok(());
    }
    if let Some(project) = &command.revoke {
        let (project, removed) = crate::resolve::revoke(paths, project)?;
        return message(
            cli,
            if removed {
                format!("revoked trust for {}", project.display())
            } else {
                format!("{} was not trusted", project.display())
            },
        );
    }
    let project = match &command.path {
        Some(path) => path.clone(),
        None => {
            crate::resolve::discover_project_root()?.ok_or_else(|| McpdError::InvalidInput {
                message: "could not find a project from the current directory".into(),
                hint: "run inside a repository or pass `mcpd trust PATH`".into(),
            })?
        }
    };
    let project = crate::resolve::trust(paths, &project)?;
    message(cli, format!("trusted project {}", project.display()))
}

fn render_import_report(cli: &Cli, report: &import::ImportReport) -> Result<()> {
    if cli.json {
        return output::json(report);
    }
    if cli.quiet {
        return Ok(());
    }
    render_adapter_warnings(&report.warnings);
    println!(
        "{} import — {}",
        if report.dry_run {
            "Dry-run"
        } else {
            "Completed"
        },
        target_display_name(&report.target)
    );
    let rows = report
        .imported
        .iter()
        .map(|entry| {
            vec![
                "+ import".into(),
                entry.name.clone(),
                entry.transport.to_owned(),
            ]
        })
        .chain(
            report
                .skipped
                .iter()
                .map(|entry| vec!["! skipped".into(), entry.name.clone(), entry.reason.clone()]),
        )
        .collect::<Vec<_>>();
    if rows.is_empty() {
        println!("✓ Nothing available to import.");
    } else {
        output::table(&["Action", "Server", "Details"], &rows);
    }
    if !report.migrated_secrets.is_empty() {
        println!("\nSecrets");
        for name in &report.migrated_secrets {
            println!(
                "  {} {name}",
                if report.dry_run { "would store" } else { "✓" }
            );
        }
    }
    if report.dry_run {
        println!("No files or ownership state were modified.");
    } else {
        println!("Target configuration was not modified.");
    }
    Ok(())
}

fn render_onboard_report(cli: &Cli, report: &crate::onboard::OnboardReport) -> Result<()> {
    if cli.json {
        return output::json(report);
    }
    if cli.quiet {
        return Ok(());
    }
    println!(
        "{} onboarding review",
        if report.dry_run { "Dry-run" } else { "mcpd" }
    );
    let rows = report
        .entries
        .iter()
        .map(|entry| {
            vec![
                entry.name.clone(),
                entry.found_in.join(", "),
                entry.transport.into(),
                entry.status.clone(),
            ]
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        println!("No readable MCP configurations were found.");
    } else {
        output::table(&["Server", "Found in", "Type", "Status"], &rows);
    }
    for entry in &report.unsupported {
        println!("! Unsupported: {entry}");
    }
    if !report.skipped_conflicts.is_empty() {
        println!(
            "! Conflicts skipped: {}. Rerun interactively to choose a source, or use --yes to accept the first discovered definition.",
            report.skipped_conflicts.join(", ")
        );
    }
    if report.dry_run {
        println!(
            "Would import {} canonical server(s) and enable {} target(s).",
            report.imported.len(),
            report.enabled_targets.len()
        );
        println!("No files, keyring entries, or ownership state were modified.");
    } else {
        println!(
            "✓ Canonical: {} server(s); enabled: {}.",
            report.canonical_servers,
            if report.enabled_targets.is_empty() {
                "none".into()
            } else {
                report.enabled_targets.join(", ")
            }
        );
        if let Some(sync) = &report.sync {
            let pending = sync
                .reports
                .iter()
                .map(|item| item.changes.len())
                .sum::<usize>();
            println!(
                "✓ Verification: {} sync action(s), {} isolated failure(s).",
                pending,
                sync.failures.len()
            );
        } else {
            println!("Synchronization deferred (--no-sync).");
        }
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
    targets::adapter(target, paths)?;
    config::set_target_enabled(&paths.config, target, enabled)?;
    message(
        cli,
        format!(
            "{} `{target}`",
            if enabled { "enabled" } else { "disabled" }
        ),
    )
}

fn removal_safety_from_plans(
    canonical: &crate::model::CanonicalConfig,
    state: &crate::state::StateFile,
    plans: &[crate::targets::TargetPlan],
) -> Option<RemovalSafetyOutput> {
    let removals = plans
        .iter()
        .flat_map(|plan| {
            plan.changes
                .iter()
                .filter(|change| change.kind == crate::targets::ChangeKind::Remove)
                .map(|change| PlannedRemovalOutput {
                    target: plan.target.clone(),
                    display_name: target_display_name(&plan.target).into(),
                    server: change.server.clone(),
                })
        })
        .collect();
    build_removal_safety(canonical, state, removals)
}

fn removal_safety_from_reports(
    canonical: &crate::model::CanonicalConfig,
    state: &crate::state::StateFile,
    reports: &[sync::SyncReport],
) -> Option<RemovalSafetyOutput> {
    let removals = reports
        .iter()
        .flat_map(|report| {
            report
                .changes
                .iter()
                .filter(|change| change.kind == crate::targets::ChangeKind::Remove)
                .map(|change| PlannedRemovalOutput {
                    target: report.target.clone(),
                    display_name: target_display_name(&report.target).into(),
                    server: change.server.clone(),
                })
        })
        .collect();
    build_removal_safety(canonical, state, removals)
}

fn build_removal_safety(
    canonical: &crate::model::CanonicalConfig,
    state: &crate::state::StateFile,
    planned_removals: Vec<PlannedRemovalOutput>,
) -> Option<RemovalSafetyOutput> {
    let managed = state.managed_server_names();
    let missing_managed_servers = managed
        .iter()
        .filter(|name| !canonical.servers.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    let missing = missing_managed_servers.iter().collect::<BTreeSet<_>>();
    let managed_entries_at_risk = state
        .targets
        .values()
        .flat_map(|target| target.managed.keys())
        .filter(|name| missing.contains(name))
        .count();
    if missing_managed_servers.is_empty() && planned_removals.is_empty() {
        return None;
    }
    Some(RemovalSafetyOutput {
        canonical_empty: canonical.servers.is_empty() && !managed.is_empty(),
        missing_managed_servers,
        managed_entries_at_risk,
        removals_require_explicit_authorization: !planned_removals.is_empty(),
        planned_removals,
    })
}

fn render_removal_safety(safety: &RemovalSafetyOutput) {
    if safety.canonical_empty {
        eprintln!(
            "WARNING: canonical config has no servers, but ownership state still tracks {} managed target entry/entries for: {}",
            safety.managed_entries_at_risk,
            safety.missing_managed_servers.join(", ")
        );
    } else if !safety.missing_managed_servers.is_empty() {
        eprintln!(
            "WARNING: canonical config is missing previously managed server(s): {}",
            safety.missing_managed_servers.join(", ")
        );
    }
    if !safety.planned_removals.is_empty() {
        eprintln!("WARNING: planned managed removals:");
        for removal in &safety.planned_removals {
            eprintln!("  REMOVE {}: {}", removal.display_name, removal.server);
        }
        eprintln!(
            "These removals are blocked by default; use `mcpd sync --allow-removals` only after verifying every entry."
        );
    }
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
    let rows = reports
        .iter()
        .flat_map(|report| {
            if report.changes.is_empty() {
                vec![vec![
                    target_display_name(&report.target).into(),
                    "✓ synced".into(),
                    "—".into(),
                ]]
            } else {
                report
                    .changes
                    .iter()
                    .map(|change| {
                        vec![
                            target_display_name(&report.target).into(),
                            change_action(&change.kind).into(),
                            change.server.clone(),
                        ]
                    })
                    .collect()
            }
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        if dry_run {
            "Planned changes"
        } else {
            "Synchronization"
        }
    );
    output::table(&["Target", "Action", "Server"], &rows);
    let changes = reports
        .iter()
        .map(|report| report.changes.len())
        .sum::<usize>();
    if dry_run {
        println!(
            "Planned {changes} action(s) across {} target(s).",
            reports.len()
        );
        println!("No files were modified.");
    } else {
        println!(
            "✓ Applied {changes} action(s) across {} target(s).",
            reports.len()
        );
    }
    for report in reports {
        render_adapter_warnings(&report.warnings);
    }
    Ok(())
}

fn render_adapter_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("Warning: {warning}");
    }
}

fn target_display_name(target: &str) -> &str {
    targets::display_name(target)
}

fn render_status(status: &StatusOutput) {
    let invalid = status
        .targets
        .iter()
        .filter(|target| target.status == "invalid")
        .count();
    let drifted = status
        .targets
        .iter()
        .filter(|target| target.status == "drifted")
        .count();
    let enabled = status
        .targets
        .iter()
        .filter(|target| target.enabled)
        .count();
    let health = if invalid > 0 {
        "! needs attention"
    } else if drifted > 0 {
        "~ changes pending"
    } else {
        "✓ healthy"
    };
    println!(
        "Overall: {health} — {} canonical server(s), {enabled} enabled target(s)",
        status.canonical_servers
    );
    let rows = status
        .targets
        .iter()
        .map(|target| {
            vec![
                target.display_name.clone(),
                status_label(target).into(),
                target.managed.to_string(),
                target.unmanaged.to_string(),
                target.pending.to_string(),
            ]
        })
        .collect::<Vec<_>>();
    output::table(
        &["Target", "Status", "Managed", "Unmanaged", "Pending"],
        &rows,
    );
    let unmanaged = status
        .targets
        .iter()
        .flat_map(|target| {
            target
                .unmanaged_servers
                .iter()
                .map(|server| vec![target.display_name.clone(), format!("+ {server}")])
        })
        .collect::<Vec<_>>();
    if !unmanaged.is_empty() {
        println!("\nUnmanaged servers");
        output::table(&["Target", "Server"], &unmanaged);
    }
    for target in &status.targets {
        if let Some(diagnostic) = &target.diagnostic {
            println!("! {}: {diagnostic}", target.display_name);
        }
        render_adapter_warnings(&target.warnings);
    }
}

fn status_label(target: &TargetStatus) -> &'static str {
    match target.status {
        "synced" => "✓ synced",
        "drifted" => "~ drifted",
        "disabled" => "- disabled",
        _ => "! invalid",
    }
}

fn change_action(kind: &crate::targets::ChangeKind) -> &'static str {
    match kind {
        crate::targets::ChangeKind::Add => "+ add",
        crate::targets::ChangeKind::Update => "~ update",
        crate::targets::ChangeKind::Remove => "- remove",
        crate::targets::ChangeKind::DriftRepair => "~ repair",
    }
}

fn render_change_table(plans: &[crate::targets::TargetPlan], title: &str) {
    println!("{title}");
    let rows = plans
        .iter()
        .flat_map(|plan| {
            if plan.changes.is_empty() {
                vec![vec![
                    target_display_name(&plan.target).into(),
                    "✓ synced".into(),
                    "—".into(),
                ]]
            } else {
                plan.changes
                    .iter()
                    .map(|change| {
                        vec![
                            target_display_name(&plan.target).into(),
                            change_action(&change.kind).into(),
                            change.server.clone(),
                        ]
                    })
                    .collect()
            }
        })
        .collect::<Vec<_>>();
    output::table(&["Target", "Action", "Server"], &rows);
}

fn render_diff_matrix(
    canonical: &crate::model::CanonicalConfig,
    plans: &[crate::targets::TargetPlan],
) {
    let mut servers = canonical.servers.keys().cloned().collect::<BTreeSet<_>>();
    for plan in plans {
        servers.extend(plan.inventory.managed_synchronized.iter().cloned());
        servers.extend(
            plan.inventory
                .managed_drift
                .iter()
                .map(|change| change.server.clone()),
        );
        servers.extend(plan.inventory.only_in_target.iter().cloned());
        servers.extend(plan.inventory.only_in_mcpd.iter().cloned());
    }
    let mut headers = vec!["Server".to_owned()];
    headers.extend(
        plans
            .iter()
            .map(|plan| target_display_name(&plan.target).to_owned()),
    );
    let rows = servers
        .into_iter()
        .map(|server| {
            let mut row = vec![server.clone()];
            for plan in plans {
                let disabled = canonical
                    .targets
                    .get(&plan.target)
                    .and_then(|target| target.servers.get(&server))
                    .is_some_and(|setting| !setting.enabled);
                let cell = if disabled {
                    "disabled"
                } else if plan.inventory.managed_synchronized.contains(&server) {
                    "✓ synced"
                } else if plan
                    .inventory
                    .managed_drift
                    .iter()
                    .any(|change| change.server == server)
                {
                    "~ drifted"
                } else if plan.inventory.only_in_target.contains(&server) {
                    "+ unmanaged"
                } else if plan.inventory.only_in_mcpd.contains(&server) {
                    "- missing"
                } else {
                    "—"
                };
                row.push(cell.into());
            }
            row
        })
        .collect::<Vec<_>>();
    println!("Server matrix");
    let header_refs = headers.iter().map(String::as_str).collect::<Vec<_>>();
    output::table(&header_refs, &rows);
    println!(
        "Legend: ✓ synced  ~ drifted  + unmanaged  - missing  disabled not selected for this target"
    );
}

fn message(cli: &Cli, text: String) -> Result<()> {
    if cli.json {
        output::json(&serde_json::json!({"message": text}))?;
    } else if !cli.quiet {
        println!("{text}");
    }
    Ok(())
}
