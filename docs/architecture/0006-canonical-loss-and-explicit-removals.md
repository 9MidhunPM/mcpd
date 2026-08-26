# ADR 0006: Canonical loss recovery and explicit managed removals

## Decision

Ownership state is evidence that a missing or empty canonical server table may be accidental data loss. `syncplane init` therefore refuses to create an empty canonical file while state records managed servers. Unsupported canonical schema versions continue to fail closed. Any schema-update implementation must use the canonical update transaction, which compares the planned input snapshot, requires every old canonical and ownership-recorded server in the rendered result, writes a private content-addressed backup, and atomically replaces the canonical file without changing ownership state.

Planning remains read-only and reports the exact managed removals implied by canonical state. Human `status`, `diff`, and `sync --dry-run` output uses explicit warnings and `REMOVE` lines; JSON output carries structured removal-safety data. Execution denies managed removals by default. `syncplane remove SERVER` authorizes only the named server, while bulk or manually staged removals require `syncplane sync --allow-removals` after review.

Unmanaged target entries are outside this policy because they are never removal candidates.

## Consequences

- Deleting a canonical file followed by `syncplane init` cannot silently convert remembered ownership into a deletion plan.
- Future migrations have one fail-closed commit path with a recoverable pre-migration copy.
- Manual canonical deletion is a two-step operation: inspect the plan, then authorize it.
- Watch mode cannot silently propagate managed deletions staged outside `syncplane remove`; an operator must confirm them explicitly.
