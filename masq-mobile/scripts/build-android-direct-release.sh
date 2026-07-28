#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ANDROID_DIR="$ROOT_DIR/android"
APP_GRADLE="$ANDROID_DIR/app/build.gradle"
DIST_DIR="$ROOT_DIR/distribution/android"
SDK_ROOT="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}"
BUILD_TOOLS_VERSION="${MASQ_ANDROID_BUILD_TOOLS_VERSION:-36.0.0}"
APPROVED_CERT_SHA256="346611622A6BCC187C0D31F54B2EF74903F830086FB17770F65016929DFE9F41"

: "${MASQ_NODE_FINDER_URL:?Set MASQ_NODE_FINDER_URL to the reviewed HTTPS node-finder endpoint.}"
: "${MASQ_ANDROID_KEYSTORE:?Set MASQ_ANDROID_KEYSTORE to the private release-keystore path.}"
: "${MASQ_ANDROID_KEYSTORE_PASSWORD:?Set MASQ_ANDROID_KEYSTORE_PASSWORD without writing it to source control.}"
: "${MASQ_ANDROID_EXPECTED_CERT_SHA256:?Set MASQ_ANDROID_EXPECTED_CERT_SHA256 to the approved signing-certificate digest.}"

MASQ_ANDROID_KEY_ALIAS="${MASQ_ANDROID_KEY_ALIAS:-masq-mobile-preview2}"
KEYSTORE_PASSWORD_VALUE="$MASQ_ANDROID_KEYSTORE_PASSWORD"
KEY_PASSWORD_VALUE="${MASQ_ANDROID_KEY_PASSWORD:-$KEYSTORE_PASSWORD_VALUE}"
unset MASQ_ANDROID_KEY_PASSWORD MASQ_ANDROID_KEYSTORE_PASSWORD

EXPECTED_CERT_SHA256="$(
  printf '%s' "$MASQ_ANDROID_EXPECTED_CERT_SHA256" |
    tr '[:lower:]' '[:upper:]' |
    tr -d ':[:space:]'
)"
if [ "$EXPECTED_CERT_SHA256" != "$APPROVED_CERT_SHA256" ]; then
  echo "error: expected certificate does not match the certificate pinned for official updates." >&2
  exit 1
fi

case "$(printf '%s' "$MASQ_NODE_FINDER_URL" | tr '[:upper:]' '[:lower:]')" in
  https://dev* | *://localhost* | *.invalid* | *://test* | *://staging*)
    if [ "${ALLOW_DEVELOPMENT_NODE_FINDER:-NO}" != "YES" ]; then
      echo "error: development node-finder requires ALLOW_DEVELOPMENT_NODE_FINDER=YES." >&2
      exit 1
    fi
    ;;
esac

if [ ! -f "$MASQ_ANDROID_KEYSTORE" ]; then
  echo "error: release keystore does not exist: $MASQ_ANDROID_KEYSTORE" >&2
  exit 1
fi
if [ -z "$SDK_ROOT" ] || [ ! -d "$SDK_ROOT" ]; then
  echo "error: set ANDROID_HOME or ANDROID_SDK_ROOT to an installed Android SDK." >&2
  exit 1
fi

BUILD_TOOLS_DIR="$SDK_ROOT/build-tools/$BUILD_TOOLS_VERSION"
APKSIGNER="$BUILD_TOOLS_DIR/apksigner"
ZIPALIGN="$BUILD_TOOLS_DIR/zipalign"
for tool in "$APKSIGNER" "$ZIPALIGN"; do
  if [ ! -x "$tool" ]; then
    echo "error: missing Android Build Tools $BUILD_TOOLS_VERSION executable: $tool" >&2
    exit 1
  fi
done

for tool in java node npm rustup cargo-ndk rg unzip strings; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: required build tool is missing from PATH: $tool" >&2
    exit 1
  fi
done

VERSION_NAME="$(
  sed -nE 's/^[[:space:]]*versionName[[:space:]]+"([^"]+)".*/\1/p' "$APP_GRADLE" |
    head -1
)"
VERSION_CODE="$(
  sed -nE 's/^[[:space:]]*versionCode[[:space:]]+([0-9]+).*/\1/p' "$APP_GRADLE" |
    head -1
)"
if [ -z "$VERSION_NAME" ] || [ -z "$VERSION_CODE" ]; then
  echo "error: could not read Android versionName/versionCode from $APP_GRADLE" >&2
  exit 1
fi

