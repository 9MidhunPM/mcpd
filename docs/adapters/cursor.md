# Cursor adapter

Verified against Cursor's MCP documentation and current configuration behavior on 2026-08-13. syncplane manages `~/.cursor/mcp.json`, using `command`/`args` for stdio and `url`/`headers` for Streamable HTTP. Cursor infers the transport from `command` or `url`; syncplane therefore omits redundant transport fields.

Unmanaged `mcpServers` entries and unrelated settings are preserved. Stdio secrets use `syncplane exec`; HTTP keyring values are rejected unless represented through Cursor's native environment substitution.

Source: <https://cursor.com/docs/context/mcp>
