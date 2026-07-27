# MASQ Mobile Android 1.0.0-preview.3

This is an experimental, independently maintained, consume-only MASQ Mobile preview distributed
outside Google Play. It is not an official MASQ Project release and has not completed an
independent mobile security audit.

## Download

Install `MASQ-Mobile-Android-v1.0.0-preview.3.apk` only from this GitHub Release. The release also
contains:

- `SHA256SUMS.txt` for file-integrity verification; and
- `SIGNING-CERTIFICATE.txt` for the stable signing-certificate fingerprint used from preview.2
  onward.

Follow the
[Android installation guide](https://github.com/EndTheCB2/MasqMobile-IOS/blob/android-v1.0.0-preview.3/ANDROID_DIRECT_INSTALL.md)
before installing or updating.

## Updating

Preview.2 users can install preview.3 directly over the existing app. Do not uninstall or reset
the app: preview.3 retains the same package ID and signing certificate, so Android preserves local
wallet and profile data.

Preview.1 used a different signing certificate. Preview.1 users must:

1. write down and verify their 12-word wallet recovery phrase offline;
2. uninstall preview.1; and
3. install preview.3 from this Release.

Uninstalling without that backup removes the locally encrypted wallet and profile. New users are
not affected. Future releases must retain preview.2 certificate SHA-256
`346611622A6BCC187C0D31F54B2EF74903F830086FB17770F65016929DFE9F41`.

## Fixes since preview.2

- Fixed the native MASQ core failing to load on Android. Vendored OpenSSL, libsodium, and compiler
  runtime code are now linked into the core with Android NDK tools for both arm64-v8a phones and
  x86_64 emulators.
- Added a strict native linker gate that rejects unresolved symbols during the Rust build.
- Added an independent ELF audit for both native libraries and both architectures. The same audit
  now runs against the final APK and in GitHub Actions, preventing an interface-only build from
  being released again.

## Included

- Consume-only MASQ routing; the phone does not serve traffic for other peers.
- A fail-closed MASQ Private browser and a separate, explicitly confirmed direct-browser mode.
- Automatic entry-node discovery and refresh.
- Wallet import from 12 recovery words or a private key, stored in operating-system secure storage.
- Configurable hop count and preferred exit country.
- Android whole-device or selected-app routing through `VpnService`.
- Balanced/Strict browser protection with exact-host exceptions and Reject-only handling for
  supported consent managers.
- ENS `.eth` websites through the eth.limo HTTPS gateway without search or Direct fallback.
- Temporary WebView sessions by default. Remembered exact-host sign-ins are offered only when the
  installed Android WebView supports isolated multi-profile storage.

## Service disclosure

- Base Mainnet RPC: `https://base-pokt.nodies.app`
- Base Sepolia RPC: `https://base-sepolia-rpc.publicnode.com`
- Entry-node discovery: `https://dev2.api.masq.ai`
- Search: Timpi Search
- ENS gateway: eth.limo

The node-finder is a development service and may be unavailable or change without notice. This is
not a Google Play production release.

## Security boundaries

- MASQ Private blocks the page if the route fails; it does not silently fall back to the direct
  device connection.
- Direct browsing exposes the public IP of the current connection or VPN.
- Browser protection is best effort and does not claim YouTube ad blocking.
- GitHub-installed builds do not update automatically.
- Back up the wallet recovery phrase before uninstalling. Never post it in a GitHub issue.

The source corresponding to this APK is available from this release tag under GPL-3.0-only.
