# Antigravity adapter

Verified against Google's Antigravity 2.0 documentation on 2026-08-13. mcpd manages the shared user configuration at `~/.gemini/config/mcp_config.json`. Local servers use `command`, `args`, and `env`; remote servers use `serverUrl` and optional `headers`.

Google also documents product-specific locations at `~/.gemini/antigravity/mcp_config.json` and `~/.gemini/antigravity-cli/mcp_config.json`. mcpd uses the sole existing documented location, defaults new configuration to the current shared path, and fails closed if multiple candidates exist. Set `MCPD_ANTIGRAVITY_CONFIG` to resolve an intentional multi-install setup. Native OAuth objects are preserved as unmanaged but are not imported into the canonical v1 model.

Sources: <https://codelabs.developers.google.com/gemini-mcp-agy>, <https://developers.google.com/workspace/chat/api/guides/configure-mcp-server>
