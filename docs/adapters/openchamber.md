# OpenCode adapter

`mcpd` targets OpenCode's current global configuration under `$XDG_CONFIG_HOME/opencode` (normally `~/.config/opencode`). `opencode.jsonc` is preferred when it exists; `opencode.json` is used only when JSONC is absent. If both exist, mcpd uses JSONC exclusively and warns that the JSON file is ignored. The adapter never merges or writes both files. OpenCode's released schema puts server names directly under `mcp`: local entries use `type = local` and a command array; remote entries use `type = remote` and a Streamable HTTP URL. mcpd writes `enabled = true` explicitly.

Unrelated OpenCode settings, JSONC comments, trailing commas, and unmanaged MCP entries are preserved in the active file. `mcpd doctor --target opencode` reports the active path. Use `MCPD_OPENCODE_CONFIG` for an explicit compatible configuration path; `MCPD_OPENCHAMBER_CONFIG` and the `openchamber` target name remain deprecated compatibility aliases.

Sources: <https://opencode.ai/docs/config>, <https://opencode.ai/docs/mcp-servers/>, <https://github.com/anomalyco/opencode>

Compatibility source last verified: 2026-08-16.
