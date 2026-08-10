#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APK_PATH="${1:-}"
if [ -z "$APK_PATH" ] || [ ! -f "$APK_PATH" ]; then
  echo "usage: $0 /path/to/signed-release.apk" >&2
  exit 2
fi

if ! command -v rg >/dev/null 2>&1; then
  echo "error: APK privacy check requires ripgrep (rg)." >&2
  exit 1
fi

find_build_tool() {
  local tool="$1"
  local sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}"

  if command -v "$tool" >/dev/null 2>&1; then
    command -v "$tool"
    return
  fi

  if [ -n "$sdk_root" ] && [ -d "$sdk_root/build-tools" ]; then
    find "$sdk_root/build-tools" -type f -name "$tool" -perm -111 |
      sort -V |
      tail -1
  fi
}

find_sdk_tool() {
  local tool="$1"
  local sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}"

  if command -v "$tool" >/dev/null 2>&1; then
    command -v "$tool"
    return
  fi

  if [ -n "$sdk_root" ] && [ -x "$sdk_root/cmdline-tools/latest/bin/$tool" ]; then
    printf '%s\n' "$sdk_root/cmdline-tools/latest/bin/$tool"
  fi
}

APKSIGNER="$(find_build_tool apksigner)"
ZIPALIGN="$(find_build_tool zipalign)"
APKANALYZER="$(find_sdk_tool apkanalyzer)"
if [ -z "$APKSIGNER" ] || [ -z "$ZIPALIGN" ] || [ -z "$APKANALYZER" ]; then
  echo "error: apksigner, zipalign, and apkanalyzer are required from the Android SDK." >&2
  exit 1
fi

TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/masq-apk-privacy.XXXXXX")"
trap 'rm -rf "$TEMP_DIR"' EXIT
APK_STRINGS="$TEMP_DIR/apk-strings.txt"
unzip -p "$APK_PATH" | strings -a >"$APK_STRINGS"

scan_binary_strings() {
  local expression="$1"
  local description="$2"

  if LC_ALL=C rg --no-config "$expression" "$APK_STRINGS" >/dev/null; then
    echo "error: APK privacy check found $description." >&2
    return 1
  fi
}

scan_archive_names() {
  local expression="$1"
  local description="$2"

  if unzip -Z1 "$APK_PATH" | LC_ALL=C rg --no-config "$expression" >/dev/null; then
    echo "error: APK privacy check found $description." >&2
    return 1
  fi
}

scan_binary_strings '/Users/[A-Za-z0-9._-]+' 'a local macOS user path'
scan_binary_strings '/home/[A-Za-z0-9._-]+' 'a local Linux user path'
scan_binary_strings '[A-Za-z]:[/\\]Users[/\\][A-Za-z0-9._-]+' 'a local Windows user path'
scan_binary_strings 'BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY' 'private-key material'
scan_binary_strings '(github_pat_|ghp_)[A-Za-z0-9_]+' 'a GitHub access token'
scan_binary_strings 'DEVELOPMENT_TEAM[[:space:]]*=[[:space:]]*[A-Z0-9]{10}' 'an Apple team identifier'
scan_archive_names '(^|/)([^/]+\.(jks|keystore|p8|p12|pem|mobileprovision)|local\.properties|\.env(\..*)?|[^/]+\.log)$' \
  'a signing, environment, provisioning, or log file'

EXPECTED_APPLICATION_ID="${MASQ_ANDROID_EXPECTED_APPLICATION_ID:-com.endthecb2.masqmobile}"
ACTUAL_APPLICATION_ID="$("$APKANALYZER" manifest application-id "$APK_PATH")"
ACTUAL_VERSION_NAME="$("$APKANALYZER" manifest version-name "$APK_PATH")"
ACTUAL_VERSION_CODE="$("$APKANALYZER" manifest version-code "$APK_PATH")"
ACTUAL_MIN_SDK="$("$APKANALYZER" manifest min-sdk "$APK_PATH")"
ACTUAL_TARGET_SDK="$("$APKANALYZER" manifest target-sdk "$APK_PATH")"
ACTUAL_DEBUGGABLE="$("$APKANALYZER" manifest debuggable "$APK_PATH")"
MANIFEST_XML="$TEMP_DIR/AndroidManifest.xml"
"$APKANALYZER" manifest print "$APK_PATH" >"$MANIFEST_XML"

[ "$ACTUAL_APPLICATION_ID" = "$EXPECTED_APPLICATION_ID" ] || {
  echo "error: unexpected Android application ID." >&2
  exit 1
}
[ -n "$ACTUAL_VERSION_NAME" ] && [ "$ACTUAL_VERSION_CODE" -ge 1 ] || {
  echo "error: invalid Android release version." >&2
  exit 1
}
if [ -n "${MASQ_ANDROID_EXPECTED_VERSION_NAME:-}" ] &&
    [ "$ACTUAL_VERSION_NAME" != "$MASQ_ANDROID_EXPECTED_VERSION_NAME" ]; then
  echo "error: unexpected Android version name." >&2
  exit 1
fi
if [ -n "${MASQ_ANDROID_EXPECTED_VERSION_CODE:-}" ] &&
    [ "$ACTUAL_VERSION_CODE" != "$MASQ_ANDROID_EXPECTED_VERSION_CODE" ]; then
  echo "error: unexpected Android version code." >&2
  exit 1
fi
[ "$ACTUAL_MIN_SDK" = "24" ] && [ "$ACTUAL_TARGET_SDK" = "36" ] || {
  echo "error: unexpected Android SDK compatibility range." >&2
  exit 1
}
[ "$ACTUAL_DEBUGGABLE" = "false" ] || {
  echo "error: distributable APK must not be debuggable." >&2
  exit 1
}
if rg --no-config 'android:(debuggable|testOnly)="true"' "$MANIFEST_XML" >/dev/null; then
  echo "error: distributable APK contains a debug or test-only manifest flag." >&2
  exit 1
fi

for abi in arm64-v8a x86_64; do
  for library in libmasq_mobile_core.so libmasq_packet_tunnel.so; do
    if ! unzip -Z1 "$APK_PATH" | rg --no-config "^lib/$abi/$library$" >/dev/null; then
      echo "error: APK is missing a required MASQ native library." >&2
      exit 1
    fi
  done
done
if unzip -Z1 "$APK_PATH" | rg --no-config '^lib/(armeabi|armeabi-v7a|x86)/' >/dev/null; then
  echo "error: APK contains an unsupported 32-bit native architecture." >&2
  exit 1
fi
"$ROOT_DIR/scripts/verify-android-native-elf.js" --apk "$APK_PATH"

CERTIFICATE_REPORT="$TEMP_DIR/certificate.txt"
"$APKSIGNER" verify --verbose --print-certs --Werr "$APK_PATH" >"$CERTIFICATE_REPORT"
if rg --no-config -i 'CN=Android Debug|Android Debug' "$CERTIFICATE_REPORT" >/dev/null; then
  echo "error: APK uses an Android debug signing certificate." >&2
  exit 1
fi
"$ZIPALIGN" -c -P 16 4 "$APK_PATH" >/dev/null

echo "Signed APK privacy, signature, and alignment checks passed."
