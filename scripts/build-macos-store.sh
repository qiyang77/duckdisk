#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root_dir"

export CI=true
version="$(node -p "require('./package.json').version")"
build_number="${MAS_BUILD_NUMBER:-510}"
store_work_dir="$(mktemp -d "${TMPDIR:-/tmp}/duckdisk-mas.XXXXXX")"
store_source_dir="$store_work_dir/source"
store_output_dir="$root_dir/src-tauri/target/mas-store"

cleanup() {
  rm -rf "$store_work_dir"
}
trap cleanup EXIT

mkdir -p "$store_source_dir"
rsync -a \
  --exclude '.git' \
  --exclude 'dist' \
  --exclude 'node_modules' \
  --exclude 'src-tauri/target' \
  "$root_dir/" "$store_source_dir/"

ln -s "$root_dir/node_modules" "$store_source_dir/node_modules"

# Build the MAS flavor in an isolated copy so disabling the direct feature does
# not alter the normal website-distribution manifest or its Google integration.
perl -0pi -e 's/default = \["custom-protocol", "direct"\]/default = ["custom-protocol"]/' \
  "$store_source_dir/src-tauri/Cargo.toml"

if grep -Fq '"macos-private-api"' "$store_source_dir/src-tauri/Cargo.toml"; then
  echo "MAS manifest still enables macos-private-api" >&2
  exit 1
fi

export CARGO_TARGET_DIR="$root_dir/src-tauri/target/mas"
(
  cd "$store_source_dir"
  VITE_DISTRIBUTION=mas VITE_GOOGLE_DRIVE_ENABLED=false \
    npm run tauri -- build \
      --ci \
      --features mas \
      --bundles app \
      --config src-tauri/tauri.mas.conf.json
)

(
  cd "$store_source_dir/src-tauri"
  export TAURI_CONFIG="$(tr -d '\n' < tauri.mas.conf.json)"
  cargo test --release --no-default-features --features mas
)

source_app="$CARGO_TARGET_DIR/release/bundle/macos/DuckDisk.app"
test -d "$source_app"

# App Review rejects binaries that link the private IOHID temperature APIs used
# by sysinfo unless its Apple App Store compatibility feature is enabled. Scan
# every Mach-O in the bundle so a future dependency change cannot regress this.
prohibited_api_pattern='IOHIDEventGetFloatValue|IOHIDEventSystemClientCreate|IOHIDEventSystemClientSetMatching|IOHIDServiceClientCopyEvent'
while IFS= read -r candidate; do
  if file "$candidate" | grep -q 'Mach-O'; then
    undefined_symbols="$(nm -u "$candidate" 2>/dev/null || true)"
    if grep -Eq "$prohibited_api_pattern" <<< "$undefined_symbols"; then
      echo "Mac App Store bundle references a prohibited private IOHID API: $candidate" >&2
      grep -E "$prohibited_api_pattern" <<< "$undefined_symbols" >&2
      exit 1
    fi
  fi
done < <(find "$source_app/Contents" -type f)

mkdir -p "$store_output_dir"
prepared_app="$store_work_dir/DuckDisk.app"
ditto --noextattr --norsrc "$source_app" "$prepared_app"
xattr -cr "$prepared_app"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $build_number" \
  "$prepared_app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Delete :LSRequiresCarbon" \
  "$prepared_app/Contents/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Delete :NSAppTransportSecurity" \
  "$prepared_app/Contents/Info.plist" 2>/dev/null || true

# A loose .app under Desktop/iCloud can reacquire com.apple.FinderInfo after it
# has been signed. Keep signing and packaging in the temporary local directory,
# and export apps as ZIP files so the verified contents remain stable.
legacy_unsigned_app="$store_output_dir/DuckDisk-${version}-MAS-unsigned.app"
legacy_signed_app="$store_output_dir/DuckDisk-${version}-MAS.app"
rm -rf "$legacy_unsigned_app" "$legacy_signed_app"

