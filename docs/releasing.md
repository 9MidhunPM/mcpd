# Releasing Syncplane

Release only from a clean commit whose crate version matches the intended tag.

1. Run the Rust gate, installer fixture, package dry run, and site build.
2. Confirm the repository is named `9MidhunPM/syncplane` and the public site is
   live on `syncplane.midhunpm.in`.
3. Push an annotated `vVERSION` tag. The release workflow builds the Linux
   archive, checksums, and GitHub provenance.
4. Download the public release into a fresh temporary environment and run the
   checksum-verified installer.
5. Run `cargo publish --dry-run --locked`, then publish with a runtime-only,
   narrowly scoped crates.io token. Never commit, log, or reuse a personal token.
6. Verify `cargo install syncplane --version VERSION` resolves this package.

The initial 1.0.0 release supports Linux x86_64 only. Do not publish when an
adapter can remove unmanaged configuration, canonical recovery is unsafe, or a
security gate is failing.
