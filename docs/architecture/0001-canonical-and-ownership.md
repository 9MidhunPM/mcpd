# ADR 0001: Canonical schema and external ownership state

Status: accepted for the v0.1 vertical slice

## Decision

Canonical configuration begins at `version = 1` and models stdio and Streamable HTTP as distinct tagged server variants. Maps use deterministic ordering. Unknown schema versions and unknown fields are errors rather than being silently reinterpreted.

Target-specific parsing and rendering live behind adapters. The initial Codex adapter owns detection, path discovery, TOML parsing, capability translation, normalization, and rendering. Generic synchronization handles locking, execution, backups, and ownership-state commits.

Target discovery is informational. A single generic eligibility step selects explicitly enabled targets before any adapter planning or execution; disabled targets are not interpreted as targets with an empty desired server set. Canonical mutations using `--no-sync` return before that selection step and do not inspect target or ownership state.

Ownership is recorded under the local state directory and never injected into client configuration. An existing same-name entry without a matching ownership record is a conflict. An owned server table is wholly controlled by canonical state; external edits are drift and normal sync repairs them. Unmanaged tables and unrelated client settings remain untouched.

## Consequences

- A lost state file does not authorize `mcpd` to infer or reclaim ownership.
- Imports and adoption are explicit operations; discovery alone never grants ownership.
- Adding another client should add an adapter, not target conditionals to generic sync code.
