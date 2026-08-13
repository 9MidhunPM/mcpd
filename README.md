# mcpd

`mcpd` is a local MCP configuration control plane: configure an MCP server once, then safely project it into supported clients without deleting configuration that `mcpd` does not own.

The Linux v1.0 implementation supports global and trusted project configuration, Codex, Claude Code, Cursor, Antigravity, and OpenChamber through OpenCode. It includes stdio and Streamable HTTP servers, safe import and synchronization, JSONC-aware edits, OS-keyring secrets, runtime secret injection, declarative custom targets, discovery, watch mode, user-level systemd integration, and shell completions.

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

Install from source without root:

```sh
cargo install --locked --path .
```

Tagged Linux x86_64 releases include a checksum-verified installer:

```sh
curl -fsSL https://raw.githubusercontent.com/9MidhunPM/mcpd/v1.0.0/scripts/install.sh | sh
```

## Quick start

```sh
mcpd init
mcpd add context7 --no-sync -- npx -y @upstash/context7-mcp
mcpd targets enable codex
mcpd targets enable claude
mcpd targets enable cursor
mcpd diff --all
mcpd import codex github --dry-run
mcpd import codex github
mcpd secret set github.token
mcpd doctor
mcpd sync --dry-run
mcpd sync
mcpd status
mcpd doctor
mcpd systemd install
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

`mcpd` refuses to overwrite a same-name unmanaged target entry until that entry is explicitly imported or removed. Once an entry is owned, canonical state is authoritative and `mcpd diff` reports external drift before `mcpd sync` repairs it.

Canonical loss is treated as a recovery event, not as an empty desired state. If the canonical file is missing while ownership state still records managed servers, `mcpd init` refuses to create an empty replacement and names the servers that must be recovered. Schema rewrites must preserve every canonical and owned server, create a private canonical backup, and atomically replace the file; unsupported schema versions are never reinterpreted implicitly.

Detection and enablement are separate. `mcpd targets` can report an installed client, but `diff`, `sync`, and automatic reconciliation after `add` or `remove` operate only on targets explicitly enabled with `mcpd targets enable <target>`. With no enabled targets they report `No enabled targets.` and do not inspect or mutate client configuration.

For stdio servers, the server ID is one positional value and `--` is required before the command. IDs must match `[a-zA-Z0-9][a-zA-Z0-9._-]*`. `--no-sync` updates only canonical configuration and reports that synchronization was skipped.

Normal `mcpd diff` shows only actionable managed synchronization changes. Use `mcpd diff --all` (or `--include-unmanaged`) for a read-only inventory separated into synchronized managed servers, managed drift, target-only unmanaged servers, and canonical-only servers. Unmanaged entries are informational and are never scheduled for deletion.

Managed target removal is explicit. `status`, `diff`, and `sync --dry-run` emit a prominent warning and list every `REMOVE` when canonical definitions are missing but ownership remains. Ordinary `mcpd sync` blocks those removals. `mcpd remove SERVER` authorizes only that named removal; after an intentional `mcpd remove SERVER --no-sync` or direct canonical edit, review `mcpd diff --all` and use `mcpd sync --allow-removals` to confirm the pending deletions. This flag never authorizes changes to unmanaged entries.

Adopt an existing Codex entry explicitly with `mcpd import codex SERVER`, or inspect a bulk adoption with `mcpd import codex --all --dry-run` before running `mcpd import codex --all`. Import reads and snapshots the target but never writes it. Single-server imports are strict: a collision or native definition that cannot round-trip losslessly fails without writes. Bulk imports are best-effort by default: safely representable entries are imported and every collision or unsupported entry is listed under `Skipped`; use `mcpd import codex --all --strict` for all-or-nothing planning. Imported entries are added to canonical configuration in one batch and become mcpd-owned only after the canonical write succeeds.

Import accepts stdio `command`, `args`, `cwd`, local `env`, and `env_vars`, plus HTTP `url`, `http_headers`, and `env_http_headers`. Strong credential names such as `*_API_KEY`, `*_TOKEN`, `*_PASSWORD`, `*_SECRET`, `*_CREDENTIAL`, and `*_PRIVATE_KEY` are offered for keyring migration and receive deterministic `<server>.<FIELD>` names. Ordinary settings such as `API_URL`, `HOST`, `PORT`, and `NODE_ENV` remain canonical literals. Interactive imports confirm all detected fields once per server; non-interactive and dry-run imports use the same deterministic mapping. `--secret FIELD=NAME` remains available as an explicit override. The source target is never changed by import.

`mcpd status` gives a compact per-target overview with enabled state, synchronization state, pending change count, and managed/unmanaged counts. It is read-only and does not enable or reconcile targets.

`status`, `diff`, `sync`, and `doctor` accept repeatable target filters. `diff` and `sync` require selected targets to be enabled; `status` and `doctor` may inspect disabled targets:

```sh
mcpd status --target codex --target cursor
mcpd diff --target codex
mcpd sync --target codex --target cursor --dry-run
mcpd doctor --target claude
```

`doctor` is static by default and never starts clients or MCP servers. Use `mcpd doctor --network` to explicitly request three-second DNS/TCP reachability checks for configured HTTP endpoints.

Multi-target synchronization is failure-isolated. If one client is malformed, other selected clients are still attempted and successful writes remain committed; the command exits non-zero and reports every failed target.

## Project overlays and trust

`mcpd` discovers `<repo-root>/.mcpd/config.toml` by walking from the current directory to a Git or mcpd project root. An overlay is ignored until its canonical project directory is explicitly trusted:

```sh
cd /path/to/project
mcpd trust
mcpd trust --list
mcpd trust --revoke /path/to/project
```

Trusted overlays may add or completely replace servers, disable a global server with `enabled = false`, and override target enablement/server selection. They never mutate the global canonical file. Secret references in untrusted overlays are never resolved. See [the overlay example](examples/project-overlay.toml) and [ADR 0005](docs/architecture/0005-project-trust-and-declarative-targets.md).

Claude Code exposes three explicit target IDs so scope is never inferred:

```sh
mcpd targets enable claude          # user scope: ~/.claude.json
mcpd targets enable claude-project  # shared scope: <project>/.mcp.json
mcpd targets enable claude-local    # private project entry in ~/.claude.json
```

Project and local Claude scopes require a current trusted project. Claude's own project MCP approval remains authoritative.

## Declarative custom targets

Simple JSON/JSONC clients can be added with manifests under `~/.config/mcpd/targets/*.toml`. A v1 manifest declares a unique ID, optional detection commands, an absolute or `~/...` configuration path, and a dot-separated server object path. See [the complete example](examples/declarative-target.toml).

Declarative adapters are intentionally export-only in v1. They preserve unmanaged entries and comments, support stdio (including `mcpd exec` for secrets) and literal Streamable HTTP configuration, and reject client-specific HTTP placeholder/auth behavior rather than guessing. Complex or import-capable clients belong in compiled adapters.

Built-in JSON adapters accept JSONC comments and trailing commas. Synchronization edits only mcpd-owned server values and necessary path objects, retaining unrelated settings, unmanaged server text, and surrounding comments. Duplicate keys fail closed.

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
mcpd secret get github.token --reveal
mcpd secret delete github.token
```

`secret set` reads from a no-echo terminal prompt. Raw values are printed only by the explicit `get --reveal` form, which warns on stderr and rejects `--json`/`--quiet`; ordinary output never reveals them. Only secret names are recorded under mcpd state; values remain in the OS keyring.

For secret-bearing stdio servers, targets receive an `mcpd exec SERVER` runtime wrapper. `mcpd exec` reads the keyring at the last possible moment and injects values only into the real child process. HTTP environment references use each target's native syntax where documented; keyring-backed HTTP secrets are rejected when a client cannot resolve them without copying the value.

## Watch, systemd, and completions

Run `mcpd watch` in the foreground to synchronize enabled targets after debounced canonical-config and trusted-overlay changes. A transient parse/sync failure is logged and the watcher continues. On Linux, inspect the exact unit with `mcpd systemd generate`; `mcpd systemd install` writes the user unit and reloads the user manager. Start it explicitly with:

```sh
systemctl --user enable --now mcpd-watch.service
```

Inspect or remove it with `mcpd systemd status` and `mcpd systemd uninstall`. Generate completions with `mcpd completions bash`, `zsh`, `fish`, `elvish`, or `powershell`.

OS-keyring secret references for HTTP headers are rejected because Codex cannot resolve them without copying the value. Use a native `${env:NAME}` reference for HTTP where appropriate. mcpd does not proxy HTTP MCP traffic.

## Test path overrides

`MCPD_CONFIG`, `MCPD_STATE_DIR`, `MCPD_HOME`, `MCPD_PROJECT_ROOT`, and `MCPD_<TARGET>_CONFIG` override discovered paths. They are intended for isolated tests and advanced packaging. User target paths must remain under `MCPD_HOME`; the trusted Claude project file is confined to the trusted project root.

An optional `client_version = "..."` under a target records the client version used for compatibility diagnostics without executing client binaries during discovery. Native schema/version incompatibilities still fail inside the owning adapter before writes.

See [PRD.md](PRD.md), [SECURITY.md](SECURITY.md), [packaging guidance](docs/packaging.md), and [docs/architecture](docs/architecture) for the product and safety model.
