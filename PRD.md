# mcpd Product Requirements Document

**Status:** Implemented v1.0 baseline; forward-looking sections remain non-binding roadmap
**Project:** `mcpd`  
**License:** MIT  
**Primary platform:** Linux  
**Implementation:** Rust  
**Audience:** Developers who use multiple MCP-capable AI clients and want one reliable, secure configuration layer.

## Authoritative compatibility references

- [MCP transport specification](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)
- [Claude Code MCP documentation](https://code.claude.com/docs/en/mcp)
- [Cursor MCP documentation](https://cursor.com/docs/mcp)
- [Codex MCP documentation](https://developers.openai.com/codex/mcp)
- [Google Antigravity MCP codelab](https://codelabs.developers.google.com/gemini-mcp-agy)
- [OpenCode MCP server documentation](https://opencode.ai/docs/mcp-servers)
- [OpenChamber repository](https://github.com/openchamber/openchamber)

## 1. Executive summary

`mcpd` is a local, open-source MCP configuration control plane for developers who use multiple AI clients such as Claude Code, Cursor, Codex, Antigravity, and OpenChamber.

The core problem is simple: MCP server configuration is fragmented across applications, formats, paths, scopes, and authentication models. Adding one server can require repeating the same information in several files and remembering which client expects which schema. Manual edits also create drift, duplicate configuration, accidental secret exposure, and difficult-to-debug failures.

`mcpd` establishes a **single canonical MCP environment** and translates it into the native configuration of every supported client. It is deliberately local-first, has no cloud dependency and no telemetry, and preserves unmanaged configuration in target files.

The product promise is:

> **Configure MCP once. Use it everywhere.**

`mcpd` is not an MCP proxy, hosted MCP server, model gateway, agent runtime, marketplace, or cloud dashboard. It is a configuration and synchronization utility with optional health diagnostics.

The design is intentionally extensible so that a small v1 can grow into a serious ecosystem project without changing its core data model.

## 2. Why this exists

MCP is now supported by multiple developer-facing AI tools. The protocol defines standard local `stdio` and remote Streamable HTTP transports, while clients expose different configuration schemas and application-specific behaviors.

Examples of fragmentation include:

- Claude Code exposes MCP through its own CLI and user/project/local scopes.
- Codex uses TOML configuration under `~/.codex/config.toml` with `[mcp_servers.<name>]` entries, and its project-local behavior is influenced by project trust.
- Antigravity currently exposes a JSON MCP configuration and Google's documentation shows a central user-level MCP config under the `.gemini` configuration hierarchy.
- Cursor supports local and remote MCP transports and its configuration format differs from the TOML-based Codex model.
- OpenChamber is built around OpenCode and has an evolving MCP/settings surface, so adapter boundaries must be resilient to client changes.

The product should therefore optimize for **translation, ownership, safety, diagnostics, and evolvability**, rather than pretending every client shares one schema.

## 3. Vision

Create the best local MCP configuration manager available: boringly reliable for ordinary use, transparent when things go wrong, safe around secrets, pleasant from the terminal, and extensible enough that adding a new MCP-capable client does not require redesigning the system.

## 4. Product principles

### 4.1 Local-first

No cloud account is required. The canonical configuration lives on the user's machine.

### 4.2 One source of truth

The canonical `mcpd` configuration is authoritative for entries it owns. Targets are projections of canonical state, not independent sources of truth during normal operation.

### 4.3 Merge-safe by default

`mcpd` must never wipe unrelated configuration it does not own.

### 4.4 Secure by default

Secrets belong in the operating system credential store whenever possible. Secrets must never be printed in normal output, diffs, logs, backups, or diagnostics.

### 4.5 Explicit trust boundaries

Project-local configuration can influence MCP execution and therefore must not silently gain access to global secrets or modify global state without an explicit trust model.

### 4.6 Deterministic behavior

The same canonical input and target adapter version should result in the same projected configuration.

### 4.7 Explainability over magic

Auto-discovery may detect clients, but enabling a client and granting permissions remains an explicit decision.

### 4.8 Adapter isolation

Each target client is a replaceable adapter. Client-specific quirks stay out of the canonical model.

### 4.9 No unnecessary daemonism

The default product is a CLI. A watch process is optional. systemd integration is a convenience, not a mandatory service architecture.

### 4.10 Small core, large future

Build the foundations for profiles, richer health checks, new targets, plugin metadata, and cross-platform packaging without committing v1 to them.

## 5. Goals

### v1 goals

1. Maintain one global canonical MCP registry.
2. Support `stdio` and Streamable HTTP as first-class canonical transports.
3. Support five initial targets:
   - Claude Code
   - Cursor
   - Codex
   - Antigravity
   - OpenChamber / OpenCode-compatible MCP configuration
4. Detect supported clients without automatically modifying them.
5. Project canonical servers into enabled targets.
6. Preserve unmanaged target entries.
7. Provide explicit project-local overrides.
8. Store secrets in the OS keyring through a Rust keyring abstraction.
9. Provide safe imports from supported target configurations.
10. Provide dry-run, diff, status, doctor, and clear error reporting.
11. Provide filesystem watch mode with debouncing and serialized sync operations.
12. Provide a systemd user service generator/installer for Linux.
13. Provide shell completions.
14. Provide strong automated tests around config parsing, merging, adapters, and filesystem operations.
15. Ship as a single Rust binary with a simple installer.

### Non-goals for v1

- Hosting MCP servers.
- Acting as an MCP proxy or gateway between clients and servers.
- Cloud synchronization.
- Telemetry or usage analytics.
- A web dashboard.
- An MCP server marketplace.
- Installing arbitrary MCP packages automatically.
- Replacing OAuth implementations built into MCP clients.
- Managing unrelated client configuration such as model settings, prompts, agent personas, editor settings, or permissions unless required for MCP integration.
- Bidirectional live synchronization from target files back into the canonical registry.

## 6. Target users

### Primary user

A developer who uses two or more MCP-capable AI clients on one machine and repeatedly configures the same MCP servers.

### Secondary user

An advanced developer or team maintainer who wants reproducible, reviewable MCP configuration without forcing every client to use the same native file format.

### Open-source maintainer

A contributor who wants to add support for another AI client through an isolated adapter without modifying core sync logic.

## 7. Core UX

The product should feel like a native Unix command-line utility, not a configuration framework that happens to have a CLI.

### First-run experience

```text
$ mcpd init

mcpd initialized.
Canonical config: ~/.config/mcpd/config.toml
Detected clients:
  ✓ Claude Code
  ✓ Cursor
  ✓ Codex
  ✓ Antigravity
  ✓ OpenChamber

No clients were modified.
Run `mcpd targets enable <name>` to select targets.
```

### Add a server

```text
$ mcpd add github -- npx -y @modelcontextprotocol/server-github
```

The command updates canonical state. Unless `--no-sync` is used, enabled targets are synchronized automatically.

### Add a remote server

```text
$ mcpd add sentry --transport http https://mcp.sentry.dev/mcp
```

### Store a secret

```text
$ mcpd secret set github.token
Enter secret:
```

The value is written to the OS keyring, not to `config.toml`.

### Sync

```text
$ mcpd sync
✓ Claude Code       3 servers synchronized
✓ Cursor            3 servers synchronized
✓ Codex             3 servers synchronized
✓ Antigravity       3 servers synchronized
✓ OpenChamber       3 servers synchronized
```

### Inspect state

```text
$ mcpd status

Canonical
  5 servers
  3 stdio
  2 HTTP
  1 server with secrets

Targets
  ✓ Claude Code     synchronized
  ✓ Cursor          synchronized
  ! Codex           drift detected
  ✓ Antigravity     synchronized
  ○ OpenChamber     disabled
```

### Dry run

```text
$ mcpd sync --dry-run
```

must show the planned changes without writing anything.

## 8. Canonical configuration model

Canonical configuration uses TOML because it is readable, strongly structured, and maps naturally onto Rust's serde ecosystem.

Default location:

```text
~/.config/mcpd/config.toml
```

Project-local override:

```text
<repository-root>/.mcpd/config.toml
```

The project file is an **overlay**, not another independent source of truth.

### Example

```toml
version = 1

[servers.github]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[servers.github.env]
GITHUB_HOST = "github.com"

[servers.github.secrets]
GITHUB_TOKEN = "github.token"

[servers.sentry]
transport = "http"
url = "https://mcp.sentry.dev/mcp"

[targets.cursor.servers.sentry]
enabled = true

[targets.codex.servers.sentry]
enabled = false
```

### Canonical transport model

v1 supports:

- `stdio`
- `http` representing MCP Streamable HTTP

Legacy SSE should be representable through an adapter or a compatibility mechanism where required by a target, but the canonical model should not promote deprecated transport semantics to a first-class v1 abstraction. The MCP specification identifies Streamable HTTP as the current standard HTTP transport and notes the backwards-compatibility path for older HTTP+SSE servers.

### Variable references

Canonical values may contain safe references:

```text
${env:NAME}
${secret:NAME}
```

`${env:NAME}` reads from the process environment at sync/runtime expansion.

`${secret:NAME}` resolves through the OS keyring.

The canonical file must never contain raw secret material merely because a user used a secret placeholder.

## 9. Scope and precedence

The resolution model is:

```text
Built-in defaults
      ↓
Global canonical config
      ↓
Trusted project overlay
      ↓
Explicit CLI overrides
      ↓
Target adapter rendering
```

Project configuration may:

- add servers;
- override non-secret fields;
- disable a global server for that project;
- enable project-only servers;
- configure target-specific enable/disable state.

Project configuration may **not** silently read arbitrary global secrets. A project can reference a secret by name only after the project is trusted, and the secret must still be present in the user's keyring.

This trust model follows the security direction already visible in clients such as Codex and Claude Code, which treat project-local MCP configuration as a meaningful trust boundary.

### Project trust

Command:

```text
mcpd trust
mcpd trust <path>
mcpd trust --list
mcpd trust --revoke <path>
```

Trust is stored locally in mcpd state and should identify projects by canonical path plus a stable project identity where practical.

Untrusted project overlays are ignored by default and produce a concise warning in commands where relevant.

## 10. Target architecture

The system is split into a canonical core and target adapters.

```text
                  Canonical Model
                        │
               Resolved Project View
                        │
                 Sync Orchestrator
                        │
        ┌───────────────┼────────────────┐
        ↓               ↓                ↓
   Target Adapter  Target Adapter   Target Adapter
     Claude            Cursor            Codex
        │               │                │
        ↓               ↓                ↓
   native config    native config    native config
```

### Target adapter responsibilities

An adapter owns:

- detection;
- config path discovery;
- native parsing;
- native serialization;
- mcpd-owned entry detection;
- target-specific validation;
- target-specific capability declaration;
- target-specific normalization;
- safe merge operations;
- import conversion into canonical form.

The adapter does **not** own:

- canonical config semantics;
- secret storage;
- watch orchestration;
- global locking;
- CLI presentation;
- update checking;
- generic diffing.

## 11. Initial target adapters

### Claude Code

The adapter must support the current user/global and project-level MCP configuration model exposed by Claude Code, while treating Claude's own CLI and authentication UX as authoritative for flows mcpd does not own. Claude Code currently supports local, project, and user MCP scopes and can also authenticate remote servers through its own UI/command flow.

The adapter must prefer direct file operations when safe and deterministic, but may expose a CLI-backed operation for client-native operations that cannot be represented safely through file edits.

### Cursor

The adapter should support current global and project MCP configuration forms documented by Cursor, including the transport variants Cursor supports.

### Codex

The adapter must render TOML into the Codex MCP section using `[mcp_servers.<name>]` semantics and the appropriate transport-specific keys supported by the detected Codex version. The commonly used global config is `~/.codex/config.toml`; project-local configuration has trust implications.

Because Codex configuration behavior has changed across releases, the adapter must isolate version-sensitive logic and have fixtures for supported versions.

### Antigravity

Antigravity's MCP configuration is JSON-based. Current Google documentation exposes MCP configuration under the `.gemini` configuration hierarchy and explicitly documents a user-level `mcp_config.json`. Exact path resolution must remain adapter-version-aware because Google's documentation has exposed more than one `.gemini` path across iterations.

### OpenChamber

OpenChamber is an OpenCode interface with an evolving MCP/settings surface. The mcpd adapter should target the underlying OpenCode-compatible MCP configuration where possible rather than depending tightly on OpenChamber UI behavior. The adapter must therefore detect the actual local OpenCode/OpenChamber configuration and preserve unrelated settings.

## 12. Custom target definitions

One of the defining capabilities of mcpd is extensibility without recompiling the binary for every new application.

Built-in targets may be compiled into Rust for richer behavior. User-defined targets may additionally use declarative manifests when the target is simple enough.

Conceptual example:

```toml
id = "bananacode"
name = "BananaCode"
platforms = ["linux", "macos"]

[detect]
commands = ["bananacode"]

[config]
path = "~/.banana/config.json"
format = "json"
servers_path = "mcp.servers"
```

Declarative adapters are intentionally limited. If a target requires non-trivial transformations, version detection, migrations, or custom authentication behavior, it must use a compiled adapter.

## 13. Target discovery and enabling

Discovery is informational by default.

```text
mcpd targets
```

may report:

```text
✓ installed    Claude Code
✓ installed    Cursor
✓ installed    Codex
✓ installed    Antigravity
○ not detected OpenChamber
```

Detection must never imply authorization to modify a target.

Enablement is explicit:

```text
mcpd targets enable cursor
mcpd targets disable cursor
```

An optional interactive first-run flow may offer detected targets, but must require confirmation before enabling them.

## 14. Ownership and merge semantics

mcpd must preserve every unmanaged target entry.

Example native target state:

```json
{
  "mcpServers": {
    "github": { "...": "mcpd-managed" },
    "my-manual-server": { "...": "user-managed" }
  }
}
```

After sync:

- `github` may be replaced by canonical mcpd state;
- `my-manual-server` must remain intact.

### Managed ownership

mcpd should not inject vendor-risky metadata into MCP server objects unless a target explicitly supports it.

Instead, ownership is tracked in:

```text
~/.local/state/mcpd/state.toml
```

or the platform equivalent.

For each target and managed server, state includes a normalized canonical hash and a rendered target hash. This permits drift detection without contaminating target files with mcpd metadata.

### Drift behavior

If a managed target entry has been manually changed:

- normal `mcpd sync` treats canonical config as authoritative and repairs the managed entry;
- `mcpd diff` reports the drift before repair;
- `mcpd sync --dry-run` previews the repair;
- the previous target file is available through the backup system when backups are enabled.

An explicit command can adopt a target entry later:

```text
mcpd adopt cursor github
```

Adoption is out of the critical v1 path but should be possible in the architecture.

## 15. Backups and atomic writes

Before modifying a target file, mcpd should:

1. parse and validate the existing target;
2. create a timestamped backup according to policy;
3. render the new document in memory;
4. write to a temporary file in the same directory;
5. flush and sync where appropriate;
6. atomically rename into place;
7. update mcpd state only after the target write succeeds.

Backups are enabled by default for first releases and can be disabled explicitly.

Default backup location:

```text
~/.local/state/mcpd/backups/
```

Backups must have restrictive permissions when the source file contains or may contain secrets.

## 16. Secrets

### Storage

Use the Rust `keyring` ecosystem to integrate with the platform credential store.

Linux should work with Secret Service and KWallet where the installed keyring backend supports them.

### API

```text
mcpd secret set <name>
mcpd secret get <name>      # only when explicitly requested; never print by default
mcpd secret remove <name>
mcpd secret list
```

`secret get` should require an explicit unsafe flag such as `--reveal` and should warn that the value may be exposed in shell history, terminal scrollback, or process capture.

### Redaction

All user-visible paths must pass through a redaction layer where secret references have been resolved. Normal output must show:

```text
GITHUB_TOKEN = ********
```

rather than the underlying value.

### Secret resolution policy

Secrets are resolved as late as possible. Canonical parsing should retain references, not secret values. Rendering a target should resolve only secrets required by that target and only for that operation.

## 17. Remote MCP servers

v1 canonical support includes Streamable HTTP.

Example:

```toml
[servers.docs]
transport = "http"
url = "https://example.com/mcp"

[servers.docs.headers]
X-Client = "mcpd"

[servers.docs.secrets]
Authorization = "docs.authorization"
```

mcpd is responsible for configuration translation, not for implementing an MCP server proxy. Remote authentication that requires OAuth browser flows should remain with the target client unless a client adapter has a safe, documented mechanism for configuring it. Claude Code, for example, exposes remote OAuth handling in its own MCP UI.

## 18. Sync engine

The sync engine operates in deterministic stages:

```text
load global config
        ↓
resolve repository context
        ↓
load trusted overlay
        ↓
resolve canonical server graph
        ↓
resolve target set
        ↓
load target snapshots
        ↓
validate
        ↓
plan
        ↓
show / dry-run / execute
        ↓
atomic writes
        ↓
state update
```

### Sync transaction behavior

Sync must be target-isolated.

If Cursor succeeds and Codex fails, mcpd must:

- report Cursor success;
- report Codex failure;
- retain enough state to identify what was applied;
- never falsely claim the whole operation succeeded.

A failed target must not roll back successful independent targets unless the user explicitly requests all-or-nothing behavior in a future version.

## 19. Watch mode

Command:

```text
mcpd watch
```

Watch mode observes the canonical configuration and project overlay files.

Optional target watching may detect user changes to target files and report drift, but it must **not automatically overwrite external target edits in response to every target filesystem event**. Doing so would create editor feedback loops and make debugging miserable.

The default model is:

```text
canonical/project changed
        ↓
 debounce
        ↓
 sync
```

Target changes should instead update the visible drift state and may be repaired on the next explicit sync or a canonical change.

### Debounce

Initial implementation target: approximately 200–500 ms, configurable later.

### Concurrency

Only one sync transaction may run at a time. A lock or in-process coordination primitive must prevent concurrent writes.

## 20. systemd integration

Linux users may run mcpd continuously as a user service.

Commands:

```text
mcpd systemd generate
mcpd systemd install
mcpd systemd uninstall
```

Generated service should run as the user's systemd user unit and restart on failure. It must not require root privileges.

The project must not assume a login shell environment. systemd services should support an explicit environment strategy because developer tools installed in user paths may not be present in a minimal service environment. This is a common issue in OpenChamber/OpenCode systemd deployments as well.

## 21. CLI command surface

Initial command tree:

```text
mcpd init
mcpd add
mcpd remove
mcpd list
mcpd get
mcpd sync
mcpd watch
mcpd status
mcpd diff
mcpd doctor
mcpd import
mcpd targets
mcpd secret
mcpd trust
mcpd systemd
mcpd version
mcpd completions
```

Subcommands should use conventional Rust CLI conventions and provide consistent `--json`, `--quiet`, `--verbose`, and `--dry-run` behavior where meaningful.

## 22. Import

Import is explicit, not automatic.

Examples:

```text
mcpd import claude
mcpd import cursor
mcpd import codex
mcpd import --all
```

Import should never silently overwrite canonical entries.

If two sources disagree, import must produce a conflict report and require the user to choose a source or merge explicitly.

## 23. Diff and status

### `mcpd diff`

The command compares canonical resolved state against target-rendered state.

Output should be concise by default and expand on demand:

```text
Codex
  = github
  ~ sentry       header drift
  + playwright   missing
```

Secret values are never shown.

### `mcpd status`

Status should answer three questions immediately:

1. What is configured?
2. Which targets are enabled/detected?
3. Is anything out of sync or unhealthy?

## 24. Doctor

`mcpd doctor` performs static checks only by default:

- canonical config validity;
- project trust state;
- target detection;
- target config parseability;
- keyring availability;
- secret reference validity without revealing values;
- backup/state directory permissions;
- systemd availability;
- PATH/command availability for stdio servers;
- adapter compatibility;
- stale locks.

A future `mcpd doctor --deep` may launch servers or perform remote MCP initialization.

## 25. Health checks

Health is intentionally separate from sync.

Future-capable commands:

```text
mcpd test <server>
mcpd test --all
```

v1 should at minimum validate whether a configured stdio command is resolvable and whether an HTTP endpoint is syntactically valid and reachable when the user explicitly asks for a network check.

Do not auto-launch every stdio server during ordinary `status` because that could execute arbitrary commands unexpectedly.

## 26. Security model

Security is a first-class product feature, not a README warning.

### Threats

- malicious project configuration;
- accidental secret disclosure;
- corrupt target configuration;
- malicious or compromised MCP packages;
- path traversal via project config;
- symlink attacks on configuration paths;
- TOCTOU races during writes;
- shell injection through command construction;
- environment leakage;
- accidental execution of arbitrary commands during discovery or diagnostics.

### Mitigations

- no shell interpolation for stdio commands; use structured process spawning;
- canonicalize and validate paths;
- refuse unsafe symlink targets for config files by default;
- atomic writes;
- restrictive permissions on mcpd state and secret-related files;
- secrets through keyring references;
- no secrets in logs;
- project trust model;
- explicit confirmation for potentially destructive operations;
- no server execution during ordinary sync;
- no automatic download or install of MCP packages in v1;
- adapter-level validation and fixture tests.

The MCP specification itself emphasizes secure handling of Streamable HTTP, including Origin validation and appropriate authentication on server implementations. mcpd should therefore avoid adding a false sense of security around remote MCPs; it is a configuration manager, not a security boundary for the remote server.

## 27. Cross-platform strategy

### v1

Linux-first, with clean platform abstractions from the beginning.

Supported concepts:

- XDG config/state/data paths on Linux;
- Linux keyrings through the chosen Rust keyring backend;
- systemd user services;
- Linux command/path discovery.

### Future

macOS and Windows should be possible without rewriting the core. Platform-specific target adapters and secret providers should implement traits behind the core abstractions.

## 28. Suggested Rust stack

Core:

- `clap` for CLI parsing;
- `serde` + `serde_json` + `toml` for data formats;
- `thiserror` for typed domain errors;
- `anyhow` at application boundaries where appropriate;
- `tracing` + `tracing-subscriber` for diagnostics;
- `notify` for file watching;
- `keyring` for OS credential storage;
- `directories` for platform paths;
- `which` for executable discovery;
- `fs2` or an equivalent locking strategy where needed;
- `reqwest` for explicit network diagnostics and future remote operations;
- `tokio` only where asynchronous IO is actually valuable.

Avoid adding dependencies purely because they are fashionable. Every dependency should have a clear operational or maintenance benefit.

## 29. Proposed repository structure

```text
mcpd/
├── src/
│   ├── main.rs
│   ├── cli/
│   ├── config/
│   ├── model/
│   ├── resolve/
│   ├── sync/
│   ├── targets/
│   │   ├── mod.rs
│   │   ├── claude.rs
│   │   ├── cursor.rs
│   │   ├── codex.rs
│   │   ├── antigravity.rs
│   │   └── openchamber.rs
│   ├── secrets/
│   ├── state/
│   ├── watch/
│   ├── systemd/
│   ├── diagnostics/
│   └── output/
├── tests/
│   ├── fixtures/
│   ├── adapters/
│   ├── sync/
│   └── security/
├── docs/
├── examples/
├── scripts/
├── PRD.md
├── AGENTS.md
├── SECURITY.md
├── CONTRIBUTING.md
├── LICENSE
└── README.md
```

## 30. Testing strategy

### Unit tests

- canonical parser;
- overlay merge semantics;
- secret reference parsing;
- target ownership state;
- path resolution;
- conflict detection;
- diff calculation;
- output redaction.

### Adapter tests

Every adapter must have fixtures covering:

- empty target;
- existing unmanaged entries;
- managed entries;
- malformed config;
- duplicate/ambiguous keys;
- transport-specific renderings;
- import behavior;
- version-specific behavior where applicable.

### Integration tests

Use temporary directories and fake client configurations.

Never modify the developer's real home directory in automated tests.

### Property testing

Use property-based tests for:

- merge invariants;
- idempotent serialization where possible;
- redaction guarantees;
- path normalization;
- sync planning.

### Security tests

Explicitly test:

- secret values absent from logs;
- malicious project config cannot access untrusted global state;
- symlink/path traversal defenses;
- atomic write failure behavior;
- malformed target documents never cause data loss.

## 31. Quality requirements

mcpd should be held to a higher bar than a normal utility because it edits configuration other programs depend on.

The project should target:

- zero known secret disclosure in normal output;
- idempotent sync;
- no data loss on malformed target files;
- deterministic target rendering;
- useful error messages with actionable remediation;
- graceful handling of unsupported client versions;
- test coverage across all built-in adapters;
- reproducible release artifacts;
- signed or verifiable release checksums before broad public adoption.

## 32. Install and distribution

Initial installer:

```text
curl -fsSL https://get.mcpd.dev/install.sh | sh
```

The installer should:

- detect OS/architecture;
- download a versioned release artifact;
- verify checksum/signature when available;
- install to `~/.local/bin/mcpd` by default;
- avoid requiring root;
- refuse to overwrite a binary it cannot safely verify.

The installer URL is a planned project endpoint, not a dependency of the core binary. Release automation should eventually publish artifacts to GitHub Releases and support package managers such as AUR/Homebrew/Nix without changing application behavior.

## 33. Update strategy

`mcpd update` should be safe, explicit, and opt-in.

It should never silently replace a running binary.

Release artifacts should include:

- semver version;
- SHA-256 or stronger integrity hashes;
- eventually Sigstore or another widely verifiable signing mechanism.

## 34. CLI conventions

The CLI should support:

- human-readable default output;
- `--json` for scripting;
- `--quiet` for automation;
- `-v`, `-vv`, `-vvv` for verbosity;
- meaningful non-zero exit codes;
- consistent error prefixes and remediation suggestions;
- color only when stdout is a TTY;
- no color when piped unless explicitly requested.

Suggested exit classes:

```text
0   success
1   operational failure
2   invalid configuration/input
3   target unavailable/unsupported
4   conflict/drift requiring user action
5   permission/security failure
```

Exact numeric semantics should be stabilized before 1.0.

## 35. Observability and privacy

mcpd has:

```text
NO TELEMETRY
NO CLOUD ACCOUNT
NO ANALYTICS
NO REMOTE CONFIG BACKUP
```

Logs are local and user-controlled.

Remote requests only occur when the user explicitly configures or requests them, except where a future explicit update-check feature is enabled.

## 36. Open-source governance

Repository should include:

- MIT license;
- `SECURITY.md`;
- `CONTRIBUTING.md`;
- Code of Conduct;
- issue templates;
- pull request template;
- release notes/changelog;
- adapter development guide.

Changes to the canonical schema or target adapter contracts should be treated as compatibility-sensitive changes.

## 37. Versioning and migrations

Canonical config begins at:

```toml
version = 1
```

Future schema migrations must be explicit and reversible where practical.

Command:

```text
mcpd migrate
```

Automatic migration may be offered only when a backup is made first and the migration can be proven safe.

Target adapter version detection is separate from canonical schema versioning.

## 38. Success metrics

The project is successful when a user can:

1. install mcpd without root;
2. add an MCP server once;
3. have it appear correctly in every enabled supported client;
4. add a new client later and synchronize without retyping servers;
5. keep secrets out of configuration files;
6. understand any drift or failure immediately;
7. recover from a failed configuration write without data loss;
8. add a custom declarative target without modifying mcpd core where possible.

Qualitative success is more important than raw installation counts: users should trust mcpd enough to let it manage their configuration automatically.

## 39. MVP acceptance criteria

The first public release is acceptable when all of the following are true:

### Installation

- [x] A fresh Linux user can install mcpd without root.
- [x] `mcpd --version` works.
- [x] Shell completion can be generated.

### Config

- [x] Global config loads from the platform config directory.
- [x] Project overlay is discovered correctly.
- [x] Untrusted overlays are ignored safely.
- [x] Config schema errors are actionable.

### Targets

- [x] Claude Code adapter passes fixture tests.
- [x] Cursor adapter passes fixture tests.
- [x] Codex adapter passes fixture tests.
- [x] Antigravity adapter passes fixture tests.
- [x] OpenChamber/OpenCode adapter passes fixture tests.
- [x] Unmanaged entries survive sync.

### Secrets

- [x] Secret values are stored in OS keyring when available.
- [x] Secrets do not appear in ordinary CLI output.
- [x] Secret references work during render.
- [x] Missing secrets produce actionable errors.

### Sync

- [x] `mcpd sync` is idempotent.
- [x] `mcpd sync --dry-run` performs no writes.
- [x] Failed target sync does not corrupt other targets.
- [x] Atomic writes are used.
- [x] Backups retain the previous target state for manual restore.

### Diagnostics

- [x] `status`, `diff`, and `doctor` provide actionable output.
- [x] `--json` works for machine-readable diagnostics.
- [x] Exit codes are deterministic.

### Watch

- [x] Watch mode detects canonical and trusted-overlay changes.
- [x] Changes are debounced.
- [x] Only one sync operation executes at once.
- [x] The process has no child/background resources and exits cleanly on termination.

### Systemd

- [x] Unit generation works.
- [x] User-level install works without root.
- [x] The service survives transient failures.

## 40. Roadmap after v1

Potential next stages:

### v1.1

- richer import conflict resolution;
- `adopt` support;
- deeper `doctor` checks;
- more adapters;
- improved systemd integration.

### v1.2

- macOS support;
- Windows support;
- package-manager distribution;
- declarative adapter registry improvements;
- profiles.

### v2

- first-class target capabilities and version compatibility matrix;
- bidirectional explicit synchronization;
- server health checks with MCP initialization;
- migration tooling across client format changes;
- optional team-shared canonical manifests without central secrets.

### Long-term

A local MCP control plane that can manage not only where servers are configured, but how developers safely reason about the MCP environment on their machine.

The project should resist becoming a cloud product unless users demonstrate a compelling need.

## 41. Architectural north star

The architecture should make the following possible without replacing the core model:

```text
                    mcpd core
                       │
        ┌──────────────┼───────────────┐
        ↓              ↓               ↓
    built-in       declarative      compiled
    adapters        adapters         adapters
        │              │               │
   Claude/Codex    simple apps      complex apps

                    │
                    ↓
             canonical MCP graph
                    │
          ┌─────────┼──────────┐
          ↓         ↓          ↓
       secrets     state      plans
          │                    │
          └─────────┬──────────┘
                    ↓
                 targets
```

The key design choice is to keep **MCP semantics, target semantics, filesystem mutation, secrets, and presentation as separate layers**. That separation is the main defense against mcpd becoming an unmaintainable pile of client-specific conditionals.

## 42. Reference sources

The design uses current public MCP/client behavior as of August 2026. Client APIs and config formats can change; target adapters must therefore be treated as compatibility boundaries rather than eternal truths.

- MCP transport specification: https://modelcontextprotocol.io/specification/2025-11-25/basic/transports
- Claude Code MCP documentation: https://docs.anthropic.com/en/docs/claude-code/mcp
- Cursor MCP documentation: https://docs.cursor.com/context/model-context-protocol
- Codex repository/config examples: https://github.com/openai/codex
- Codex MCP documentation: https://developers.openai.com/codex/mcp
- Antigravity MCP documentation: https://developers.google.com/workspace/guides/configure-mcp-servers
- OpenChamber: https://github.com/openchamber/openchamber

## 43. Final product definition

`mcpd` is a **local MCP configuration control plane**.

It owns one canonical configuration, stores secrets safely, translates configuration into multiple native client formats, detects installed clients, preserves unmanaged state, reports drift clearly, and optionally keeps enabled targets synchronized.

Everything else is secondary.

The product should feel less like another AI tool and more like a piece of infrastructure developers forget is there because it simply keeps the MCP layer coherent.
