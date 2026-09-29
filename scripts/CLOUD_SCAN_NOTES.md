# Cloud-safe local scans

The `pdu-dual-size.patch` is applied to upstream pdu 0.23.0 by both sidecar
build scripts and the Intel release workflow. Keep the tracked ARM sidecar
and locally generated Intel/universal sidecars in sync with this patch.

On macOS, the scanner sets the process-wide
`IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES` policy to `OFF` before path
inspection or worker creation. It fails closed if the policy cannot be set.
Scan-related cache metadata operations in the app use a thread-bound RAII
guard that restores the previous policy before returning. Neither guard
changes Finder's policy or the user's cloud synchronization settings.

`SF_DATALESS` nodes are retained but not entered. Intentional skips and
materialization-denied (`EDEADLK`) diagnostics are classified separately from
permission failures, attached to matching result nodes, and saved with the
scan cache. The UI displays unknown contents as a dash, not an empty folder.
Logical size is not inferred from zero allocated blocks: ordinary sparse
files retain their logical size. The new cache index version requires a
fresh scan of older cached results.

Cloud API clients have a 10-second connection timeout and 30-second request
timeout (including response-body reads). Scan cancellation drops the entire
pending scan future, checked every 100 ms; this includes token refresh,
network waits and retry sleeps. Existing provider retry behavior remains.
Deletion operations are not attached to scan cancellation.

These safeguards avoid automatic downloads; they do not make protected data
readable or guarantee that every filesystem/provider metadata operation
completes promptly. Remote-only bytes are not local used disk space and must
not be assigned to the `Unscanned` byte difference.

Regression checks:

- `npm run build` and `npm run test:scan-totals`
- `cargo test --manifest-path src-tauri/Cargo.toml`
- `cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features mas,custom-protocol`
- In the patched pdu source: `cargo test --lib` and `cargo test --bin pdu`
- CLI smoke test: a normal 64 MiB zero-block sparse file reports 64 MiB
  apparent size and 0 allocated bytes.

See Apple's [TN3150](https://developer.apple.com/documentation/technotes/tn3150-getting-ready-for-data-less-files)
and the SDK's `sys/resource.h` for the public materialization-policy API.
