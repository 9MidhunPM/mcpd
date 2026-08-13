# Claude Code adapter

Verified against Claude Code's MCP documentation on 2026-08-13. mcpd supports user scope (`claude`, `~/.claude.json`), shared project scope (`claude-project`, `<project>/.mcp.json`), and private local scope (`claude-local`, the canonical project entry in `~/.claude.json`). Project/local targets require explicit mcpd project trust. Claude's own approval UI remains authoritative.

Claude's `${NAME}` expansion is used for canonical `${env:NAME}` values. Keyring-backed stdio values use `mcpd exec`; keyring-backed HTTP values remain unsupported because mcpd is not a proxy. JSONC comments and unmanaged entries are retained. OAuth state remains client-owned and cannot be imported losslessly.

Source: <https://code.claude.com/docs/en/mcp>
