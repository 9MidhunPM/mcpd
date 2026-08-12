# OpenChamber / OpenCode adapter

OpenChamber uses the installed OpenCode CLI, so mcpd targets OpenCode's current global JSON configuration at `$XDG_CONFIG_HOME/opencode/opencode.json` (normally `~/.config/opencode/opencode.json`). The adapter uses the native v2 `mcp.servers` shape: `type = local` with a command array, or `type = remote` with a Streamable HTTP URL.

Unrelated OpenCode settings and unmanaged MCP entries are preserved. JSONC files are not rewritten by v0.1; use the JSON location or an explicit `MCPD_OPENCHAMBER_CONFIG` override.

Sources: <https://opencode.ai/v2/docs/config>, <https://opencode.ai/v2/docs/mcp-servers>, <https://github.com/openchamber/openchamber>
