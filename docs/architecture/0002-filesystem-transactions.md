# ADR 0002: Per-target filesystem transactions

Status: accepted for v1.0

## Decision

An executing sync is serialized by a local lock. It validates and plans in memory, verifies that the input snapshot is unchanged, creates a restrictive backup, persists a secret-free pending transaction, atomically replaces the target, then atomically commits ownership state. Pending records are reconciled by hashes after interruption.

Dry-run stops after planning. It creates no filesystem state.

The PRD contains competing requirements that backups never contain secrets and that complete pre-write target backups remain recoverable. Data recovery wins for opaque target snapshots: a backup may contain credentials already present in the client file, so it is mode `0600` beneath mode `0700` state directories and is never logged, parsed into metadata, or printed.

Symbolic links in the Codex target path are rejected. The first slice does not offer an override because following user-managed dotfile symlinks needs a separately reviewed path policy.

## Consequences

- Atomicity is per target, not across all future adapters.
- A state-commit failure is recoverable without silently claiming an unrelated file.
- A small residual race remains between final snapshot verification and atomic rename and is documented in the security model.
