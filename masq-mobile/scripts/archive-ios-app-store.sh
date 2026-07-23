#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKSPACE="$ROOT_DIR/ios/MasqMobile.xcworkspace"
SCHEME="MasqMobile"
ARCHIVE_PATH="${MASQ_ARCHIVE_PATH:-$ROOT_DIR/native/build/app-store/MasqMobile.xcarchive}"
BUNDLE_IDENTIFIER="${MASQ_BUNDLE_IDENTIFIER:-com.endthecb2.masqmobile}"
MARKETING_VERSION="${MASQ_MARKETING_VERSION:-1.0.0}"
BUILD_NUMBER="${MASQ_BUILD_NUMBER:-1}"
TEAM_ID="${APPLE_DEVELOPMENT_TEAM:-}"
NODE_FINDER_URL="${MASQ_NODE_FINDER_URL:-}"

fail() {
  echo "error: $1" >&2
  exit 1
}

valid_ipv4_host() {
  local candidate="$1"
  local octets
  local octet

  IFS='.' read -r -a octets <<< "$candidate"
  [ "${#octets[@]}" -eq 4 ] || return 1
  for octet in "${octets[@]}"; do
    [[ "$octet" =~ ^[0-9]{1,3}$ ]] || return 1
    [ "$((10#$octet))" -le 255 ] || return 1
  done
}

count_ipv6_groups() {
  local sequence="$1"
  local groups
  local group

  IPV6_GROUP_COUNT=0
  [ -n "$sequence" ] || return 0
  IFS=':' read -r -a groups <<< "$sequence"
  for group in "${groups[@]}"; do
    [[ "$group" =~ ^[0-9A-Fa-f]{1,4}$ ]] || return 1
    IPV6_GROUP_COUNT=$((IPV6_GROUP_COUNT + 1))
  done
}

valid_ipv6_host() {
  local candidate="$1"
  local left
  local right
  local left_count
  local right_count
  local ipv4_tail

  [[ "$candidate" == *:* && "$candidate" != *:::* ]] || return 1
  if [[ "$candidate" == *.* ]]; then
    ipv4_tail="${candidate##*:}"
    valid_ipv4_host "$ipv4_tail" || return 1
    candidate="${candidate%:*}:0:0"
  fi
  [[ "$candidate" =~ ^[0-9A-Fa-f:]+$ ]] || return 1

  if [[ "$candidate" == *::* ]]; then
    right="${candidate#*::}"
    [[ "$right" != *::* ]] || return 1
    left="${candidate%%::*}"
    count_ipv6_groups "$left" || return 1
    left_count="$IPV6_GROUP_COUNT"
    count_ipv6_groups "$right" || return 1
    right_count="$IPV6_GROUP_COUNT"
    [ "$((left_count + right_count))" -lt 8 ]
    return
  fi

  [[ "$candidate" != :* && "$candidate" != *: ]] || return 1
  count_ipv6_groups "$candidate" || return 1
  [ "$IPV6_GROUP_COUNT" -eq 8 ]
}

valid_dns_or_ipv4_host() {
  local host="$1"
  local candidate="$host"
  local label

  [ "${#candidate}" -le 253 ] || return 1
  if [[ "$candidate" == *.* ]] && [[ "$candidate" =~ ^[0-9.]+$ ]]; then
    valid_ipv4_host "$candidate"
    return
  fi

  # A single final dot is valid DNS notation; empty labels anywhere else are not.
  [[ "$candidate" != *.. ]] || return 1
  candidate="${candidate%.}"
  [ -n "$candidate" ] || return 1
  while [ -n "$candidate" ]; do
    if [[ "$candidate" == *.* ]]; then
      label="${candidate%%.*}"
      candidate="${candidate#*.}"
    else
      label="$candidate"
      candidate=""
    fi
    [ "${#label}" -le 63 ] || return 1
    [[ "$label" =~ ^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?$ ]] || return 1
  done
}

