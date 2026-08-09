#!/usr/bin/env bash
set -euo pipefail

PDU_VERSION="${1:-0.23.0}"
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PATCH_FILE="${ROOT_DIR}/scripts/pdu-dual-size.patch"
BIN_DIR="${ROOT_DIR}/src-tauri/bin"
BUILD_DIR="$(mktemp -d "${TMPDIR:-/tmp}/duckdisk-pdu.XXXXXX")"
trap 'rm -rf "${BUILD_DIR}"' EXIT

git clone --quiet --depth 1 --branch "${PDU_VERSION}" \
  https://github.com/KSXGitHub/parallel-disk-usage.git "${BUILD_DIR}/source"
patch -d "${BUILD_DIR}/source" -p1 < "${PATCH_FILE}"

cargo build \
  --manifest-path "${BUILD_DIR}/source/Cargo.toml" \
  --release \
  --target aarch64-apple-darwin \
  --bin pdu

install -m 755 \
  "${BUILD_DIR}/source/target/aarch64-apple-darwin/release/pdu" \
  "${BIN_DIR}/pdu-aarch64-apple-darwin"

"${BIN_DIR}/pdu-aarch64-apple-darwin" --help | grep -q "dual-size"
echo "Built DuckDisk pdu ${PDU_VERSION} with dual-size support."
