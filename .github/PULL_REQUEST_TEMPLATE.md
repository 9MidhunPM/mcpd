## Summary

Describe the user-visible behavior and why it belongs in mcpd.

## Safety

- [ ] Unmanaged configuration remains untouched.
- [ ] Dry-run remains write-free.
- [ ] No secret values appear in config, output, logs, fixtures, or state.
- [ ] Path, trust, and compatibility implications are documented.

## Verification

- [ ] `cargo fmt --check`
- [ ] `cargo check --all-targets --all-features`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --all-targets --all-features`
- [ ] `cargo build --release`
- [ ] `git diff --check`
