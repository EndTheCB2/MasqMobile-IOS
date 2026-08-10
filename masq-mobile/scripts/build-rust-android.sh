#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$ROOT_DIR/native/masq-mobile-core/Cargo.toml"
OUTPUT_DIR="$ROOT_DIR/android/app/src/main/jniLibs"
RUST_TOOLCHAIN="${MASQ_RUST_TOOLCHAIN:-1.77.2}"
CARGO_NDK_VERSION="${MASQ_CARGO_NDK_VERSION:-4.1.2}"
ANDROID_NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"

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
if [ -z "$ANDROID_NDK" ]; then
  echo "error: ANDROID_NDK_HOME or ANDROID_NDK_ROOT is not configured." >&2
  exit 1
fi

# Vendored OpenSSL and libsodium must be archived with the NDK tools. Apple's
# /usr/bin/ar accepts the Android object files without failing, but produces
# empty archives that leave their symbols unresolved in the final .so.
LLVM_AR="$(
  find -L "$ANDROID_NDK/toolchains/llvm/prebuilt" -type f -path '*/bin/llvm-ar' -perm -111 |
    sort |
    head -1
)"
LLVM_RANLIB="$(
  find -L "$ANDROID_NDK/toolchains/llvm/prebuilt" -type f -path '*/bin/llvm-ranlib' -perm -111 |
    sort |
    head -1
)"
LLVM_NM="$(
  find -L "$ANDROID_NDK/toolchains/llvm/prebuilt" -type f -path '*/bin/llvm-nm' -perm -111 |
    sort |
    head -1
)"
if [ -z "$LLVM_AR" ] || [ -z "$LLVM_RANLIB" ] || [ -z "$LLVM_NM" ]; then
  echo "error: the configured Android NDK does not contain llvm-ar, llvm-ranlib, and llvm-nm." >&2
  exit 1
fi
export AR="$LLVM_AR"
export RANLIB="$LLVM_RANLIB"
export NM="$LLVM_NM"
export AR_aarch64_linux_android="$LLVM_AR"
export AR_x86_64_linux_android="$LLVM_AR"
export RANLIB_aarch64_linux_android="$LLVM_RANLIB"
export RANLIB_x86_64_linux_android="$LLVM_RANLIB"
export NM_aarch64_linux_android="$LLVM_NM"
export NM_x86_64_linux_android="$LLVM_NM"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_AR="$LLVM_AR"
export CARGO_TARGET_X86_64_LINUX_ANDROID_AR="$LLVM_AR"

# Keep Android's Cargo cache away from checkout paths containing spaces. Older
# native dependency builds can otherwise reuse empty host-generated archives.
# Some vendored C dependencies also compile their build directory into the
# shared library, so a target below a user home would leak builder identity even
# when rustc path remapping is enabled.
export CARGO_TARGET_DIR="${MASQ_ANDROID_CARGO_TARGET_DIR:-${TMPDIR:-/tmp}/masq-mobile-android-cargo-target-v2}"
mkdir -p "$CARGO_TARGET_DIR"
CARGO_TARGET_DIR="$(cd "$CARGO_TARGET_DIR" && pwd -P)"
case "$CARGO_TARGET_DIR/" in
  "$HOME/"* | "$ROOT_DIR/"* | /Users/* | /home/*)
    echo "error: MASQ_ANDROID_CARGO_TARGET_DIR must use a privacy-neutral temporary path outside a user home or checkout." >&2
    exit 1
    ;;
esac
export CARGO_TARGET_DIR

# Rust panic locations otherwise retain the builder's absolute home and checkout
# paths. Encoded flags preserve paths containing spaces as one rustc argument.
append_encoded_rustflag "--remap-path-prefix=$HOME=/build/source"
append_encoded_rustflag "--remap-path-prefix=$ROOT_DIR=/usr/src/masq-mobile"
append_encoded_rustflag "--remap-path-prefix=$CARGO_TARGET_DIR=/build/android-target"
# Android must reject a shared library with unresolved non-system symbols at
# link time instead of shipping an APK that only fails on the device.
append_encoded_rustflag "-Clink-arg=-Wl,-z,defs"

mkdir -p "$OUTPUT_DIR"
rustup target add --toolchain "$RUST_TOOLCHAIN" aarch64-linux-android x86_64-linux-android >/dev/null
export RUSTC="$(rustup which --toolchain "$RUST_TOOLCHAIN" rustc)"
cd "$(dirname "$MANIFEST")"
rustup run "$RUST_TOOLCHAIN" cargo ndk \
  --target arm64-v8a \
  --target x86_64 \
  --platform 24 \
  --link-builtins \
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
  --link-builtins \
  --output-dir "$OUTPUT_DIR" \
  --manifest-path "$TUNNEL_MANIFEST" \
  build \
  --package masq-packet-tunnel \
  --release \
  --locked

# cargo-ndk copies these dependency cdylibs even though the packet tunnel links
# their Rust code statically and does not declare them in DT_NEEDED. Remove only
# the two audited copies; any new or required shared dependency fails below.
find "$OUTPUT_DIR" -type f \
  \( -name 'libsysinfo.so' -o -name 'libsysinfo-*.so' -o \
     -name 'libtun2proxy.so' -o -name 'libtun2proxy-*.so' \) \
  -delete

node "$ROOT_DIR/scripts/verify-android-native-elf.js" --jni-dir "$OUTPUT_DIR"
