# Contributing

Read [AGENTS.md](AGENTS.md) before changing synchronization, adapters, secrets, trust, paths, or filesystem mutation.

Keep changes cohesive and include regression tests for behavior changes. Before submitting a change, run:

```sh
cargo fmt --check
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
git diff --check
```

Ubuntu/Debian contributors need `libdbus-1-dev` and `pkg-config` to build the Linux Secret Service keyring backend.

Adapter compatibility changes must cite the authoritative upstream behavior and update fixtures or integration tests. Never use a developer's real home directory or credentials in tests.
