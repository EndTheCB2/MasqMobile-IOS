#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$ROOT_DIR/native/masq-mobile-core/Cargo.toml"
OUTPUT_DIR="$ROOT_DIR/android/app/src/main/jniLibs"
RUST_TOOLCHAIN="${MASQ_RUST_TOOLCHAIN:-1.77.2}"
CARGO_NDK_VERSION="${MASQ_CARGO_NDK_VERSION:-4.1.2}"

append_encoded_rustflag() {
  local flag="$1"
  if [ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]; then
    CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS}"$'\x1f'"${flag}"
  else
    CARGO_ENCODED_RUSTFLAGS="$flag"
  fi
  export CARGO_ENCODED_RUSTFLAGS
}

if ! command -v cargo-ndk >/dev/null 2>&1; then
  echo "error: cargo-ndk is missing. Install it once with: rustup run 1.97.1 cargo install cargo-ndk --version $CARGO_NDK_VERSION --locked" >&2
  exit 1
fi
if [ -z "${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}" ]; then
  echo "error: ANDROID_NDK_HOME or ANDROID_NDK_ROOT is not configured." >&2
  exit 1
fi

# Rust panic locations otherwise retain the builder's absolute home and checkout
# paths. Encoded flags preserve paths containing spaces as one rustc argument.
append_encoded_rustflag "--remap-path-prefix=$HOME=/build/source"
append_encoded_rustflag "--remap-path-prefix=$ROOT_DIR=/usr/src/masq-mobile"

mkdir -p "$OUTPUT_DIR"
rustup target add --toolchain "$RUST_TOOLCHAIN" aarch64-linux-android x86_64-linux-android >/dev/null
export RUSTC="$(rustup which --toolchain "$RUST_TOOLCHAIN" rustc)"
cd "$(dirname "$MANIFEST")"
rustup run "$RUST_TOOLCHAIN" cargo ndk \
  --target arm64-v8a \
  --target x86_64 \
  --platform 24 \
  --output-dir "$OUTPUT_DIR" \
  --manifest-path "$MANIFEST" \
  build \
  --package masq-mobile-core \
  --features node-engine \
  --release \
  --locked

TUNNEL_TOOLCHAIN="${MASQ_TUNNEL_RUST_TOOLCHAIN:-1.97.1}"
TUNNEL_MANIFEST="$ROOT_DIR/native/masq-packet-tunnel/Cargo.toml"
rustup target add --toolchain "$TUNNEL_TOOLCHAIN" aarch64-linux-android x86_64-linux-android >/dev/null
export RUSTC="$(rustup which --toolchain "$TUNNEL_TOOLCHAIN" rustc)"
cd "$(dirname "$TUNNEL_MANIFEST")"
rustup run "$TUNNEL_TOOLCHAIN" cargo ndk \
  --target arm64-v8a \
  --target x86_64 \
  --platform 24 \
  --output-dir "$OUTPUT_DIR" \
  --manifest-path "$TUNNEL_MANIFEST" \
  build \
  --package masq-packet-tunnel \
  --release \
  --locked

# cargo-ndk also copies cdylib dependencies into the JNI directory. The two
# exported MASQ libraries link only Android system libraries, so those copies
# are unnecessary and would enlarge the APK considerably.
find "$OUTPUT_DIR" -type f -name '*.so' \
  ! -name 'libmasq_mobile_core.so' \
  ! -name 'libmasq_packet_tunnel.so' \
  -delete
