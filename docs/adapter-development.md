# Adapter development

Built-in adapters own detection, path selection, native parsing/rendering, imports, capabilities, and compatibility diagnostics. Generic sync owns locking, backups, atomic writes, failure isolation, ownership, and diff presentation.

For a simple JSON/JSONC client, prefer a declarative manifest under `~/.config/syncplane/targets`. It must use a unique ID, a path below the user's home directory, and a dot-separated object path. Declarative adapters are export-only and intentionally cannot run hooks or shell commands.

A compiled adapter must fail closed on malformed or version-incompatible input, reject symlinks outside its documented policy, preserve unmanaged entries, round-trip every imported field losslessly, and provide tests for empty, managed, unmanaged, mixed, malformed, remote, stdio, secret, and version-sensitive input. Never put client quirks into the generic synchronization engine.
