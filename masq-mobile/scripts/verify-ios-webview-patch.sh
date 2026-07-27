#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEBVIEW_SOURCE="$ROOT_DIR/node_modules/react-native-webview/apple/RNCWebViewImpl.m"
PATCH_FILE="$ROOT_DIR/patches/react-native-webview+14.0.1.patch"

fail() {
  echo "error: $1" >&2
  exit 1
}

[ -f "$WEBVIEW_SOURCE" ] || \
  fail "react-native-webview is not installed. Run npm install before building iOS."
[ -f "$PATCH_FILE" ] || \
  fail "The required react-native-webview fail-closed patch is missing."

MASQ_DATA_STORE_CALL="masq_private_browser_data_store()"
DIRECT_DATA_STORE_CALL="masq_direct_browser_data_store()"
MASQ_PERSISTENT_DATA_STORE_CALL="masq_persistent_browser_data_store()"
DIRECT_PERSISTENT_DATA_STORE_CALL="masq_direct_persistent_browser_data_store()"
CONTENT_CONTROLLER_CALL="masq_configure_private_browser_content_controller("

for required_call in \
  "$MASQ_DATA_STORE_CALL" \
  "$DIRECT_DATA_STORE_CALL" \
  "$MASQ_PERSISTENT_DATA_STORE_CALL" \
  "$DIRECT_PERSISTENT_DATA_STORE_CALL" \
  "$CONTENT_CONTROLLER_CALL"; do
  /usr/bin/grep -Fq "$required_call" "$WEBVIEW_SOURCE" || \
    fail "react-native-webview is unpatched. Run npm install without --ignore-scripts."
  /usr/bin/grep -Fq "$required_call" "$PATCH_FILE" || \
    fail "The persisted react-native-webview patch is incomplete."
done

(cd "$ROOT_DIR" && /usr/bin/git apply --check --reverse "$PATCH_FILE") || \
  fail "The persisted react-native-webview patch is corrupt or does not match the installed source."

echo "iOS MASQ/direct fail-closed WebView patch verified."
