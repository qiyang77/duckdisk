## PDU Version: 0.23.0 with DuckDisk dual-size patch

Only the Apple silicon macOS sidecar is bundled:

- `pdu-aarch64-apple-darwin`

The app passes `--deduplicate-hardlinks` so APFS hard links do not make full
disk scans appear stuck after the progress indicator reaches 100%.

`scripts/update-pdu.sh` applies `scripts/pdu-dual-size.patch` before building.
The `dual-size` quantity records apparent and allocated bytes from the same
metadata lookup, so it does not add a second filesystem traversal. On macOS,
files carrying the `UF_DATALESS` cloud-placeholder flag, plus zero-block
provider placeholders without that flag, contribute zero bytes to local Size
instead of inflating the disk result with remote-only content.

The patch also preserves an `isDirectory` marker in JSON output. Empty,
unreadable, and depth-truncated directories can therefore retain folder
behavior even when their serialized `children` array is empty.
