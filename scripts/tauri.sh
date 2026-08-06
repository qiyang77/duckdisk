#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "$0")/.." && pwd)"
tauri_cli="$root_dir/node_modules/.bin/tauri"

# Tauri 1 disables Cargo's default features in dev mode. Explicitly restore the
# direct-distribution flavor so `npm run tauri dev` keeps its existing behavior.
if [[ "${1:-}" == "dev" ]]; then
  shift
  exec "$tauri_cli" dev --features direct "$@"
fi

exec "$tauri_cli" "$@"
