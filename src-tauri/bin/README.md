## PDU Version: 0.23.0

Only macOS sidecar binaries are bundled:

- `pdu-aarch64-apple-darwin`
- `pdu-x86_64-apple-darwin`

The app passes `--deduplicate-hardlinks` so APFS hard links do not make full
disk scans appear stuck after the progress indicator reaches 100%.