validate_node_finder_url() {
  local value="$1"
  local remainder
  local authority
  local host
  local port=""
  local port_number

  [ -n "$value" ] || return 1
  [[ "$value" =~ ^[Hh][Tt][Tt][Pp][Ss]:// ]] || return 1
  [[ "$value" != *[[:space:]]* && "$value" != *\?* && "$value" != *\#* ]] || return 1
  [[ "$value" != *\\* && "$value" != *\"* && "$value" != *\'* ]] || return 1

  remainder="${value#*://}"
  authority="${remainder%%/*}"
  [ -n "$authority" ] || return 1
  [[ "$authority" != *@* ]] || return 1

  if [[ "$authority" == \[* ]]; then
    [[ "$authority" =~ ^\[([0-9A-Fa-f:.]+)\](:([0-9]+))?$ ]] || return 1
    host="${BASH_REMATCH[1]}"
    port="${BASH_REMATCH[3]:-}"
    valid_ipv6_host "$host" || return 1
  else
    [[ "$authority" != *:*:* ]] || return 1
    if [[ "$authority" == *:* ]]; then
      host="${authority%:*}"
      port="${authority##*:}"
      [ -n "$port" ] || return 1
    else
      host="$authority"
    fi
    valid_dns_or_ipv4_host "$host" || return 1
  fi

  if [ -n "$port" ]; then
    [[ "$port" =~ ^[0-9]{1,5}$ ]] || return 1
    port_number=$((10#$port))
    [ "$port_number" -ge 1 ] && [ "$port_number" -le 65535 ] || return 1
  fi
}

main() {
[ -d "$WORKSPACE" ] || fail "Run bundle exec pod install in ios/ before archiving."
[ -n "$TEAM_ID" ] || fail "Set APPLE_DEVELOPMENT_TEAM to the private 10-character Apple Team ID."
[[ "$TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || fail "APPLE_DEVELOPMENT_TEAM is not a valid Team ID."
[[ "$BUNDLE_IDENTIFIER" =~ ^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$ ]] || \
  fail "MASQ_BUNDLE_IDENTIFIER is invalid."
[[ "$MARKETING_VERSION" =~ ^[0-9]+(\.[0-9]+){1,2}$ ]] || \
  fail "MASQ_MARKETING_VERSION must contain two or three numeric components."
[[ "$BUILD_NUMBER" =~ ^[1-9][0-9]*$ ]] || fail "MASQ_BUILD_NUMBER must be a positive integer."
validate_node_finder_url "$NODE_FINDER_URL" || \
  fail "Set MASQ_NODE_FINDER_URL to the reviewed production HTTPS endpoint."

if [[ "$NODE_FINDER_URL" =~ (^|[./-])(dev|dev[0-9]+|staging|test)([./-]|$) ]] && \
    [ "${ALLOW_DEVELOPMENT_NODE_FINDER:-NO}" != "YES" ]; then
  fail "The node-finder looks like a development service. Set ALLOW_DEVELOPMENT_NODE_FINDER=YES only after the operator confirms it is approved for production."
fi

"$ROOT_DIR/scripts/verify-ios-webview-patch.sh"
"$ROOT_DIR/scripts/verify-source-privacy.sh"

mkdir -p "$(dirname "$ARCHIVE_PATH")"

xcodebuild \
  -workspace "$WORKSPACE" \
  -scheme "$SCHEME" \
  -configuration Release \
  -destination 'generic/platform=iOS' \
  -archivePath "$ARCHIVE_PATH" \
  -allowProvisioningUpdates \
  DEVELOPMENT_TEAM="$TEAM_ID" \
  CODE_SIGN_STYLE=Automatic \
  PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE_IDENTIFIER" \
  MARKETING_VERSION="$MARKETING_VERSION" \
  CURRENT_PROJECT_VERSION="$BUILD_NUMBER" \
  MASQ_NODE_FINDER_URL="$NODE_FINDER_URL" \
  archive

APP_PATH="$ARCHIVE_PATH/Products/Applications/MasqMobile.app"
INFO_PLIST="$APP_PATH/Info.plist"
[ -f "$INFO_PLIST" ] || fail "The archive does not contain MasqMobile.app."

ARCHIVED_BUNDLE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$INFO_PLIST")"
ARCHIVED_NODE_FINDER="$(/usr/libexec/PlistBuddy -c 'Print :MASQNodeFinderURL' "$INFO_PLIST")"
[ "$ARCHIVED_BUNDLE_ID" = "$BUNDLE_IDENTIFIER" ] || fail "The archived bundle ID is incorrect."
[ "$ARCHIVED_NODE_FINDER" = "$NODE_FINDER_URL" ] || fail "The production node-finder was not embedded."

if rg -a -l '/Users/[^/[:space:]]+|Apple Development:|\$\(MASQ_NODE_FINDER_URL\)' "$APP_PATH" >/dev/null; then
  fail "The archive privacy scan found a local path, development identity or unresolved setting."
fi
if rg -a -l '__masqPrivateYouTubeFilter' "$APP_PATH" >/dev/null; then
  fail "The private YouTube filter was found in the public App Store archive."
fi

echo "App Store archive created and privacy-scanned: $ARCHIVE_PATH"
echo "Validate it in Xcode Organizer, generate the privacy report, then upload it to App Store Connect."
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
