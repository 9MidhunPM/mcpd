# ADR 0005: Project trust, client scopes, and declarative targets

Status: accepted for v1.0

## Decision

The resolved configuration is global canonical state overlaid by `<project>/.mcpd/config.toml` only when the canonical project directory is present in local trust state. Project roots are discovered from an explicit override or by walking ancestors for `.git`/`.mcpd`. Trust is local, path-bound, and never stored in the repository. Untrusted overlays are ignored before parsing server policy into the resolved graph or resolving any secret reference.

Claude's user, shared-project, and private-local scopes are separate target IDs. This makes the destination and ownership state explicit and prevents a working-directory change from silently redirecting an already-enabled user target. Project/local scopes require current project trust and remain subject to Claude's native project approval.

Simple custom targets use manifests in the mcpd config directory. V1 manifests are intentionally restricted to JSON/JSONC, a safe home-relative/absolute path, and a dot-separated server collection. They are export-only, preserve unmanaged entries, and reject client-specific HTTP secret/environment syntax. Complex transformations, imports, authentication, or migrations require a compiled adapter.

JSONC is parsed with comments and trailing commas while preserving byte positions. Synchronization patches only owned server values and necessary path objects, then reparses the complete rendered document before an atomic write. Duplicate keys are rejected because ownership cannot be determined unambiguously.

## Consequences

- Trust must be re-established after moving a project.
- A system-wide watch service manages global state; project overlays are watched when `mcpd watch` starts in that trusted project context.
- Declarative adapters extend export coverage without allowing arbitrary code hooks.
- Native configuration comments outside mcpd-owned values survive synchronization.
