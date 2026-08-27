# Syncplane overview

Syncplane is a local-first Rust CLI for keeping one MCP environment consistent
across the clients you use. Define a server once, choose the clients that
Syncplane is allowed to manage, review the plan, and project the definition into
each client's native configuration format.

> Configure MCP once. Use it everywhere.

## Why it exists

MCP clients do not share one configuration file. A server may need to be copied
into several native formats, credentials must be handled differently, and a
client can already contain settings that belong to another tool. Manual copying
creates drift; broad rewrites risk deleting useful configuration.

Syncplane is the small control plane between those realities:

```text
                 canonical MCP environment
                             │
              explicit target enablement
                             │
       ┌─────────────┬───────┼────────┬─────────────┐
       ↓             ↓       ↓        ↓             ↓
    Claude         Cursor   Codex  Antigravity   OpenCode
    native         native   native    native       native
    config         config   config    config       config
```

It configures MCP clients; it does not proxy MCP traffic, run agents, install
arbitrary servers, or become a hosted settings service.

## The mental model

### 1. Canonical state is the source of truth

The global canonical file defaults to
`~/.config/syncplane/config.toml`. A trusted project can add an overlay at
`.syncplane/config.toml`. Canonical state describes servers and target intent;
it does not contain raw keyring secret values.

### 2. Detection is not permission

`syncplane targets` can tell you which supported clients appear to be installed.
That is informational. A target participates in `sync`, `diff`, or automatic
reconciliation only after you explicitly enable it.

### 3. Ownership is recorded outside client files

Syncplane stores managed names and normalized hashes in local state under
`~/.local/state/syncplane/`. It does not inject private metadata into a client's
configuration. Entries it does not own remain untouched and are shown as
unmanaged when you request the full inventory.

### 4. Adapters preserve native behavior

Each target adapter owns its path discovery, parser, serializer, compatibility
rules, and transport translation. Generic synchronization coordinates the work;
it does not guess how a particular client represents MCP servers.

## Supported targets

| Target | Native configuration | Built-in scope |
| --- | --- | --- |
| Claude Code | `~/.claude.json` and project `.mcp.json` | user, shared project, private local project |
| Cursor | `~/.cursor/mcp.json` | user |
| Codex | `~/.codex/config.toml` | user |
| Antigravity | `~/.gemini/config/mcp_config.json` | shared user configuration |
| OpenCode-compatible | `$XDG_CONFIG_HOME/opencode/opencode.jsonc` or `opencode.json` | user |

Paths and schemas are compatibility-sensitive. The adapters document their
current behavior in [`docs/adapters/`](adapters), and explicit environment
overrides are available for isolated tests and intentional alternate installs.

## A safe lifecycle

```text
discover → import or define → enable targets → dry-run → sync → inspect status
```

- **Discover** existing target entries without implying ownership.
- **Import** an existing definition only when you ask Syncplane to make it
  canonical, or define a new server directly.
- **Enable** the exact clients that may be changed.
- **Dry-run** to see the planned changes without writing files, state, backups,
  locks, or keyring entries.
- **Sync** each selected target using a target-local backup, temporary file, and
  atomic replacement.
- **Inspect** status and drift after the write. Ownership state is saved only
  after the target write succeeds.

## Safety contract

The important defaults are intentionally conservative:

- unmanaged target entries are preserved;
- same-name collisions require explicit import or adoption;
- managed removals require an explicit authorization path;
- malformed or ambiguous target documents fail closed;
- project overlays are ignored until their canonical project path is trusted;
- secret references stay symbolic until late runtime resolution;
- ordinary sync and diagnostics never launch clients or MCP servers;
- one failed target does not silently erase successful writes to other targets;
- repeated syncs settle into a semantic no-op when nothing changed.

Read the [security policy](../SECURITY.md), the [architecture decisions](architecture),
and the [canonical loss and explicit removals ADR](architecture/0006-canonical-loss-and-explicit-removals.md)
before automating synchronization in a production environment.

## Start here

For a fresh installation, the shortest safe path is:

```sh
syncplane import all --dry-run
syncplane import all --no-sync
syncplane targets enable codex
syncplane sync --dry-run
syncplane sync
syncplane status
```

If you are creating a new canonical environment instead, use
`syncplane init`, `syncplane add`, and then enable the targets you want to
manage. The full walkthrough, including stdio argument boundaries and project
trust, is in the [getting started guide](../README.md#quick-start).

## Read next

- [README](../README.md) — installation, commands, configuration, and examples.
- [Product requirements](../PRD.md) — product scope and roadmap.
- [Security policy](../SECURITY.md) — threat boundaries and reporting guidance.
- [Adapter development](adapter-development.md) — adapter responsibilities and fixtures.
- [Architecture decisions](architecture) — the decisions behind state, writes, secrets, trust, and recovery.
