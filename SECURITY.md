# Security policy

`mcpd` edits configuration consumed by programs that can launch local processes or contact remote services. Treat canonical and project MCP configuration as executable policy.

## v1 security boundary

- Global canonical configuration and explicitly trusted project overlays are supported. Untrusted overlays are ignored before their server or secret references are resolved.
- Built-in user target files and their existing path components must be regular paths below the configured home directory. Trusted Claude project files are confined to that canonical project root. Symbolic links are rejected.
- Sync and static diagnostics never launch a client or MCP server. Only explicit `mcpd exec SERVER` replaces itself with a configured stdio process.
- Dry-run does not create locks, directories, backups, target files, or ownership state.
- Existing malformed targets, duplicate JSONC keys, ambiguous Antigravity paths, and unmanaged name collisions stop that target before a backup or write.
- Target writes use a same-directory temporary file, file sync, atomic rename, and parent-directory sync.
- Ownership state is committed only after the target write. A secret-free pending record recovers interruption between those operations.
- Existing target backups are opaque full-file snapshots stored with mode `0600`; backup directories and state directories use mode `0700`. Backup contents are never included in metadata or command output.

Synchronization is atomic per target, not across targets. A failure does not roll back independent successful targets and does not authorize mutation of the failed target. The implementation checks that each target snapshot still matches the planned input immediately before writing. A residual time-of-check/time-of-use window remains between that check and rename; eliminating it portably requires a future platform-specific filesystem layer.

Project trust is stored by canonical directory path in mode-`0600` state beneath a mode-`0700` directory. Moving a repository does not transfer trust. Claude project/local scopes additionally require current project trust. Client-native approval prompts, OAuth state, and remote MCP authentication remain client-owned.

## Secrets

Secret values are stored through the operating-system keyring and are never included in the canonical file, ownership state, transaction records, diagnostics, or generated Codex configuration. Canonical structured references use `{ secret = "NAME" }`.

Secret-bearing stdio servers are projected to `mcpd exec SERVER`. Values are resolved only immediately before replacing that process with the real configured command and are injected only into its environment. Sync, diff, status, and ordinary doctor checks do not launch servers or resolve secret values.

Targets that cannot resolve OS-keyring values for HTTP headers reject that projection rather than copying credentials or implementing a proxy. Documented client-native environment references remain supported. Declarative targets reject HTTP environment/keyring references because their placeholder syntax is not known.

Import identifies only strong credential-name suffixes automatically, groups confirmation per server, and derives deterministic `<server>.<FIELD>` keyring names. Other environment values remain literal rather than being guessed secret. Import never writes the source target, never prints values, and rolls back ordinary keyring failures before canonical mutation. A process crash can leave an unreferenced keyring item, but never a literal detected secret in canonical configuration.

Do not report vulnerabilities in public issues. Until a dedicated security contact is published, contact the repository owner privately with reproduction steps and affected versions.
