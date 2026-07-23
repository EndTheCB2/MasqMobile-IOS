# MASQ Mobile Android 1.0.0-preview.1

This is an experimental, independently maintained, consume-only MASQ Mobile preview distributed
outside Google Play. It is not an official MASQ Project release and has not completed an
independent mobile security audit.

## Download

Install `MASQ-Mobile-Android-v1.0.0-preview.1.apk` only from this GitHub Release. The release also
contains:

- `SHA256SUMS.txt` for file-integrity verification; and
- `SIGNING-CERTIFICATE.txt` for the stable Android signing-certificate fingerprint.

Follow the
[Android installation guide](https://github.com/EndTheCB2/MasqMobile-IOS/blob/android-v1.0.0-preview.1/ANDROID_DIRECT_INSTALL.md)
before installing or updating.

## Included

- Consume-only MASQ routing; the phone does not serve traffic for other peers.
- A fail-closed MASQ Private browser and a separate, explicitly confirmed direct-browser mode.
- Automatic entry-node discovery and refresh.
- Wallet import from 12 recovery words or a private key, stored in operating-system secure storage.
- Configurable hop count and preferred exit country.
- Android whole-device or selected-app routing through `VpnService`.
- Local browser protection for recognized ads, trackers, cross-site cookies and cookie banners.

## Service disclosure

- Base Mainnet RPC: `https://base-pokt.nodies.app`
- Base Sepolia RPC: `https://base-sepolia-rpc.publicnode.com`
- Entry-node discovery: `https://dev2.api.masq.ai`
- Search: Timpi Search

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
