#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root_dir"

export CI=true
export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:-Developer ID Application: Qi Yang (PH4H68P6G5)}"

if [[ -z "${TAURI_PRIVATE_KEY:-}" && -f "$HOME/.tauri/duckdisk-updater.key" ]]; then
  TAURI_PRIVATE_KEY="$(cat "$HOME/.tauri/duckdisk-updater.key")"
fi
if [[ -z "${TAURI_KEY_PASSWORD:-}" ]]; then
  TAURI_KEY_PASSWORD="$(security find-generic-password \
    -a "$USER" \
    -s duckdisk-updater-key-password \
    -w 2>/dev/null || true)"
fi
: "${TAURI_PRIVATE_KEY:?Set TAURI_PRIVATE_KEY to sign automatic update artifacts}"
: "${TAURI_KEY_PASSWORD:?Set TAURI_KEY_PASSWORD to sign automatic update artifacts}"

# Tauri 2 renamed the updater signing variables. Keep the repository and
# GitHub secret names stable while exporting the names consumed by the v2 CLI.
export TAURI_SIGNING_PRIVATE_KEY="$TAURI_PRIVATE_KEY"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$TAURI_KEY_PASSWORD"

version="$(node -p "require('./package.json').version")"
arch="$(uname -m)"
skip_notarization="${SKIP_NOTARIZATION:-0}"
if [[ "$arch" == "aarch64" ]]; then
  arch="arm64"
fi

if [[ "$skip_notarization" == "1" ]]; then
  notary_args=()
elif [[ -n "${APPLE_NOTARY_PROFILE:-}" ]]; then
  notary_args=(--keychain-profile "$APPLE_NOTARY_PROFILE")
elif [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
  notary_args=(
    --apple-id "$APPLE_ID"
    --password "$APPLE_PASSWORD"
    --team-id "$APPLE_TEAM_ID"
  )
else
  notary_args=(--keychain-profile duckdisk-notary)
fi

echo "Building and signing DuckDisk.app..."
# Keep notarization credentials away from the bundler and submit the cleaned,
# re-signed app with notarytool below.
env -u APPLE_ID -u APPLE_PASSWORD -u APPLE_TEAM_ID \
  npm run tauri -- build --bundles app

source_app="src-tauri/target/release/bundle/macos/DuckDisk.app"
test -d "$source_app"

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/duckdisk-release.XXXXXX")"
mount_dir="$work_dir/mount"
mounted=false

cleanup() {
  if [[ "$mounted" == true ]]; then
    hdiutil detach "$mount_dir" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

clean_app="$work_dir/DuckDisk.app"
ditto --noextattr --norsrc "$source_app" "$clean_app"
xattr -cr "$clean_app"
codesign --force --deep --options runtime --timestamp \
  --sign "$APPLE_SIGNING_IDENTITY" "$clean_app"
codesign --verify --deep --strict --verbose=2 "$clean_app"
codesign_details="$(codesign -d --verbose=4 "$clean_app" 2>&1)"
grep -q 'flags=.*runtime' <<< "$codesign_details"

if [[ "$skip_notarization" == "1" ]]; then
  echo "Skipping DuckDisk.app notarization by request."
else
  echo "Notarizing DuckDisk.app..."
  app_zip="$work_dir/DuckDisk.zip"
  ditto -c -k --keepParent "$clean_app" "$app_zip"
  xcrun notarytool submit "$app_zip" "${notary_args[@]}" --wait
  xcrun stapler staple "$clean_app"
  xcrun stapler validate "$clean_app"
  spctl --assess --type execute --verbose=4 "$clean_app"
fi

echo "Creating signed updater artifact..."
updater_archive="$work_dir/DuckDisk_${version}_${arch}.app.tar.gz"
COPYFILE_DISABLE=1 tar -czf "$updater_archive" -C "$work_dir" DuckDisk.app
updater_key_file="$work_dir/duckdisk-updater.key"
printf '%s' "$TAURI_PRIVATE_KEY" > "$updater_key_file"
chmod 600 "$updater_key_file"
env \
  -u TAURI_PRIVATE_KEY \
  -u TAURI_KEY_PASSWORD \
  -u TAURI_SIGNING_PRIVATE_KEY \
  -u TAURI_SIGNING_PRIVATE_KEY_PASSWORD \
  "$root_dir/node_modules/.bin/tauri" signer sign \
  --private-key-path "$updater_key_file" \
  --password "$TAURI_KEY_PASSWORD" \
  "$updater_archive"
test -f "$updater_archive.sig"

updater_verify_dir="$work_dir/updater-verify"
mkdir -p "$updater_verify_dir"
tar -xzf "$updater_archive" -C "$updater_verify_dir"
codesign --verify --deep --strict --verbose=2 \
  "$updater_verify_dir/DuckDisk.app"
if [[ "$skip_notarization" != "1" ]]; then
  xcrun stapler validate "$updater_verify_dir/DuckDisk.app"
  spctl --assess --type execute --verbose=4 \
    "$updater_verify_dir/DuckDisk.app"
fi

echo "Creating signed DMG..."
image_size_mb="$(( $(du -sm "$clean_app" | awk '{print $1}') + 24 ))"
rw_dmg="$work_dir/DuckDisk-rw.dmg"
hdiutil create -size "${image_size_mb}m" -fs HFS+ -volname DuckDisk "$rw_dmg"

mkdir -p "$mount_dir"
hdiutil attach -nobrowse -mountpoint "$mount_dir" "$rw_dmg" >/dev/null
mounted=true
ditto --noextattr --norsrc "$clean_app" "$mount_dir/DuckDisk.app"
ln -s /Applications "$mount_dir/Applications"
xattr -cr "$mount_dir/DuckDisk.app"
codesign --verify --deep --strict --verbose=2 "$mount_dir/DuckDisk.app"
hdiutil detach "$mount_dir" >/dev/null
mounted=false

compressed_dmg="$work_dir/DuckDisk_${version}_${arch}.dmg"
hdiutil convert "$rw_dmg" -format UDZO -imagekey zlib-level=9 -o "$compressed_dmg" >/dev/null
codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$compressed_dmg"

if [[ "$skip_notarization" == "1" ]]; then
  echo "Skipping DMG notarization by request."
else
  echo "Notarizing DMG..."
  xcrun notarytool submit "$compressed_dmg" "${notary_args[@]}" --wait
  xcrun stapler staple "$compressed_dmg"
  xcrun stapler validate "$compressed_dmg"
fi
hdiutil verify "$compressed_dmg"
codesign --verify --verbose=2 "$compressed_dmg"
if [[ "$skip_notarization" != "1" ]]; then
  spctl --assess --type open --context context:primary-signature --verbose=4 "$compressed_dmg"
fi

dmg_dir="src-tauri/target/release/bundle/dmg"
mkdir -p "$dmg_dir"
final_dmg="$dmg_dir/DuckDisk_${version}_${arch}.dmg"
ditto --noextattr "$compressed_dmg" "$final_dmg"

updater_dir="src-tauri/target/release/bundle/updater"
mkdir -p "$updater_dir"
final_updater="$updater_dir/DuckDisk_${version}_${arch}.app.tar.gz"
final_updater_signature="$final_updater.sig"
ditto --noextattr "$updater_archive" "$final_updater"
ditto --noextattr "$updater_archive.sig" "$final_updater_signature"

echo "Release DMG: $final_dmg"
echo "Updater archive: $final_updater"
echo "Updater signature: $final_updater_signature"
