# Security policy

`mcpd` edits configuration consumed by programs that can launch local processes or contact remote services. Treat canonical and project MCP configuration as executable policy.

## Current security boundary

- Only the global canonical file and global Codex target are implemented.
- Target files and their existing path components must be regular paths below the configured home directory. Symbolic links are rejected.
- Sync never launches Codex or an MCP server.
- Dry-run does not create locks, directories, backups, target files, or ownership state.
- Existing malformed targets and unmanaged name collisions stop the operation before a backup or write.
- Target writes use a same-directory temporary file, file sync, atomic rename, and parent-directory sync.
- Ownership state is committed only after the target write. A secret-free pending record recovers interruption between those operations.
- Existing target backups are opaque full-file snapshots stored with mode `0600`; backup directories and state directories use mode `0700`. Backup contents are never included in metadata or command output.

The implementation checks that the target snapshot still matches the planned input immediately before writing. A residual time-of-check/time-of-use window remains between that check and rename; eliminating it portably requires a future platform-specific filesystem layer.

## Secrets

Secret values are stored through the operating-system keyring and are never included in the canonical file, ownership state, transaction records, diagnostics, or generated Codex configuration. Canonical structured references use `{ secret = "NAME" }`.

Secret-bearing stdio servers are projected to `mcpd exec SERVER`. Values are resolved only immediately before replacing that process with the real configured command and are injected only into its environment. Sync, diff, status, and ordinary doctor checks do not launch servers or resolve secret values.

Codex cannot resolve OS-keyring values for HTTP headers. mcpd rejects that projection rather than copying credentials or implementing a proxy. Codex-native environment references remain supported.

Import identifies only strong credential-name suffixes automatically, groups confirmation per server, and derives deterministic `<server>.<FIELD>` keyring names. Other environment values remain literal rather than being guessed secret. Import never writes the source target, never prints values, and rolls back ordinary keyring failures before canonical mutation. A process crash can leave an unreferenced keyring item, but never a literal detected secret in canonical configuration.

Do not report vulnerabilities in public issues. Until a dedicated security contact is published, contact the repository owner privately with reproduction steps and affected versions.