if [[ "${MAS_SKIP_SIGNING:-0}" == "1" ]]; then
  codesign --force --options runtime --sign - \
    --entitlements "$root_dir/src-tauri/entitlements.helper.mas.plist" \
    "$prepared_app/Contents/MacOS/pdu"
  codesign --force --options runtime --sign - \
    --entitlements "$root_dir/src-tauri/entitlements.mas.plist" \
    "$prepared_app"
  codesign --verify --deep --strict --verbose=2 "$prepared_app"

  unsigned_zip="$store_output_dir/DuckDisk-${version}-MAS-unsigned.zip"
  rm -f "$unsigned_zip"
  ditto -c -k --keepParent --norsrc --noextattr --noqtn --noacl \
    "$prepared_app" "$unsigned_zip"
  verify_dir="$store_work_dir/verify-unsigned"
  mkdir -p "$verify_dir"
  ditto -x -k "$unsigned_zip" "$verify_dir"
  codesign --verify --deep --strict --verbose=2 "$verify_dir/DuckDisk.app"
  echo "Ad-hoc signed MAS app archive: $unsigned_zip"
  exit 0
fi

: "${MAS_APP_SIGNING_IDENTITY:?Set MAS_APP_SIGNING_IDENTITY to a Mac App Distribution identity}"
: "${MAS_INSTALLER_SIGNING_IDENTITY:?Set MAS_INSTALLER_SIGNING_IDENTITY to a Mac Installer Distribution identity}"
: "${MAS_TEAM_ID:?Set MAS_TEAM_ID to the Apple Developer Team ID}"

profile_path="$store_work_dir/DuckDisk.provisionprofile"
if [[ -n "${MAS_PROVISIONING_PROFILE_PATH:-}" ]]; then
  cp "$MAS_PROVISIONING_PROFILE_PATH" "$profile_path"
elif [[ -n "${MAS_PROVISIONING_PROFILE:-}" ]]; then
  printf '%s' "$MAS_PROVISIONING_PROFILE" | base64 --decode > "$profile_path"
else
  echo "Set MAS_PROVISIONING_PROFILE_PATH or MAS_PROVISIONING_PROFILE" >&2
  exit 1
fi
cp "$profile_path" "$prepared_app/Contents/embedded.provisionprofile"

profile_plist="$store_work_dir/provisioning-profile.plist"
security cms -D -i "$profile_path" > "$profile_plist"
profile_app_identifier="$(/usr/libexec/PlistBuddy -c \
  'Print :Entitlements:com.apple.application-identifier' "$profile_plist")"
profile_team_identifier="$(/usr/libexec/PlistBuddy -c \
  'Print :Entitlements:com.apple.developer.team-identifier' "$profile_plist")"
profile_bundle_identifier="${profile_app_identifier#*.}"
if [[ "$profile_bundle_identifier" != 'com.duckdisk.app' ]]; then
  echo "Provisioning profile does not authorize com.duckdisk.app" >&2
  exit 1
fi
if [[ "$profile_team_identifier" != "$MAS_TEAM_ID" ]]; then
  echo "Provisioning profile team does not match MAS_TEAM_ID" >&2
  exit 1
fi

main_entitlements="$store_work_dir/entitlements.mas.plist"
cp "$root_dir/src-tauri/entitlements.mas.plist" "$main_entitlements"
/usr/libexec/PlistBuddy -c \
  "Add :com.apple.application-identifier string $profile_app_identifier" \
  "$main_entitlements"
/usr/libexec/PlistBuddy -c \
  "Add :com.apple.developer.team-identifier string $profile_team_identifier" \
  "$main_entitlements"
/usr/libexec/PlistBuddy -c \
  "Add :keychain-access-groups array" \
  "$main_entitlements"
/usr/libexec/PlistBuddy -c \
  "Add :keychain-access-groups:0 string $profile_app_identifier" \
  "$main_entitlements"

codesign --force --options runtime --timestamp \
  --entitlements "$root_dir/src-tauri/entitlements.helper.mas.plist" \
  --sign "$MAS_APP_SIGNING_IDENTITY" \
  "$prepared_app/Contents/MacOS/pdu"
codesign --force --options runtime --timestamp \
  --entitlements "$main_entitlements" \
  --sign "$MAS_APP_SIGNING_IDENTITY" \
  "$prepared_app"
codesign --verify --deep --strict --verbose=2 "$prepared_app"

package_path="$store_output_dir/DuckDisk-${version}-MAS.pkg"
staged_package="$store_work_dir/DuckDisk-${version}-MAS.pkg"
rm -f "$package_path"
productbuild \
  --component "$prepared_app" /Applications \
  --sign "$MAS_INSTALLER_SIGNING_IDENTITY" \
  "$staged_package"
pkgutil --check-signature "$staged_package"
ditto --noextattr --norsrc "$staged_package" "$package_path"
pkgutil --check-signature "$package_path"

echo "Mac App Store package: $package_path"
