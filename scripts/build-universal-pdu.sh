#!/usr/bin/env bash
set -euo pipefail

pdu_version="${1:-0.23.0}"
root_dir="$(cd "$(dirname "$0")/.." && pwd)"
patch_file="$root_dir/scripts/pdu-dual-size.patch"
output="$root_dir/src-tauri/bin/pdu-universal-apple-darwin"
x86_output="$root_dir/src-tauri/bin/pdu-x86_64-apple-darwin"
build_dir="$(mktemp -d "${TMPDIR:-/tmp}/duckdisk-pdu-universal.XXXXXX")"

cleanup() {
  rm -rf "$build_dir"
}
trap cleanup EXIT

git clone --quiet --depth 1 --branch "$pdu_version" \
  https://github.com/KSXGitHub/parallel-disk-usage.git "$build_dir/source"
patch -d "$build_dir/source" -p1 < "$patch_file"

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build \
    --manifest-path "$build_dir/source/Cargo.toml" \
    --release \
    --target "$target" \
    --bin pdu
done

install -m 755 \
  "$build_dir/source/target/x86_64-apple-darwin/release/pdu" \
  "$x86_output"
file "$x86_output" | grep -q "x86_64"

lipo -create \
  "$build_dir/source/target/aarch64-apple-darwin/release/pdu" \
  "$build_dir/source/target/x86_64-apple-darwin/release/pdu" \
  -output "$output"
chmod 755 "$output"
lipo "$output" -verify_arch arm64 x86_64
"$output" --help | grep -q "dual-size"

echo "Built universal DuckDisk pdu $pdu_version: $(lipo -archs "$output")"
