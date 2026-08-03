## PDU Version: 0.23.0

Only the Apple silicon macOS sidecar is bundled:

- `pdu-aarch64-apple-darwin`

The app passes `--deduplicate-hardlinks` so APFS hard links do not make full
disk scans appear stuck after the progress indicator reaches 100%.
