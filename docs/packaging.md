# Packaging mcpd

The package and binary are both named `mcpd`. The CLI has no required daemon, network service, post-install hook, or root-owned state. It discovers XDG paths at runtime and generates shell completions and the optional user systemd unit on demand.

## Source package (`mcpd`)

Build with Rust 1.85 or newer using the tagged source archive and locked dependencies:

```sh
cargo build --locked --release
install -Dm755 target/release/mcpd "$pkgdir/usr/bin/mcpd"
```

On Linux, the keyring Secret Service backend requires D-Bus development metadata at build time (`dbus`/`libdbus-1-dev` and `pkgconf`/`pkg-config`, depending on the distribution). Runtime packages should retain the platform D-Bus/keyring library dependency.

## Binary package (`mcpd-bin`)

GitHub releases publish `mcpd-vVERSION-x86_64-unknown-linux-gnu.tar.gz` and `checksums.txt`. Verify the SHA-256 entry before extracting and install only the `mcpd` executable; README, LICENSE, and CHANGELOG are included for package metadata.

## Git package (`mcpd-git`)

Build the selected Git revision with `cargo build --locked --release`. Record a revision-derived package version without changing `Cargo.toml`. Do not run tests against a packager's real home or keyring; the repository integration tests create isolated temporary homes and use the debug-only mock backend.

Completions may be generated in `package()` with `mcpd completions <shell>`. The systemd unit contains the installed executable's absolute path, so generate it after installation or let users run `mcpd systemd install`; do not ship a root/system unit.
