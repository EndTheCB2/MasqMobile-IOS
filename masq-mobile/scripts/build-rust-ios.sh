#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$ROOT_DIR/native/masq-mobile-core/Cargo.toml"
TARGET_DIR="$ROOT_DIR/native/build/ios"
OUTPUT="${CONFIGURATION_BUILD_DIR:-$TARGET_DIR/output}/libmasq_mobile_core.a"
RUST_TOOLCHAIN="${MASQ_RUST_TOOLCHAIN:-1.77.2}"

if [ -f "$ROOT_DIR/ios/.xcode.env.local" ]; then
  # Match React Native's Xcode environment when this script is launched from Xcode.app.
  # shellcheck disable=SC1091
  source "$ROOT_DIR/ios/.xcode.env.local"
fi

RUSTUP_BIN="$(command -v rustup || true)"
if [ -z "$RUSTUP_BIN" ]; then
  echo "error: rustup is required to build the MASQ mobile core." >&2
  exit 1
fi

RUST_TOOLCHAIN_BIN="$(dirname "$("$RUSTUP_BIN" which --toolchain "$RUST_TOOLCHAIN" rustc)")"
export PATH="$RUST_TOOLCHAIN_BIN:$PATH"

# Keep local usernames and workspace paths out of release artifacts and panic metadata. Cargo's
# encoded form preserves paths containing spaces as a single rustc argument.
RUST_FLAG_SEPARATOR=$'\x1f'
rust_path_remaps=(
  "--remap-path-prefix=$HOME=/build/home"
  "--remap-path-prefix=$ROOT_DIR=/src/masq-mobile"
)
for rust_path_remap in "${rust_path_remaps[@]}"; do
  if [ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]; then
    CARGO_ENCODED_RUSTFLAGS+="$RUST_FLAG_SEPARATOR"
  fi
  CARGO_ENCODED_RUSTFLAGS+="$rust_path_remap"
done
export CARGO_ENCODED_RUSTFLAGS

# Xcode exports the iPhone SDK to every build phase. Cargo also compiles native
# macOS build scripts, so give those host artifacts an explicit macOS linker.
export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="$ROOT_DIR/scripts/xcode-host-cc.sh"
export CC_aarch64_apple_darwin="$ROOT_DIR/scripts/xcode-host-cc.sh"
export CXX_aarch64_apple_darwin="$ROOT_DIR/scripts/xcode-host-cxx.sh"

platform="${PLATFORM_NAME:-iphonesimulator}"
architectures="${ARCHS:-arm64}"
libraries=()

for architecture in $architectures; do
  case "$platform/$architecture" in
    iphoneos/arm64) rust_target="aarch64-apple-ios" ;;
    iphonesimulator/arm64) rust_target="aarch64-apple-ios-sim" ;;
    iphonesimulator/x86_64) rust_target="x86_64-apple-ios" ;;
    *)
      echo "error: Niet-ondersteund iOS Rust-target: $platform/$architecture" >&2
      exit 1
      ;;
  esac

  "$RUSTUP_BIN" target add --toolchain "$RUST_TOOLCHAIN" "$rust_target" >/dev/null
  CARGO_TARGET_DIR="$TARGET_DIR" \
    "$RUSTUP_BIN" run "$RUST_TOOLCHAIN" cargo rustc \
      --manifest-path "$MANIFEST" \
      --package masq-mobile-core \
      --lib \
      --crate-type staticlib \
      --features node-engine \
      --target "$rust_target" \
      --release \
      --locked
  libraries+=("$TARGET_DIR/$rust_target/release/libmasq_mobile_core.a")
done

mkdir -p "$(dirname "$OUTPUT")"
if [ "${#libraries[@]}" -eq 1 ]; then
  cp "${libraries[0]}" "$OUTPUT"
else
  xcrun lipo -create "${libraries[@]}" -output "$OUTPUT"
fi
