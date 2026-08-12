# AGENTS.md

## Purpose

This repository contains `mcpd`, a production-quality open-source Rust CLI that manages one canonical MCP configuration and safely projects it into multiple MCP-capable AI clients.

The project is intentionally infrastructure-like. Changes that modify user configuration, secrets, target adapters, synchronization semantics, or trust behavior require more rigor than ordinary application changes.

## Core product rule

> Configure MCP once. Use it everywhere.

Keep `mcpd` focused on MCP configuration, synchronization, discovery, secrets, diagnostics, and safe target integration.

Do not turn it into an MCP proxy, model gateway, agent runtime, server marketplace, hosted service, or general AI configuration manager.

## Engineering priorities

When tradeoffs appear, prioritize in this order:

1. Data safety.
2. Secret safety.
3. Correctness and deterministic behavior.
4. Clear user-facing diagnostics.
5. Backwards compatibility.
6. Testability.
7. Extensibility.
8. Performance.
9. Convenience.

A faster sync that can corrupt somebody's Codex configuration is not an optimization. It is a bug wearing sportswear.

## Repository architecture

Expected high-level structure:

```text
src/
├── cli/            CLI parsing and command orchestration
├── config/         Canonical configuration formats and parsing
├── model/          Canonical MCP domain model
├── resolve/        Scope/overlay/trust resolution
├── sync/           Planning, merge, transaction, and execution
├── targets/        Target adapter implementations
├── secrets/        Keyring abstraction and secret references
├── state/          Ownership, drift, trust, and local state
├── watch/          Filesystem watching and debouncing
├── systemd/        Linux user-service generation/integration
├── diagnostics/    Errors, doctor checks, validation
└── output/         Human/JSON rendering and redaction
```

Do not place target-specific behavior in generic synchronization code.

If a rule only exists because Cursor, Codex, Claude, Antigravity, or OpenChamber behaves a certain way, it belongs in that target adapter or in an explicitly versioned compatibility layer.

## Canonical data model

Canonical MCP state is the source of truth.

Default config:

```text
~/.config/mcpd/config.toml
```

Project overlay:

```text
<repo-root>/.mcpd/config.toml
```

Local state:

```text
~/.local/state/mcpd/
```

Secrets must not be stored in canonical config files. Use keyring references.

Recommended reference syntax:

```text
${env:NAME}
${secret:NAME}
```

Keep secret references as symbolic values throughout parsing and resolution. Resolve actual secret bytes as late as possible.

## Target adapter contract

Every target adapter should provide concepts equivalent to:

```rust
trait TargetAdapter {
    fn id(&self) -> TargetId;
    fn detect(&self, ctx: &TargetContext) -> DetectionResult;
    fn locate_config(&self, ctx: &TargetContext) -> Result<ConfigLocation>;
    fn parse(&self, input: &TargetSnapshot) -> Result<TargetDocument>;
    fn import(&self, document: &TargetDocument) -> Result<Vec<CanonicalServer>>;
    fn plan(&self, desired: &ResolvedConfig, current: &TargetDocument) -> Result<TargetPlan>;
    fn render(&self, plan: &TargetPlan) -> Result<RenderedDocument>;
    fn capabilities(&self) -> TargetCapabilities;
}
```

The exact Rust API can evolve, but the conceptual separation must remain.

Adapters must own:

- path discovery;
- native parsing;
- native serialization;
- version detection;
- capability declarations;
- client-specific quirks;
- import/export translation.

Adapters must not own:

- keyring implementation;
- global locking;
- global watch orchestration;
- generic CLI parsing;
- global trust policy;
- generic diff rendering.

## Target compatibility

The project initially supports:

- Claude Code
- Cursor
- Codex
- Antigravity
- OpenChamber/OpenCode-compatible MCP configuration

Client config formats and paths change. Never assume that a path or schema observed once is permanent.

When adding or updating an adapter:

1. Verify current upstream documentation.
2. Prefer authoritative sources.
3. Check release notes and known regressions when behavior is version-sensitive.
4. Add fixtures for the observed format.
5. Add graceful detection for unsupported versions.
6. Keep the compatibility logic inside the adapter.

Current known facts include:

- MCP standardizes `stdio` and Streamable HTTP as transports; legacy HTTP+SSE may need compatibility handling. citeturn518265search2
- Claude Code exposes multiple MCP scopes and native MCP management commands. citeturn899695search0
- Codex uses `[mcp_servers.<name>]` in TOML and project-local config can be subject to trust behavior. citeturn977749search0turn977749search1
- Antigravity uses JSON MCP configuration under the `.gemini` configuration hierarchy, with documented path details evolving over time. citeturn923575search0turn923575search4
- Cursor supports stdio, SSE, and Streamable HTTP integrations, so adapters must not reduce Cursor to a single transport assumption. citeturn518265search4
- OpenChamber's MCP and OpenCode integration evolves actively, so adapter design should avoid depending on UI-only behavior. citeturn923575search1turn518265search3

Treat these references as implementation inputs, not hard-coded truths. Re-verify before making compatibility claims.

## Sync invariants

These invariants are mandatory:

### 1. Idempotence

