#!/usr/bin/env bash
set -euo pipefail

version="${1:?Pass the App Store version}"
build_number="${2:?Pass the App Store build number}"
whats_new="${3:-Fixed several bugs.}"
bundle_id="${MAS_BUNDLE_ID:-com.duckdisk.app}"
api_root="https://api.appstoreconnect.apple.com"

: "${APP_STORE_CONNECT_KEY_ID:?Set APP_STORE_CONNECT_KEY_ID}"
: "${APP_STORE_CONNECT_ISSUER_ID:?Set APP_STORE_CONNECT_ISSUER_ID}"

private_key_path="${APP_STORE_CONNECT_PRIVATE_KEY_PATH:-$HOME/.appstoreconnect/private_keys/AuthKey_${APP_STORE_CONNECT_KEY_ID}.p8}"
test -f "$private_key_path"

response_file="$(mktemp)"
cleanup() {
  rm -f "$response_file"
}
trap cleanup EXIT

urlencode() {
  jq -rn --arg value "$1" '$value | @uri'
}

authorization_token() {
  ruby -ropenssl -rjson -rbase64 -e '
    def b64url(value)
      Base64.urlsafe_encode64(value, padding: false)
    end

    key_path, key_id, issuer_id = ARGV
    now = Time.now.to_i
    header = b64url({ alg: "ES256", kid: key_id, typ: "JWT" }.to_json)
    payload = b64url({
      iss: issuer_id,
      iat: now,
      exp: now + 1200,
      aud: "appstoreconnect-v1"
    }.to_json)
    signing_input = "#{header}.#{payload}"
    key = OpenSSL::PKey.read(File.read(key_path))
    der_signature = key.dsa_sign_asn1(OpenSSL::Digest::SHA256.digest(signing_input))
    sequence = OpenSSL::ASN1.decode(der_signature)
    raw_signature = sequence.value.map do |integer|
      value = integer.value.to_s(16)
      value = "0#{value}" if value.length.odd?
      [value].pack("H*").rjust(32, "\0")
    end.join
    puts "#{signing_input}.#{b64url(raw_signature)}"
  ' "$private_key_path" "$APP_STORE_CONNECT_KEY_ID" "$APP_STORE_CONNECT_ISSUER_ID"
}

asc_request() {
  local method="$1"
  local path="$2"
  local body="${3:-}"
  local token
  local status
  token="$(authorization_token)"

  if [[ -n "$body" ]]; then
    status="$(curl --silent --show-error \
      --output "$response_file" \
      --write-out '%{http_code}' \
      --request "$method" \
      --header "Authorization: Bearer $token" \
      --header 'Content-Type: application/json' \
      --data "$body" \
      "$api_root$path")"
  else
    status="$(curl --silent --show-error \
      --output "$response_file" \
      --write-out '%{http_code}' \
      --request "$method" \
      --header "Authorization: Bearer $token" \
      "$api_root$path")"
  fi

  if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
    echo "App Store Connect API $method $path failed with HTTP $status" >&2
    jq -r '.errors[]? | [.status, .code, .title, .detail] | @tsv' \
      "$response_file" >&2 || cat "$response_file" >&2
    exit 1
  fi
}

encoded_bundle_id="$(urlencode "$bundle_id")"
asc_request GET "/v1/apps?filter%5BbundleId%5D=$encoded_bundle_id&limit=1"
app_id="$(jq -er '.data[0].id' "$response_file")"

encoded_version="$(urlencode "$version")"
asc_request GET "/v1/apps/$app_id/appStoreVersions?filter%5Bplatform%5D=MAC_OS&filter%5BversionString%5D=$encoded_version&limit=1"
version_id="$(jq -r '.data[0].id // empty' "$response_file")"

if [[ -z "$version_id" ]]; then
  version_body="$(jq -cn \
    --arg version "$version" \
    --arg app_id "$app_id" \
    '{
      data: {
        type: "appStoreVersions",
        attributes: {
          platform: "MAC_OS",
          versionString: $version,
          releaseType: "AFTER_APPROVAL"
        },
        relationships: {
          app: { data: { type: "apps", id: $app_id } }
        }
      }
    }')"
  asc_request POST "/v1/appStoreVersions" "$version_body"
  version_id="$(jq -er '.data.id' "$response_file")"
  echo "Created App Store version $version ($version_id)"
