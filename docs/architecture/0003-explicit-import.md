# ADR 0003: Explicit, lossless target import

Status: accepted for v1.0

## Decision

Discovery is read-only and does not imply ownership. Import is a separate explicit command implemented through the target-adapter contract. The adapter parses native definitions, validates target-specific fields, translates supported transports, and supplies normalized ownership hashes. Generic orchestration handles selection, canonical conflicts, batching, locking, and the ownership commit.

An import must round-trip through the current canonical model without changing the native server definition, except when the user explicitly requests a secret migration. Unsupported native policy and transport fields are rejected rather than discarded. Reliably secret-bearing literals require keyring migration; uncertain values require an explicit user decision.

Import never writes the target. All selected canonical additions are planned and validated before mutation. A same-name canonical entry aborts the whole batch. Real imports serialize with sync, atomically replace canonical configuration, and then commit ownership state. A secret-free pending record allows the state commit to recover after interruption. Dry-run stops before lock or filesystem mutation.

Bulk import skips entries already owned by matching adapter state and reports them separately. It does not parse them as new imports, so newly unsupported native fields on an already managed entry do not accidentally broaden adoption.

## Consequences

- Existing target definitions can be adopted without a target rewrite.
- An imported entry is immediately synchronized when the canonical and target definitions match.
- Native features without a lossless canonical representation remain unmanaged.
- Supporting a new target requires implementing adapter translation, not adding target-specific import branches to the CLI.
