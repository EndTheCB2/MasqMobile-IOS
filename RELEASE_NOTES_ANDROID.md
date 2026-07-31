# MASQ Mobile Android 1.0.0-preview.6

This is an experimental, independently maintained, consume-only MASQ Mobile preview distributed
outside Google Play. It is not an official MASQ Project release and has not completed an
independent mobile security audit.

## Download

Install `MASQ-Mobile-Android-v1.0.0-preview.6.apk` only from this GitHub Release. The release also
contains:

- `SHA256SUMS.txt` for file-integrity verification; and
- `SIGNING-CERTIFICATE.txt` for the stable signing-certificate fingerprint used from preview.2
  onward.

Follow the
[Android installation guide](https://github.com/EndTheCB2/MasqMobile-IOS/blob/android-v1.0.0-preview.6/ANDROID_DIRECT_INSTALL.md)
before installing or updating.

## Updating

Preview.2 through preview.4 users can install preview.6 directly over the existing app. Do not
uninstall or reset the app: preview.6 retains the same package ID and signing certificate, so
Android preserves local wallet and profile data.

Preview.1 used a different signing certificate. Preview.1 users must:

1. write down and verify their 12-word wallet recovery phrase offline;
2. uninstall preview.1; and
3. install preview.6 from this Release.

Uninstalling without that backup removes the locally encrypted wallet and profile. New users are
not affected. Future releases must retain preview.2 certificate SHA-256
`346611622A6BCC187C0D31F54B2EF74903F830086FB17770F65016929DFE9F41`.

## Improvements since preview.4

- A user-requested MASQ consumer session now runs in an Android foreground service, allowing the
  connection to remain active when the app is backgrounded or the screen is locked.
- A bounded screen-off CPU lease is renewed only while MASQ needs it and is released when the
  screen turns on, the user disconnects, or validated network access disappears.
- If Android reclaims the app process, the service can restore the saved consumer profile and
  device-encrypted wallet, rediscover entry nodes, and retry the private connection without
  enabling whole-device VPN routing.
- Connection health now requires a real entry peer and route progress; stalled or zero-peer states
  trigger bounded automatic recovery instead of showing a false connected state.
- Profile-dependent actions remain disabled until saved native status and configuration have both
  been restored, preventing startup taps from using temporary defaults.
- A configured native core without a matching saved profile now fails closed with a retry action
  instead of silently creating or overwriting a profile.
- Retrying controller initialization preserves the existing profile and editable settings; only a
  genuinely unconfigured installation receives the deterministic default profile.
- Saved RPC, entry-node and routing preferences are semantically validated and compared with every
  native status field that represents the active profile before profile actions are unlocked.
- If an invalid profile cannot be reloaded, an explicit confirmed recovery removes only network
  settings, verifies that MASQ stopped, and preserves the consumer wallet.
- Status and network polling remain paused until initialization has committed one current profile
  snapshot, so an older startup result cannot overwrite fresher native state.
- Public documentation now states explicitly that whole-device and selected-app routing remain
  unavailable, matching the enforced safety gate.
- The direct-release builder pins the established update certificate and keeps signing passwords
  out of Gradle, Cargo and Node build subprocesses.

## Included

- Consume-only MASQ routing; the phone does not serve traffic for other peers.
- A fail-closed MASQ Private browser and a separate, explicitly confirmed direct-browser mode.
- Automatic entry-node discovery and refresh.
- Wallet import from 12 recovery words or a private key, stored in operating-system secure storage.
- Configurable hop count and preferred exit country.
- Experimental Android `VpnService` and packet-translator foundations, kept behind a public safety
  gate pending complete lifecycle and leak testing.
- Balanced/Strict browser protection with exact-host exceptions and Reject-only handling for
  supported consent managers.
- ENS `.eth` websites through the eth.limo HTTPS gateway without search or Direct fallback.
- Temporary WebView sessions by default. Remembered exact-host sign-ins are offered only when the
  installed Android WebView supports isolated multi-profile storage.
- A persistent, low-priority Android notification while a requested MASQ connection is active in
  the background.

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
- Whole-device and selected-app routing cannot be started in this preview.
- Keeping a connection active while the screen is off can increase battery use. Android Doze,
  manufacturer battery policies, loss of validated network access, or a user force-stop can still
  suspend or end the connection.
- Browser protection is best effort and does not claim YouTube ad blocking.
- GitHub-installed builds do not update automatically.
- Back up the wallet recovery phrase before uninstalling. Never post it in a GitHub issue.

The source corresponding to this APK is available from this release tag under GPL-3.0-only.
