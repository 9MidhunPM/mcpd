# ADR 0004: Keyring secrets and late stdio runtime injection

Status: accepted for v1.0

## Decision

Canonical environment and header values are typed as either non-secret literals or symbolic keyring references. References serialize as `{ secret = "NAME" }`; raw values are never obtained while parsing, planning, diffing, syncing, or rendering output. Obvious secret-bearing field names reject literal canonical values.

The secret subsystem owns a backend trait. Production uses the platform keyring through the Rust `keyring` crate. Tests use an isolated backend enabled only in debug builds, plus an in-memory backend for unit tests. A mode-0600 registry stores names for listing; it never stores values.

Target adapters declare separate stdio and HTTP secret capabilities. Codex uses runtime injection for stdio: target configuration contains `syncplane exec SERVER`, and `exec` resolves keyring references immediately before replacing itself with the real command. On Linux, Codex's stdio environment allowlist requires the wrapper to forward `DBUS_SESSION_BUS_ADDRESS`; syncplane adds that name through `env_vars` without serializing its value and merges valid forwarding already present on an owned wrapper. HTTP keyring references are rejected because Codex cannot consume the OS keyring and syncplane is not an HTTP proxy. Native environment references remain available.

Import scans adapter-native fields and classifies a literal as secret only when its normalized name is `Authorization` or ends with a strong credential suffix such as `_API_KEY`, `_TOKEN`, `_PASSWORD`, `_SECRET`, `_CREDENTIAL`, or `_PRIVATE_KEY`. Other literals remain canonical literals. Detected fields are grouped per server and derive `<server>.<FIELD>` keyring identifiers; terminal use confirms once per server, while non-interactive and dry-run use the deterministic names without prompting. Explicit `--secret FIELD=NAME` remains an override. Import validates all target and canonical input before secret writes, rolls back ordinary pre-canonical failures, writes canonical configuration atomically, then commits ownership. The source target remains read-only.

## Consequences

- Secret values exist only in the keyring and the final child-process environment.
- Generated target files, canonical configuration, state, pending records, and normal output contain names but not values.
- `sync --dry-run` and import dry-run never access values or mutate the keyring.
- A hard process crash during import may leave an unreferenced keyring item, which is safer than a canonical reference with no recoverable value; ordinary returned failures are rolled back.
- Future adapters choose capabilities without embedding their policy in generic synchronization code.
