# Claude Code adapter

Verified against Claude Code's MCP documentation on 2026-08-13. mcpd manages the user-scope `mcpServers` object in `~/.claude.json`. It renders stdio entries with `type = stdio` and Streamable HTTP entries with `type = http`, preserving every unrelated top-level field and unmanaged server.

Claude's `${NAME}` expansion is used for canonical `${env:NAME}` HTTP headers. Keyring-backed stdio values use `mcpd exec`; keyring-backed HTTP values remain unsupported because mcpd is not a proxy. Project/local Claude scopes and OAuth state remain client-owned.

Source: <https://code.claude.com/docs/en/mcp>