Running:

```text
mcpd sync
mcpd sync
mcpd sync
```

without external changes must produce no semantic changes after the first successful sync.

### 2. Preserve unmanaged entries

Never delete or rewrite target configuration outside the mcpd-owned scope unless the user explicitly requests it.

### 3. Dry-run purity

```text
mcpd sync --dry-run
```

must not modify target files, canonical config, keyring state, or ownership state.

### 4. Atomicity per target

A target file must be written atomically. Never stream serialized JSON/TOML directly into the real file path.

### 5. Failure isolation

A failure synchronizing one target must not silently erase successful changes in other targets.

### 6. State-after-success

Ownership/drift state should be persisted only after the target write succeeds.

### 7. No implicit code execution during sync

Sync must configure MCP servers, not launch them.

The only exception is narrowly scoped validation explicitly requested by the user.

## File writes

For configuration mutation:

1. Read the current file.
2. Refuse unsafe or malformed input where recovery is ambiguous.
3. Parse into an owned data structure.
4. Calculate a plan.
5. Serialize to memory.
6. Write a temporary file in the same directory.
7. Flush/sync where appropriate.
8. Atomically rename.
9. Update mcpd state.

Do not use ad-hoc search-and-replace on JSON/TOML when structured parsing is available.

Preserve formatting/comments when the target format/library makes that safely possible. If a parser cannot preserve comments, design the adapter around the smallest safe ownership region rather than rewriting entire user files casually.

## Backups

Before modifying an existing target, create a backup according to policy.

Backups belong under mcpd local state, not in the repository and not in the canonical config directory.

Never include secret values in backup metadata or logs.

## Secrets

Secrets are high-risk data.

Rules:

- Do not serialize raw secrets to canonical config.
- Do not log raw secret values.
- Do not include raw secrets in `--json` output.
- Do not place secrets in error messages.
- Do not include secret values in test snapshots.
- Do not read secrets earlier than necessary.
- Avoid passing secrets through shell command strings.
- Prefer structured process/environment APIs.

Use symbolic references such as:

```text
${secret:GITHUB_TOKEN}
```

and resolve them late.

When adding debug logging around resolution, log the key name and resolution status, never the value.

## Project trust

Project-local config is executable policy because it can alter the MCP environment.

Untrusted project overlays must not read global secrets or silently influence global configuration.

Do not weaken this boundary for convenience.

If a feature requires project-specific credentials, make the trust model explicit and test it.

## Process execution

Never construct shell commands like:

```rust
format!("{} {}", command, args.join(" "))
```

and feed them to a shell for convenience.

Use structured process spawning:

```rust
Command::new(program)
    .args(args)
    .envs(env)
    .spawn()?;
```

Commands and arguments are user-controlled configuration and must be treated as untrusted input.

## Paths and symlinks

Configuration paths are security-sensitive.

Validate:

- absolute vs relative path assumptions;
- traversal sequences;
- symlink destinations;
- project root boundaries;
- platform path semantics.

Do not follow an unexpected symlink to a config file outside the intended target without an explicit, documented policy.

## Watch mode

`mcpd watch` should observe canonical config and trusted project overlays.

Use debouncing to avoid editor-write storms.

Do not create a feedback loop by treating every target-file write as a reason to immediately rewrite the same target.

Target changes should update drift information. Canonical changes should trigger synchronization.

Only one sync transaction may run at a time.

## State and drift

mcpd should track managed ownership and normalized hashes in local state rather than injecting proprietary metadata into target configuration files.

A useful state record includes:

```text
target id
config path
managed server names
canonical hash
rendered hash
adapter version
last successful sync time
```

Do not put secret values in state.

## CLI design

Commands should be consistent and composable.

Initial command surface:

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

Human output is the default.

Machine output should be available through:

```text
--json
```

Do not make the human output look like raw JSON unless `--json` was requested.

Use color only when appropriate for the output stream.

Interactive prompts should fail gracefully when stdin is not a TTY. Automation users must be able to use non-interactive commands.

## Error handling

Errors should answer:

1. What happened?
2. Which target/file/server caused it?
3. Why did it happen?
4. What should the user do next?

Bad:

```text
parse error
```

Good:

```text
Codex sync failed: ~/.codex/config.toml is valid TOML but contains an unsupported MCP server field `foo` for the detected Codex version.

Hint: run `mcpd doctor codex` to inspect adapter compatibility.
```

Avoid enormous backtraces in normal CLI mode.

Preserve underlying error causes for verbose/debug output.

## Exit codes

The final numeric scheme is still subject to implementation review, but the intended categories are:

```text
0   success
1   operational failure
2   invalid input/config
3   target unavailable or unsupported
4   conflict/drift requiring action
5   security/permission failure
```

Keep exit behavior stable once documented publicly.

## Tests

Every bug fix should come with a regression test unless impossible.

Minimum test layers:

### Unit

Pure functions, parsers, mergers, resolvers, redaction.

### Adapter

Input fixtures → canonical model → target rendering → reparsing.

### Integration

Temporary filesystem trees, state directories, fake target configs.

### Security

Path traversal, symlink handling, untrusted projects, secret redaction, malformed target recovery.

