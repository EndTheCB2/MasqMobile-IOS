# Install the MASQ Mobile Android preview

This is an experimental, consume-only MASQ Mobile build distributed directly through GitHub. It is
not an official MASQ release and has not been reviewed by Google Play. Download it only from the
official release page of this repository.

This preview uses the public Nodies Base Mainnet RPC, the public PublicNode Base Sepolia RPC, and
the experimental MASQ node-finder at `https://dev2.api.masq.ai`. The node-finder is a development
service and may be unavailable or change without notice.

## Requirements

- A 64-bit Android phone running Android 7.0 or later.
- A current Android System WebView.
- Enough MASQ and Base ETH for the selected network profile when using MASQ Private.

The Android package ID is `com.endthecb2.masqmobile`.

## Install

### One-time migration from preview.1

`1.0.0-preview.2` establishes a new permanent Android signing certificate because the password for
the preview.1 signing key was no longer available. Android therefore cannot install preview.2 over
preview.1.

Before removing preview.1, write down and verify the wallet's 12-word recovery phrase offline.
Then uninstall preview.1 and install preview.2 using the steps below. Uninstalling without that
backup permanently removes the locally encrypted wallet and profile. New installations are not
affected. Releases after preview.2 must keep the preview.2 certificate.

1. On the Android phone, open the official GitHub Release page.
2. Download the file named `MASQ-Mobile-Android-v*.apk`. Do not install an APK forwarded through a
   chat message or hosted elsewhere.
3. Open the download from the browser notification or the Downloads/Files app.
4. If Android says that this source is not allowed, open **Settings**, enable **Allow from this
   source** only for the browser or Files app that opened the APK, go back, and tap **Install**.
   Menu wording can differ by phone manufacturer.
5. Let Google Play Protect scan the APK if offered. Do not disable Play Protect. If it reports the
   APK as harmful, stop and report the exact warning on this repository instead of bypassing it.
6. After installation, turn **Allow from this source** off again for the browser or Files app.

Android asks for VPN permission only if whole-device or selected-app routing is enabled. That
system dialog is expected; MASQ Mobile cannot grant the permission itself.

## Check the download

The Release contains `SHA256SUMS.txt` and `SIGNING-CERTIFICATE.txt`. A technical user can compare
the APK's SHA-256 checksum with `SHA256SUMS.txt`. Preview.2 uses certificate SHA-256
`346611622A6BCC187C0D31F54B2EF74903F830086FB17770F65016929DFE9F41`. Every later update must have
this same signing-certificate fingerprint and a higher Android version code.

Never install a file when its checksum differs from the value published in the same GitHub
Release.

## Update or remove

GitHub installations do not update automatically. Starting with preview.2, download a newer APK
from the same repository and install it over the existing app. Android preserves local app data
only when the application ID and signing certificate match.

Back up the wallet recovery phrase before uninstalling. Uninstalling MASQ Mobile removes its local
profile and encrypted wallet data. The recovery phrase is never recoverable from the signing key
or from GitHub.

## Preview boundaries

- The app consumes MASQ routes; it does not serve traffic for other peers.
- **MASQ Private** fails closed. It does not silently fall back to the device connection.
- **Browse without MASQ** is an explicit direct mode and exposes the connection's normal public IP.
- Android system routing supports whole-device or selected-app scope. The MASQ management process
  itself is excluded to avoid a VPN loop.
- Browser protection is best effort. This public build does not claim YouTube ad blocking.
- This preview uses the development node-finder endpoint recorded above and in its Release notes.
  It is not a Google Play production release.

Starting with regional enforcement on 30 September 2026 and a wider rollout in 2027, Android
Developer Verification may add extra installation requirements for apps distributed outside
Google Play. See the official
[Android developer verification guide](https://developer.android.com/developer-verification/guides)
for the current country and device rules.

Official Android guidance:

- [Alternative distribution](https://developer.android.com/distribute/marketing-tools/alternative-distribution)
- [Google Play Protect](https://support.google.com/googleplay/answer/2812853)
- [APK signing](https://developer.android.com/studio/publish/app-signing)
