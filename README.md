# MASQ Mobile Android preview and shared source

This repository contains an experimental, consume-only MASQ mobile client. The app embeds an
adapted MASQ Node runtime and can consume private routes without serving traffic for other peers.

It is an independent development build and is not an official MASQ release. MASQ names, logos,
and upstream source remain subject to the MASQ Project's copyright, licence, and trademark terms.

## Download the Android preview

Download the signed APK, checksum, and signing-certificate fingerprint only from this repository's
[GitHub Releases](https://github.com/EndTheCB2/MasqMobile-IOS/releases). Follow
[`ANDROID_DIRECT_INSTALL.md`](ANDROID_DIRECT_INSTALL.md) before installing it. This preview is
distributed outside Google Play, does not update automatically, and has not completed an
independent mobile security audit.

Preview.2 started the permanent Android signing lineage retained by every later release. Preview.1
users must first back up their 12-word recovery phrase, uninstall preview.1, and then install the
current preview. Preview.2 users can update in place. See the installation guide for the
certificate fingerprint and migration warning.

The repository also retains the shared iOS target and build instructions so the complete mobile
adaptation remains available as corresponding source.

## Repository layout

- `masq-mobile/` — React Native app, iOS/Android native bridges, and mobile Rust wrapper.
- `masq-node-mobile/` — adapted MASQ Node source used by the mobile wrapper.

Keep both directories next to each other. The Rust wrapper intentionally resolves the Node
workspace through `../masq-node-mobile/node`.

## Current scope

- iOS 17+ and Android application targets.
- Consumer mode only; the phone does not serve routing or exit traffic.
- Embedded **MASQ Private** browser with fail-closed proxying and a separate, explicitly selected
  **Browse without MASQ** mode for ordinary device networking.
- Bare public domains such as `example.com` open as HTTPS addresses. Genuine free text uses the
  locally selected Timpi or DuckDuckGo search provider; the app stores the provider choice but not
  the query or search history.
- Local browser protection for known advertising/tracking resources, cross-site cookies and common
  consent managers, with Balanced/Strict presets, exact-host exceptions and verified Reject-only
  automation.
- ENS `.eth` browsing through an explicit eth.limo HTTPS transport boundary while the logical ENS
  address remains visible.
- Temporary browser sessions by default, with exact-host remembered sign-in profiles only on
  Android WebView runtimes that report multi-profile isolation support.
- Android `VpnService` and packet-translator foundations for future whole-device and selected-app
  routing. The public preview keeps this UI disabled until process-death, network-handover and
  leak tests prove the complete lifecycle is fail-closed.
- Automatic RPC validation and entry-node discovery/retry.
- Wallet import, live balance warnings, configurable hop count, and live exit-country inventory.

iOS system routing remains unavailable: it requires an Apple Network Extension entitlement and a
separately signed Packet Tunnel extension. The iOS UI reports that boundary and never claims that
other apps are protected. Per-app iOS routing additionally requires managed devices through MDM.
Android Always-on VPN support is also disabled while the system-tunnel lifecycle remains under
validation. Installing this preview stops any system tunnel left by an earlier experimental build.

The browser-protection rules are a small, MASQ Mobile-authored set bundled with the app. Rule
matching happens locally: the feature downloads no external filter list and sends no browsing
telemetry or rule-match report to the maintainer. The public build deliberately compiles out
YouTube-specific filtering and does not broadly block `googlevideo` media hosts.

Consent automation only uses reviewed exact Reject controls for supported managers and hides a
resolved banner only after that action succeeds. It never chooses **Accept**. An unrecognized gate
stays visible. Android remembers a site only after explicit opt-in and a runtime
`MULTI_PROFILE` capability check; otherwise the control is disabled and the session stays
temporary. MASQ and Direct profiles remain separate.
Cross-site top-frame links and redirects always select the destination profile. Android also
blocks top-frame non-GET form navigations because WebView does not expose those requests to the
normal navigation-policy callback.

Browser routing is never selected implicitly. **MASQ Private** blocks the page if its route fails
and never falls back to the device connection. **Browse without MASQ** is a separate, temporary
choice that first stops any active MASQ connection and system routing. A compact persistent
`DIRECT · MASQ OFF` badge identifies the mode; destination sites see the public IP used by the
current connection or VPN, while the
internet provider and DNS resolver can observe normal connection metadata. During a temporary app
switch, the active page stays mounted behind a privacy shield, retains its exact MASQ Private or
Direct route, and can continue network activity. Explicitly closing the browser returns its native
routing state to blocked; Android or iOS process eviction can still lose the exact page. The user
must reconnect before opening **MASQ Private** again after a Direct session.

## Build and install on iPhone

1. Install Xcode, Node.js 22.11+, Rust 1.77.2, Ruby Bundler, and CocoaPods.
2. In `masq-mobile/`, run `npm ci`.
3. Run `rustup toolchain install 1.77.2 --profile minimal`.
4. In `masq-mobile/ios/`, run `bundle install` followed by `bundle exec pod install`.
5. Open `masq-mobile/ios/MasqMobile.xcworkspace` in Xcode.
6. Select the `MasqMobile` target and choose your own Development Team. The Debug identifier is
   separate from the permanent production identifier.
7. Connect and unlock the iPhone, select it as the run destination, and press Run.

Developer Mode is required for a directly installed development build. A normal non-Developer-
Mode installation requires TestFlight or App Store distribution with the appropriate Apple
Developer account and review/provisioning flow.

The no-NFT source branch also includes `masq-mobile/scripts/install-ios-direct-private.sh` for a
self-contained personal build with the experimental YouTube filter compiled in. It requires the
device ID, Apple Team ID, bundle ID and node-finder URL as local environment variables; none of
those identifiers are committed. This direct-install variant is not the App Store build.

## Prepare an App Store release

The iOS target has a stable production bundle ID, iPhone-only assets, automatic signing, Release
hardening, a privacy manifest, an in-app Privacy & Legal screen and a privacy-scanned archive
workflow. Signing identities and Team IDs remain local.

Before archiving, complete every legal and policy gate in
[`APP_STORE_RELEASE.md`](APP_STORE_RELEASE.md), especially Organization membership, MASQ trademark
permission, GPL/App Store distribution review, token-payment treatment, production node-finder
approval and encryption export classification. Host [`PRIVACY_POLICY.md`](PRIVACY_POLICY.md) at a
public HTTPS URL with the distributor's support contact. GitHub is for source; upload signed builds
through Xcode Organizer to TestFlight or App Store Connect.

## Build and install on Android

1. Install JDK 17, Node.js 22.11+, Rust 1.77.2 and 1.97.1, Android SDK Platform/Build Tools 36, and
   NDK `27.1.12297006`.
2. Set `ANDROID_HOME` to the Android SDK directory and accept the Android SDK licences.
3. In `masq-mobile/`, run `npm ci` and
   `rustup run 1.97.1 cargo install cargo-ndk --version 4.1.2 --locked`.
4. Connect a 64-bit Android device with USB debugging enabled, or start an x86_64 emulator.
5. Run `npm run android` to start the development workflow and install the app.
6. To install a previously built debug APK, keep Metro running with `npm start`, run
   `adb reverse tcp:8081 tcp:8081`, and install it with
   `adb install -r android/app/build/outputs/apk/debug/app-debug.apk`.

The debug APK is generated at `masq-mobile/android/app/build/outputs/apk/debug/app-debug.apk` and
requires Metro. `cd android && ./gradlew assembleRelease` creates a self-contained unsigned APK at
`app/build/outputs/apk/release/app-release-unsigned.apk`; sign that output with a privately held
release key before distributing it. Never commit the keystore or its passwords.

For the signed GitHub preview, checksum verification, safe sideloading steps, and update behavior,
see [`ANDROID_DIRECT_INSTALL.md`](ANDROID_DIRECT_INSTALL.md). Maintainers can build the same
privacy-scanned artifact with `npm run build:android:direct` after supplying the node-finder URL
and signing values through the local environment.

See [`masq-mobile/README.md`](masq-mobile/README.md) for architecture, configuration, Android
instructions, verification commands, and security boundaries.

## Privacy of this source archive

Generated builds, signing certificates, provisioning profiles, Apple team identifiers, local
paths, wallet databases, seed phrases, private keys, logs, caches, Pods, and installed dependencies
are intentionally excluded. Each builder must provide their own Apple signing team/bundle ID or
Android release keystore/application ID.

## Licence

The mobile Rust core and adapted MASQ Node source are distributed under GPL-3.0-only, matching the
upstream MASQ Node project. See [`LICENSE`](LICENSE) and
[`masq-node-mobile/LICENSE`](masq-node-mobile/LICENSE). Third-party dependencies retain their own
licences. The principal mobile adaptations are recorded in [`MODIFICATIONS.md`](MODIFICATIONS.md).
