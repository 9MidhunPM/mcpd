# Changelog

All notable changes to syncplane are documented here. The project follows semantic versioning.

## 1.0.0 - 2026-08-13

### Added

- Safe synchronization and import for Codex, Claude Code, Cursor, Antigravity, and OpenChamber/OpenCode.
- Canonical stdio and Streamable HTTP servers, OS-keyring references, guarded `secret get --reveal`, and late `syncplane exec` injection.
- Trusted project overlays with deterministic precedence and explicit trust/revocation commands.
- Claude user, project, and project-local scopes.
- JSONC parsing and ownership-region edits that retain comments, trailing-comma compatibility, unmanaged servers, and unrelated settings.
- Export-only declarative JSON/JSONC target manifests for simple custom clients.
- Repeatable `--target` filtering for `status`, `diff`, `sync`, and `doctor`.
- Watch mode, user-level systemd unit generation/installation, shell completions, backups, drift status, and machine-readable output.
- GitHub Actions CI with Linux keyring build dependencies and a tagged release workflow producing a versioned archive and SHA-256 checksums.

### Safety

- Target writes are atomic, backed up with restrictive permissions, and committed to ownership state only after success.
- Dry runs do not write files, locks, state, backups, or keyring values.
- Unmanaged name collisions, malformed files, duplicate JSONC keys, unsafe symlinks, and ambiguous Antigravity paths fail closed.
- Multi-target failures are isolated: successful targets remain committed and every failure is reported.
- Missing canonical configuration can no longer be reinitialized empty while ownership state records managed servers.
- Schema-update transactions preserve all canonical and owned servers, create private backups, and reject lossy rewrites before mutation.
- Managed removals are blocked during ordinary sync, require explicit authorization, and are listed prominently by status, diff, and dry-run output.