### Property-based

Use where invariants are important and examples are insufficient, especially for merge logic and serialization normalization.

## Adapter fixture rules

For each built-in adapter maintain fixtures for at least:

```text
empty config
single mcpd server
multiple mcpd servers
unmanaged servers
mixed managed/unmanaged
malformed config
unsupported fields
remote server
stdio server
secret references
```

Version-specific fixtures should be named clearly.

Never use a developer's real home directory as a test fixture.

## Documentation requirements

Every public behavior should be documented in at least one of:

- README;
- command help;
- architecture documentation;
- adapter documentation;
- security documentation.

When an upstream client changes configuration semantics, update the corresponding adapter docs and tests.

## Dependencies

Prefer a small, well-maintained dependency set.

Expected core choices include:

- `clap`
- `serde`
- `serde_json`
- `toml`
- `thiserror`
- `anyhow` where appropriate at application boundaries
- `tracing`
- `tracing-subscriber`
- `notify`
- `keyring`
- `directories`
- `which`
- `reqwest` for explicit HTTP diagnostics
- `tokio` only when asynchronous operations justify it

Before adding a dependency, consider:

- maintenance status;
- licensing;
- transitive dependency cost;
- platform support;
- security history;
- whether a small internal implementation is clearer.

Do not add a giant framework to solve a three-function problem.

## Formatting and linting

Expected baseline:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

CI should run these checks before a merge.

## Performance

mcpd is primarily a configuration tool, not a high-throughput service.

Optimize for:

- fast startup;
- low memory usage;
- minimal unnecessary filesystem scans;
- no unnecessary network calls;
- cheap no-op syncs.

Do not introduce asynchronous complexity merely to shave milliseconds from a command that runs once per hour.

## Compatibility and migrations

Canonical config must include a schema version:

```toml
version = 1
```

Never silently reinterpret an incompatible schema.

For migrations:

1. detect the old version;
2. explain the migration;
3. make a backup;
4. migrate deterministically;
5. validate the result;
6. only then replace the original.

Target adapter versioning is separate from canonical config versioning.

## systemd

systemd support is user-level.

Generated units must:

- run without root;
- use a stable executable path;
- account for minimal service environments;
- restart on transient failure;
- log predictably through the user journal;
- avoid assuming shell startup files were loaded.

Do not hardcode the developer's current home-directory path in tests, docs, or code.

## Open-source contribution rules

A pull request should generally contain:

1. implementation;
2. tests;
3. documentation updates when behavior is user-visible.

Prefer small cohesive commits.

Do not combine unrelated refactors with compatibility-sensitive adapter changes.

When updating an adapter, explain which upstream version/behavior prompted the change.

## Git workflow

Do not rewrite history or force-push unless explicitly requested.

Do not commit:

- personal secrets;
- real MCP configs containing credentials;
- machine-specific absolute paths unless they are test fixtures;
- generated local state;
- build artifacts.

Expected ignored paths include the normal Rust build directory and local test/state artifacts.

## Release discipline

Before a release:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Additionally verify:

- installer behavior;
- target adapter fixtures;
- schema migrations;
- secret redaction;
- reproducible version output;
- release checksum/signature generation.

Do not publish a release if a built-in adapter can corrupt or erase unmanaged target configuration.

## Security-sensitive changes

Treat these as security-sensitive and require extra review:

- secret handling;
- project trust;
- path validation;
- symlink behavior;
- process execution;
- remote HTTP behavior;
- automatic update logic;
- target configuration mutation;
- backup/restore behavior.

Document the threat model and regression test where possible.

## What not to build accidentally

Do not add these to core without a new product decision:

```text
MCP proxy/gateway
Cloud sync service
MCP marketplace
Agent runner
Model router
Remote configuration SaaS
General IDE settings manager
Automatic package installer for arbitrary MCP servers
Always-on background daemon as a mandatory dependency
```

The project wins by doing one hard thing extremely well.

## Decision record rule

When a design decision affects any of these, document it:

- canonical schema;
- sync semantics;
- ownership model;
- trust boundaries;
- adapter contract;
- secrets;
- target compatibility;
- filesystem mutation;
- public CLI behavior.

A short ADR is preferable to institutional memory.

## How to evaluate a proposed feature

Before merging a feature, ask:

1. Does it strengthen the central promise?
2. Does it make configuration safer?
3. Does it introduce a new trust boundary?
4. Does it belong in core or an adapter?
5. Can it be tested deterministically?
6. Can it be explained simply to users?
7. Does it make future compatibility easier or harder?
8. Is the operational complexity justified?

If the feature does not clearly help the configuration control-plane problem, defer it.

## Product north star for contributors

A user should be able to install mcpd, define an MCP server once, enable five clients, and forget that those clients use five different configuration systems.

The ideal result is invisible infrastructure:

```text
          one canonical MCP environment
                      │
        ┌─────────────┼─────────────┐
        ↓             ↓             ↓
      Claude        Cursor         Codex
        ↓             ↓             ↓
   Antigravity    OpenChamber     future client
```

If a change makes that diagram harder to reason about, stop and redesign before coding.
