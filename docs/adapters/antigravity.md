# Antigravity adapter

Verified against Google's Antigravity 2.0 documentation on 2026-08-13. mcpd manages the shared user configuration at `~/.gemini/config/mcp_config.json`. Local servers use `command`, `args`, and `env`; remote servers use `serverUrl` and optional `headers`.

Google also documents older/product-specific locations such as `~/.gemini/antigravity/mcp_config.json`. mcpd deliberately targets the current shared 2.0 location and supports `MCPD_ANTIGRAVITY_CONFIG` for explicit compatibility. Native OAuth objects are preserved as unmanaged but are not imported into the canonical v0.1 model.

Sources: <https://codelabs.developers.google.com/getting-started-google-antigravity>, <https://developers.google.com/workspace/guides/configure-mcp-servers>
