#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEBVIEW_SOURCE="$ROOT_DIR/node_modules/react-native-webview/android/src/main/java/com/reactnativecommunity/webview/RNCWebViewManagerImpl.kt"
WEBVIEW_CLIENT_SOURCE="$ROOT_DIR/node_modules/react-native-webview/android/src/main/java/com/reactnativecommunity/webview/RNCWebViewClient.java"
WEBVIEW_EVENT_SOURCE="$ROOT_DIR/node_modules/react-native-webview/android/src/main/java/com/reactnativecommunity/webview/events/TopShouldStartLoadWithRequestEvent.kt"
PATCH_FILE="$ROOT_DIR/patches/react-native-webview+14.0.1.patch"

fail() {
  echo "error: $1" >&2
  exit 1
}

[ -f "$WEBVIEW_SOURCE" ] || \
  fail "react-native-webview is not installed. Run npm install before building Android."
[ -f "$WEBVIEW_CLIENT_SOURCE" ] || \
  fail "react-native-webview navigation source is missing."
[ -f "$WEBVIEW_EVENT_SOURCE" ] || \
  fail "react-native-webview navigation event source is missing."
[ -f "$PATCH_FILE" ] || \
  fail "The required Android WebView profile patch is missing."

for required_call in \
  'WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE)' \
  'WebViewCompat.setProfile(webView, profileName)' \
  'browser-profile.active' \
  'settings.savePassword = false' \
  'settings.saveFormData = false' \
  '^masq_(masq|direct)_[a-f0-9]{64}$'; do
  /usr/bin/grep -Fq "$required_call" "$WEBVIEW_SOURCE" || \
    fail "react-native-webview is unpatched. Run npm install without --ignore-scripts."
  /usr/bin/grep -Fq "$required_call" "$PATCH_FILE" || \
    fail "The persisted Android WebView profile patch is incomplete."
done

for required_call in \
  'shouldOverrideUrlLoadingWithMetadata' \
  'request.hasGesture()' \
  'request.isRedirect()' \
  'event.putBoolean("isRedirect", isRedirect)'; do
  /usr/bin/grep -Fq "$required_call" "$WEBVIEW_CLIENT_SOURCE" || \
    fail "react-native-webview is missing fail-closed Android navigation metadata."
  /usr/bin/grep -Fq "$required_call" "$PATCH_FILE" || \
    fail "The persisted Android navigation metadata patch is incomplete."
done

for required_call in \
  'shouldInterceptRequest(WebView view, WebResourceRequest request)' \
  'request.isForMainFrame()' \
  '!"GET".equalsIgnoreCase(request.getMethod())' \
  'Collections.singletonMap("Cache-Control", "no-store")' \
  'new ByteArrayInputStream(blockedBody)'; do
  /usr/bin/grep -Fq "$required_call" "$WEBVIEW_CLIENT_SOURCE" || \
    fail "react-native-webview is missing the fail-closed Android form-navigation guard."
done

/usr/bin/grep -Fq 'if (!mData.hasKey("navigationType"))' "$WEBVIEW_EVENT_SOURCE" || \
  fail "react-native-webview overwrites Android navigation metadata."
/usr/bin/grep -Fq 'if (!mData.hasKey("navigationType"))' "$PATCH_FILE" || \
  fail "The persisted Android navigation event patch is incomplete."

echo "Android isolated WebView profile patch verified."
