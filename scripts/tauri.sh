#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "$0")/.." && pwd)"
tauri_cli="$root_dir/node_modules/.bin/tauri"

# Explicitly select the direct-distribution flavor in development so local
# builds keep Google Drive, SSH, and updater behavior enabled.
if [[ "${1:-}" == "dev" ]]; then
  shift
  exec "$tauri_cli" dev --features direct "$@"
fi

exec "$tauri_cli" "$@"