else
  echo "Using existing App Store version $version ($version_id)"
fi

asc_request GET "/v1/appStoreVersions/$version_id/appStoreVersionLocalizations?limit=200"
localizations="$(jq -c '.data' "$response_file")"

if [[ "$(jq 'length' <<< "$localizations")" -eq 0 ]]; then
  asc_request GET "/v1/apps/$app_id/appStoreVersions?filter%5Bplatform%5D=MAC_OS&sort=-versionString&limit=20"
  previous_version_id="$(jq -r --arg current "$version_id" \
    'first(.data[] | select(.id != $current) | .id) // empty' \
    "$response_file")"
  if [[ -z "$previous_version_id" ]]; then
    echo "No previous macOS version is available to copy localization metadata" >&2
    exit 1
  fi

  asc_request GET "/v1/appStoreVersions/$previous_version_id/appStoreVersionLocalizations?limit=200"
  previous_localizations="$(jq -c '.data' "$response_file")"
  if [[ "$(jq 'length' <<< "$previous_localizations")" -eq 0 ]]; then
    echo "The previous macOS version has no localization metadata" >&2
    exit 1
  fi

  while IFS= read -r localization; do
    attributes="$(jq -c --arg whats_new "$whats_new" '
      .attributes
      | {
          locale,
          description,
          keywords,
          marketingUrl,
          promotionalText,
          supportUrl,
          whatsNew: $whats_new
        }
      | with_entries(select(.value != null))
    ' <<< "$localization")"
    localization_body="$(jq -cn \
      --argjson attributes "$attributes" \
      --arg version_id "$version_id" \
      '{
        data: {
          type: "appStoreVersionLocalizations",
          attributes: $attributes,
          relationships: {
            appStoreVersion: {
              data: { type: "appStoreVersions", id: $version_id }
            }
          }
        }
      }')"
    asc_request POST "/v1/appStoreVersionLocalizations" "$localization_body"
  done < <(jq -c '.[]' <<< "$previous_localizations")
else
  while IFS= read -r localization_id; do
    localization_body="$(jq -cn \
      --arg id "$localization_id" \
      --arg whats_new "$whats_new" \
      '{
        data: {
          type: "appStoreVersionLocalizations",
          id: $id,
          attributes: { whatsNew: $whats_new }
        }
      }')"
    asc_request PATCH "/v1/appStoreVersionLocalizations/$localization_id" "$localization_body"
  done < <(jq -r '.[].id' <<< "$localizations")
fi
echo "Updated What's New to: $whats_new"

encoded_build_number="$(urlencode "$build_number")"
build_id=""
for attempt in $(seq 1 60); do
  asc_request GET "/v1/builds?filter%5Bapp%5D=$app_id&filter%5Bversion%5D=$encoded_build_number&sort=-uploadedDate&limit=10"
  build_id="$(jq -r '
    first(
      .data[]
      | select(.attributes.processingState == "VALID" and .attributes.expired == false)
      | .id
    ) // empty
  ' "$response_file")"
  if [[ -n "$build_id" ]]; then
    break
  fi

  failed_state="$(jq -r '
    first(
      .data[]?
      | select(.attributes.processingState == "FAILED" or .attributes.processingState == "INVALID")
      | .attributes.processingState
    ) // empty
  ' "$response_file")"
  if [[ -n "$failed_state" ]]; then
    echo "App Store build $build_number entered state $failed_state" >&2
    exit 1
  fi

  echo "Waiting for App Store build $build_number to finish processing ($attempt/60)..."
  sleep 30
done

if [[ -z "$build_id" ]]; then
  echo "Timed out waiting for App Store build $build_number" >&2
  exit 1
fi

build_body="$(jq -cn \
  --arg build_id "$build_id" \
  '{ data: { type: "builds", id: $build_id } }')"
asc_request PATCH "/v1/appStoreVersions/$version_id/relationships/build" "$build_body"
echo "Attached build $build_number ($build_id) to App Store version $version"

submission_body="$(jq -cn \
  --arg version_id "$version_id" \
  '{
    data: {
      type: "appStoreVersionSubmissions",
      relationships: {
        appStoreVersion: {
          data: { type: "appStoreVersions", id: $version_id }
        }
      }
    }
  }')"
asc_request POST "/v1/appStoreVersionSubmissions" "$submission_body"
echo "Submitted DuckDisk $version (build $build_number) to App Review"
