# Codex adapter

Compatibility source last verified: 2026-08-12.

The adapter follows the official [Codex MCP documentation](https://developers.openai.com/codex/mcp): global configuration is located at `~/.codex/config.toml`, MCP servers use `[mcp_servers.<name>]`, and current clients support stdio and Streamable HTTP.

Implemented mappings:

| Canonical | Codex |
| --- | --- |
| stdio `command`, `args`, `cwd` | `command`, `args`, `cwd` |
| literal stdio environment | `env` |
| `${env:NAME}` stdio forwarding | `env_vars` |
| HTTP `url` | `url` |
| literal HTTP headers | `http_headers` |
| `${env:NAME}` HTTP headers | `env_http_headers` |

Import supports stdio `command`, `args`, `cwd`, `env`, and string `env_vars`; HTTP `url`, `http_headers`, and `env_http_headers`. Strong credential-like fields receive deterministic `<server>.<FIELD>` keyring references after one confirmation per server. Non-sensitive literals remain literals. Import rejects fields the canonical model cannot represent and never modifies the Codex file.

Codex declares runtime-injection capability for stdio secrets and native environment-reference capability for HTTP. A secret-bearing stdio entry renders as `mcpd exec SERVER`; raw values never enter Codex configuration. On Linux, mcpd also forwards `DBUS_SESSION_BUS_ADDRESS` through Codex `env_vars` so the wrapper can reach the session keyring. Existing managed wrapper forwarding is retained and merged deterministically. OS-keyring HTTP references are unsupported because Codex cannot resolve them directly, so synchronization fails instead of copying the value.

The schema was rechecked against the official Codex MCP documentation on 2026-08-12. Codex now also documents remote-source `env_vars` objects, `experimental_environment`, authentication, bearer tokens, tool policy, enablement, and timeout fields. Those fields, OAuth state, and project-scoped `.codex/config.toml` are not imported or projected yet. Unrecognized fields in unmanaged server tables are preserved. Once a table is explicitly imported and mcpd-owned, its complete rendered server table is canonical-authoritative.