APK_BASENAME="MASQ-Mobile-Android-v${VERSION_NAME}"
UNSIGNED_APK="$ANDROID_DIR/app/build/outputs/apk/release/app-release-unsigned.apk"
ALIGNED_APK="$DIST_DIR/.${APK_BASENAME}-aligned.apk"
SIGNED_TEMP_APK="$DIST_DIR/.${APK_BASENAME}-signed.apk"
SIGNED_TEMP_IDSIG="${SIGNED_TEMP_APK}.idsig"
SIGNED_APK="$DIST_DIR/${APK_BASENAME}.apk"

mkdir -p "$DIST_DIR"
trap 'rm -f "$ALIGNED_APK" "$SIGNED_TEMP_APK" "$SIGNED_TEMP_IDSIG"' EXIT
rm -f \
  "$SIGNED_APK" \
  "$SIGNED_TEMP_IDSIG" \
  "$DIST_DIR/SHA256SUMS.txt" \
  "$DIST_DIR/SIGNING-CERTIFICATE.txt"

"$ROOT_DIR/scripts/verify-source-privacy.sh"

(
  cd "$ANDROID_DIR"
  ./gradlew clean assembleRelease --no-daemon
)

if [ ! -f "$UNSIGNED_APK" ]; then
  echo "error: Gradle did not create the expected release APK." >&2
  exit 1
fi

if ! unzip -p "$UNSIGNED_APK" 'classes*.dex' |
    strings -a |
    rg -F "$MASQ_NODE_FINDER_URL" >/dev/null; then
  echo "error: the reviewed node-finder URL was not embedded in the APK." >&2
  exit 1
fi
if unzip -p "$UNSIGNED_APK" 'classes*.dex' |
    strings -a |
    rg 'ci\.invalid\.example' >/dev/null; then
  echo "error: APK contains the CI-only node-finder endpoint." >&2
  exit 1
fi

"$ZIPALIGN" -f -P 16 4 "$UNSIGNED_APK" "$ALIGNED_APK"
MASQ_ANDROID_KEYSTORE_PASSWORD="$KEYSTORE_PASSWORD_VALUE" \
MASQ_ANDROID_KEY_PASSWORD="$KEY_PASSWORD_VALUE" \
  "$APKSIGNER" sign \
  --ks "$MASQ_ANDROID_KEYSTORE" \
  --ks-key-alias "$MASQ_ANDROID_KEY_ALIAS" \
  --ks-pass env:MASQ_ANDROID_KEYSTORE_PASSWORD \
  --key-pass env:MASQ_ANDROID_KEY_PASSWORD \
  --v4-signing-enabled false \
  --out "$SIGNED_TEMP_APK" \
  "$ALIGNED_APK"
unset KEYSTORE_PASSWORD_VALUE KEY_PASSWORD_VALUE

CERTIFICATE_REPORT="$DIST_DIR/.${APK_BASENAME}-certificate.txt"
trap 'rm -f "$ALIGNED_APK" "$SIGNED_TEMP_APK" "$SIGNED_TEMP_IDSIG" "$CERTIFICATE_REPORT"' EXIT
"$APKSIGNER" verify --print-certs --Werr "$SIGNED_TEMP_APK" >"$CERTIFICATE_REPORT"
ACTUAL_CERT_SHA256="$(
  awk -F ': ' '/Signer #1 certificate SHA-256 digest/ { print toupper($2) }' \
    "$CERTIFICATE_REPORT"
)"
if [ -z "$ACTUAL_CERT_SHA256" ] || [ "$ACTUAL_CERT_SHA256" != "$EXPECTED_CERT_SHA256" ]; then
  echo "error: APK signing certificate does not match the approved fingerprint." >&2
  exit 1
fi

ANDROID_HOME="$SDK_ROOT" \
  MASQ_ANDROID_EXPECTED_VERSION_NAME="$VERSION_NAME" \
  MASQ_ANDROID_EXPECTED_VERSION_CODE="$VERSION_CODE" \
  "$ROOT_DIR/scripts/verify-android-apk-privacy.sh" "$SIGNED_TEMP_APK"
mv -f "$SIGNED_TEMP_APK" "$SIGNED_APK"

(
  cd "$DIST_DIR"
  shasum -a 256 "$(basename "$SIGNED_APK")" >SHA256SUMS.txt
  printf 'Signer certificate SHA-256: %s\n' "$ACTUAL_CERT_SHA256" \
    >SIGNING-CERTIFICATE.txt
)

echo "Android direct-distribution release created:"
echo "  $SIGNED_APK"
echo "  $DIST_DIR/SHA256SUMS.txt"
echo "  $DIST_DIR/SIGNING-CERTIFICATE.txt"
