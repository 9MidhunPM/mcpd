# mcpd

`mcpd` is a local MCP configuration control plane: configure an MCP server once, then safely project it into supported clients without deleting configuration that `mcpd` does not own.

This repository currently implements the first Linux vertical slice. It supports the global canonical configuration, the global Codex target, OS-keyring secrets, runtime secret injection for stdio servers, read-only discovery, and explicit Codex import/adoption. Claude Code, Cursor, Antigravity, OpenChamber/OpenCode, project overlays, watch mode, and systemd integration are intentionally deferred.

## Scope

`mcpd` is deliberately narrow: it is the safe configuration control plane for
MCP client integrations. It owns a canonical server definition, records
explicit target ownership, discovers existing target entries, imports them only
on request, and synchronizes only the entries it owns.

It is not an MCP proxy, server marketplace, package installer, agent runtime,
model gateway, cloud-sync service, or a general IDE-settings manager. Existing
client configuration remains read-only unless the user explicitly enables a
target and asks `mcpd` to synchronize an entry it manages.

## Build

```sh
cargo build
cargo test --all-targets --all-features
```

Rust 1.85 or newer is required.

## Quick start

```sh
mcpd init
mcpd add context7 --no-sync -- npx -y @upstash/context7-mcp
mcpd targets enable codex
mcpd diff --all
mcpd import codex github --dry-run
mcpd import codex github
mcpd secret set github.token
mcpd doctor
mcpd sync --dry-run
mcpd sync
mcpd status
```

Add a Streamable HTTP server with:

```sh
mcpd add docs --transport http https://example.com/mcp
```

The canonical file defaults to `~/.config/mcpd/config.toml`:

```toml
version = 1

[servers.context7]
transport = "stdio"
command = "npx"
args = ["-y", "@upstash/context7-mcp"]

[targets.codex]
enabled = true

[targets.codex.servers.context7]
enabled = true
```

`mcpd` refuses to overwrite a same-name Codex entry until that entry is explicitly imported or removed. Once an entry is owned, canonical state is authoritative and `mcpd diff` reports external drift before `mcpd sync` repairs it.

Detection and enablement are separate. `mcpd targets` can report an installed client, but `diff`, `sync`, and automatic reconciliation after `add` or `remove` operate only on targets explicitly enabled with `mcpd targets enable <target>`. With no enabled targets they report `No enabled targets.` and do not inspect or mutate client configuration.

For stdio servers, the server ID is one positional value and `--` is required before the command. IDs must match `[a-zA-Z0-9][a-zA-Z0-9._-]*`. `--no-sync` updates only canonical configuration and reports that synchronization was skipped.

Normal `mcpd diff` shows only actionable managed synchronization changes. Use `mcpd diff --all` (or `--include-unmanaged`) for a read-only inventory separated into synchronized managed servers, managed drift, target-only unmanaged servers, and canonical-only servers. Unmanaged entries are informational and are never scheduled for deletion.

Adopt an existing Codex entry explicitly with `mcpd import codex SERVER`, or inspect a bulk adoption with `mcpd import codex --all --dry-run` before running `mcpd import codex --all`. Import reads and snapshots the target but never writes it. It rejects canonical name collisions and any native definition that cannot round-trip losslessly through the current canonical model. Imported entries are added to canonical configuration in one batch and become mcpd-owned only after the canonical write succeeds.

Import accepts stdio `command`, `args`, `cwd`, local `env`, and `env_vars`, plus HTTP `url`, `http_headers`, and `env_http_headers`. Strong credential names such as `*_API_KEY`, `*_TOKEN`, `*_PASSWORD`, `*_SECRET`, `*_CREDENTIAL`, and `*_PRIVATE_KEY` are offered for keyring migration and receive deterministic `<server>.<FIELD>` names. Ordinary settings such as `API_URL`, `HOST`, `PORT`, and `NODE_ENV` remain canonical literals. Interactive imports confirm all detected fields once per server; non-interactive and dry-run imports use the same deterministic mapping. `--secret FIELD=NAME` remains available as an explicit override. The source target is never changed by import.

`mcpd status` gives a compact per-target overview with enabled state, synchronization state, pending change count, and managed/unmanaged counts. It is read-only and does not enable or reconcile targets.

## Environment references

References remain symbolic in canonical configuration:

```toml
[servers.local.env]
TOKEN = { secret = "github.token" }

[servers.remote.headers]
Authorization = "${env:REMOTE_TOKEN}"
```

Store and inspect secrets without revealing them:

```sh
mcpd secret set github.token
mcpd secret list
mcpd secret check github.token
mcpd secret delete github.token
```

`secret set` reads from a no-echo terminal prompt. There is intentionally no command that prints a value. Only secret names are recorded under mcpd state; values remain in the OS keyring.

For secret-bearing stdio servers, Codex receives `command = "mcpd"` and `args = ["exec", "SERVER"]`. `mcpd exec` reads the keyring at the last possible moment and injects values only into the real child process. Environment references still become Codex `env_vars` and `env_http_headers` where native indirection is safe.

OS-keyring secret references for HTTP headers are rejected because Codex cannot resolve them without copying the value. Use a native `${env:NAME}` reference for HTTP where appropriate. mcpd does not proxy HTTP MCP traffic.

## Test path overrides

`MCPD_CONFIG`, `MCPD_STATE_DIR`, `MCPD_CODEX_CONFIG`, and `MCPD_HOME` override discovered paths. They are intended for isolated tests and advanced packaging; target paths must remain under `MCPD_HOME`.

See [PRD.md](PRD.md), [SECURITY.md](SECURITY.md), and [docs/architecture](docs/architecture) for the product and safety model.
