use std::{collections::BTreeMap, os::unix::process::CommandExt, process::Command};

use crate::{
    Paths,
    diagnostics::{Result, SyncplaneError},
    model::{ConfigValue, Server},
    secrets::SecretStore,
};

pub fn execute(server_name: &str, paths: &Paths) -> Result<()> {
    let config = crate::resolve::load(paths)?.config;
    let server = config
        .servers
        .get(server_name)
        .ok_or_else(|| SyncplaneError::InvalidInput {
            message: format!("server `{server_name}` does not exist"),
            hint: "run `syncplane list` to see canonical servers".into(),
        })?;
    let Server::Stdio {
        command,
        args,
        env,
        secrets,
        cwd,
    } = server
    else {
        return Err(SyncplaneError::InvalidInput {
            message: format!("server `{server_name}` is HTTP and cannot be executed locally"),
            hint:
                "HTTP MCP servers are connected by target clients; syncplane is not an HTTP proxy"
                    .into(),
        });
    };
    if command == "syncplane" && args.first().is_some_and(|arg| arg == "exec") {
        return Err(SyncplaneError::InvalidInput {
            message: format!("server `{server_name}` recursively invokes `syncplane exec`"),
            hint: "set its canonical command to the real MCP executable".into(),
        });
    }

    let store = SecretStore::discover(paths)?;
    let resolved = resolve_environment(server_name, env, secrets, &store)?;
    let mut child = Command::new(command);
    child.args(args).envs(resolved);
    if let Some(cwd) = cwd {
        child.current_dir(cwd);
    }
    let source = child.exec();
    Err(SyncplaneError::Operational {
        message: format!("could not execute stdio MCP server `{server_name}`: {source}"),
        hint: "check that the configured command exists and is executable".into(),
    })
}

fn resolve_environment(
    server_name: &str,
    env: &BTreeMap<String, ConfigValue>,
    legacy_secrets: &BTreeMap<String, String>,
    store: &SecretStore,
) -> Result<BTreeMap<String, String>> {
    let mut resolved = BTreeMap::new();
    for (name, value) in env {
        let value = if let Some(secret) = value.secret_name() {
            store
                .get(secret)?
                .ok_or_else(|| missing_secret(server_name, secret))?
                .expose()
                .to_owned()
        } else {
            match value {
                ConfigValue::Literal(value) => {
                    resolve_environment_reference(server_name, name, value)?
                }
                ConfigValue::Secret { .. } => {
                    return Err(SyncplaneError::Operational {
                        message: format!(
                            "secret reference for server `{server_name}` was not resolved"
                        ),
                        hint: "report this as a Syncplane bug".into(),
                    });
                }
            }
        };
        resolved.insert(name.clone(), value);
    }
    for (name, secret) in legacy_secrets {
        let value = store
            .get(secret)?
            .ok_or_else(|| missing_secret(server_name, secret))?;
        resolved.insert(name.clone(), value.expose().to_owned());
    }
    Ok(resolved)
}

fn resolve_environment_reference(server: &str, field: &str, value: &str) -> Result<String> {
    if let Some(name) = value
        .strip_prefix("${env:")
        .and_then(|value| value.strip_suffix('}'))
    {
        return std::env::var(name).map_err(|_| SyncplaneError::Operational {
            message: format!(
                "environment variable `{name}` required by server `{server}` is not set"
            ),
            hint: format!("set `{name}` before running `syncplane exec {server}`"),
        });
    }
    if value.contains("${env:") {
        return Err(SyncplaneError::InvalidInput {
            message: format!(
                "server `{server}` field `{field}` has an invalid environment reference"
            ),
            hint: "use `${env:NAME}` as the complete value".into(),
        });
    }
    Ok(value.into())
}

fn missing_secret(server: &str, secret: &str) -> SyncplaneError {
    SyncplaneError::Operational {
        message: format!("secret `{secret}` required by server `{server}` is missing"),
        hint: format!("run `syncplane secret set {secret}`"),
    }
}
