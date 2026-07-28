# MASQ Mobile

A mobile MASQ consumer for iOS and Android. **MASQ Private** remains fail-closed, while a separate
**Browse without MASQ** action deliberately uses the ordinary device connection after a clear
public-IP warning and stops any active MASQ connection first. The app contains the real MASQ Node
actors, but starts them exclusively in
`consume-only` mode: the phone purchases routes and exit traffic but does not provide routing or
exit services to other nodes.

## What works

- React Native interface for Base Mainnet and Base Sepolia.
- Secure import from a 12-word recovery phrase or private key. iOS uses a device-bound,
  unlocked-only Keychain item; Android encrypts it with a non-exportable Android Keystore key.
- Automatic RPC health checking with chain verification and public fallback endpoints.
- Automatic selection, reachability testing, retry, and last-known-good caching of two entry nodes.
- Single-flight connection control with generation-checked Android native start/stop serialization
  and cancellable node refresh when the user disconnects, resets, backgrounds the app, or taps
  connect more than once.
- An explicit cancel action while a route is being built, with fail-safe shutdown that still stops
  the core if browser-proxy cleanup itself reports an error.
- Actionable, privacy-safe issue categories that route the user to retry, device settings, the
  network profile, or wallet recovery instead of exposing raw native errors.
- Automatic removal of transient offline, entry-node, permission, and route warnings only after
  network or core status proves that the corresponding condition recovered.
- Embedded Rust/MASQ Node runtime without a separate daemon process or root privileges.
- One random high-numbered localhost port for HTTP `CONNECT`; internally, the existing MASQ TLS
  proxy pipeline continues to see port 443.
- A non-persistent **MASQ Private** browser that opens only after a real HTTP `CONNECT` preflight
  succeeds through the local MASQ proxy and exit route.
- A separately selected direct browser that works without a wallet or MASQ route, stops any active
  MASQ connection and system routing, never inherits MASQ hop/exit settings, and continuously
  identifies the mode with the compact `DIRECT · MASQ OFF` badge.
- Bounded private-browser recovery for transient iOS and Android WebView failures, without blindly
  retrying offline or TLS-certificate failures and without letting stale timers reload a new URL.
- A hardened embedded WebView profile in both modes that disables local-file access, mixed
  content, popup windows, shared cookies and process pools, link previews, and production WebView
  debugging.
- User-controlled Balanced/Strict browser protection for known advertising/tracking resources,
  cross-site cookies and supported consent managers, plus exact-host compatibility exceptions.
- Opt-in Reject-only handling for supported OneTrust, Cookiebot, Didomi, Usercentrics and DPG
  Media dialogs. Unknown gates remain visible and **Accept** is never selected.
- ENS `.eth` navigation through eth.limo HTTPS transport without search or Direct fallback.
- Temporary sessions by default, with exact-host remembered sign-in profiles only after Android
  WebView confirms multi-profile isolation support. Cross-site top-frame transitions select the
  destination profile and top-frame non-GET form navigations are blocked fail-closed.
- Local MASQ Mobile-authored protection rules with no downloaded filter list, browsing telemetry
  or rule-match reporting.
- User-selectable route lengths from one to six hops and an optional exit-country preference with
  strict or fallback routing.
- A live country inventory learned from the current neighborhood, so unavailable exit countries
  are not presented as if they were active.
- MASQ and Base ETH balance checks, current gas-reserve guidance, and low-funds warnings without
  sending credentials to the RPC.
- Android WebView routing through AndroidX `ProxyController`, with exact `blocked`, `masq` and
  `direct` states and no automatic direct fallback.
- Android `VpnService` and an isolated Rust TUN translator are present as experimental
  foundations. Public preview UI cannot start whole-device or selected-app routing until
  process-death, network-handover and leak tests pass; Android Always-on VPN support is disabled
  in the meantime.
- iOS 17+ WebKit routing through `WKWebsiteDataStore.proxyConfigurations`, without proxy failover.
  The MASQ WebView is bound to a dedicated non-persistent data store that is never configured for
  direct networking. A second non-persistent store can use the device connection only in the
  explicit direct state; both stores point at an unreachable localhost sink while blocked.
- Native data in Android `noBackupFilesDir` and iOS Application Support, excluded from backups.
- Ordinary **Pause MASQ** blocks browser routing while keeping the consume-only mesh warm for a
  fast resume. Opening the explicit direct browser instead performs an acknowledged full teardown
  of the mesh and system tunnel while retaining the wallet and profile; MASQ can reconnect later
  in the same app process.
- Race-free native status polling, iOS/Android network-path monitoring, foreground recovery, and
  fail-closed browser shutdown when the app enters the background. Android removes the WebView
  immediately and requires an exact native `blocked` acknowledgement before leaving the browser;
  a failed acknowledgement stays on a non-browsing retry screen. Browser mode is never persisted,
  and backgrounding always selects `blocked`.
- Separate wallet, network-profile, and full resets, plus redacted diagnostics sharing.

The iOS build intentionally remains browser-only. Whole-device routing needs a separately signed
Network Extension target and Apple entitlement; per-app routing additionally requires managed apps
and MDM. Android system-routing foundations remain behind a public safety gate until the complete
tunnel lifecycle is proven fail-closed. The embedded browser continues to use the explicit MASQ
WebView proxy.

## Structure

```text
React Native UI
  -> TurboModule (Kotlin / Objective-C++)
  -> masq-mobile-core (Rust C/JNI ABI)
  -> MASQ Node actors (consume-only)
  -> high-numbered 127.0.0.1 CONNECT port
  -> MASQ route and exit node

Android apps (optional)
  -> Android VpnService TUN
  -> isolated tun2proxy Rust library
  -> the same fail-closed local CONNECT port
```

The mobile app is located in this directory. The adapted upstream Node worktree is located next to
it at `../masq-node-mobile`. Additional technical context is available in
[`docs/MASQ_MOBILE_ARCHITECTURE.md`](docs/MASQ_MOBILE_ARCHITECTURE.md).

## Requirements

- Node.js 22.11 or later.
- Rust 1.77.2 for MASQ Node and Rust 1.97.1 for the isolated Android packet translator.
- iOS: a complete Xcode installation, CocoaPods, and iOS 17+.
- Android: JDK 17, Android Studio/SDK/NDK, and a current Android System WebView.
- Android one-time setup:
  `rustup run 1.97.1 cargo install cargo-ndk --version 4.1.2 --locked`.

The two Rust crates are separate workspaces. MASQ Node stays on 1.77.2 for its older dependency
graph; the packet translator uses 1.97.1 and its own lockfile so modern TUN dependencies do not
silently upgrade Node.

## Install and run

```bash
npm install
rustup toolchain install 1.77.2 --profile minimal
rustup toolchain install 1.97.1 --profile minimal
```

iOS:

```bash
cd ios
bundle install
bundle exec pod install
cd ..
open ios/MasqMobile.xcworkspace
```

In Xcode, select the `MasqMobile` target and choose your own Apple Development Team. Debug uses
`com.endthecb2.masqmobile.debug`; Release uses the permanent `com.endthecb2.masqmobile` identifier.
Change the production identifier only before creating the first App Store Connect record. Connect
an unlocked iPhone, select it as the run destination, and press Run. No signing identity,
provisioning profile, Team ID or personal Apple account information is stored in this repository.

The Xcode target automatically builds the Rust static library for the device or simulator and
force-loads the FFI symbols into the app.

For a signed App Store archive, first approve a production node-finder and follow
[`../APP_STORE_RELEASE.md`](../APP_STORE_RELEASE.md). The archive command requires the Apple Team ID
and production service URL as local environment variables; it then verifies the embedded values and
scans the archive for personal build paths. Never commit or publish the resulting archive or IPA.

Android:

```bash
rustup run 1.97.1 cargo install cargo-ndk --version 4.1.2 --locked
npm run android
```

Gradle automatically builds the `arm64-v8a` and `x86_64` Rust libraries and packages them as
`jniLibs`. For a development APK, run `cd android && ./gradlew assembleDebug`; the output is
`android/app/build/outputs/apk/debug/app-debug.apk`. It requires Metro, so run `npm start` and
`adb reverse tcp:8081 tcp:8081` before installing it manually. A self-contained release bundle is
built with
`MASQ_NODE_FINDER_URL='https://production-node-finder.example' ./gradlew assembleRelease` and written to
`android/app/build/outputs/apk/release/app-release-unsigned.apk`. Release signing is deliberately
not stored in the repository; the distributor must sign this APK with a private release key. Debug
defaults to `dev2.api.masq.ai`, while Release deliberately fails when the reviewed production
node-finder is not supplied.

For direct GitHub distribution, `npm run build:android:direct` builds, aligns, signs, and
privacy-scans a versioned APK. It requires `MASQ_ANDROID_KEYSTORE`,
`MASQ_ANDROID_KEYSTORE_PASSWORD`, an optional `MASQ_ANDROID_KEY_PASSWORD` and
`MASQ_NODE_FINDER_URL` in the local environment. It also requires
`MASQ_ANDROID_EXPECTED_CERT_SHA256`, copied from the approved keystore's public certificate
fingerprint. The official direct-release script also pins the preview.2 certificate, so changing
only the environment cannot silently produce an incompatible update. Read the fingerprint with
`keytool -list -v -keystore "$MASQ_ANDROID_KEYSTORE"` and remove the displayed colon separators.
The keystore and passwords must remain outside the repository; the script exposes the passwords
only to the signing subprocess, after compilation has finished. End-user installation and update
instructions are in
[`../ANDROID_DIRECT_INSTALL.md`](../ANDROID_DIRECT_INSTALL.md).

## Configure

1. Select Base Mainnet or Base Sepolia.
2. Accept the verified default RPC or choose an HTTPS endpoint in Advanced settings.
3. Let the app select two reachable entry nodes, or enter descriptors in Advanced settings.
4. Import the consumer wallet with its 12 recovery words or private key.
5. Choose one to six hops and, optionally, a preferred exit country.
6. Start MASQ and follow the five-stage connection status.
7. Open **MASQ Private** after its route preflight succeeds, or explicitly choose **Browse without
   MASQ** and confirm that the normal connection exposes the device IP and bypasses MASQ routing.
8. Choose **Balanced** or **Strict**, or review the individual **Ads & trackers**,
   **Cross-site cookies**, **Hide resolved banners** and **Reject optional cookies** controls.
   Exact-host protection exceptions and remembered sign-ins are opt-in.
9. Enter a normalized `.eth` address to load it through the eth.limo HTTPS gateway while retaining
   the logical ENS address in the bar.
10. In public Android previews, only traffic from the embedded **MASQ Private** browser can use
    MASQ. Whole-device and selected-app routing remain unavailable until lifecycle, handover and
    leak tests pass.

A real route requires reachable entry nodes and a wallet that meets the network requirements. No
community nodes or private keys are intentionally hardcoded in the source code.

## Browser protection

The public app ships a deliberately bounded, MASQ Mobile-authored ruleset:

- On iOS, WebKit compiles local
  [`WKContentRuleList`](https://developer.apple.com/documentation/webkit/wkcontentrulelist) rules
  that block requests to a selected set of known ad/tracker hosts, strip cookies from cross-site
  requests and hide common ad or cookie-banner elements.
- On Android, the embedded WebView disables third-party cookies and applies the corresponding
  local cosmetic selectors. It does not claim the same native request-filtering coverage as iOS.
- **Reject optional cookies** is separate and off by default. Exact Reject controls are supported
  for selected OneTrust, Cookiebot, Didomi, Usercentrics and DPG Media dialogs. A banner is hidden
  only after rejection succeeds; unknown gates remain visible and **Accept** is never selected.
- Android sessions are temporary by default. Remembered sign-in is enabled only after AndroidX
  WebKit confirms `MULTI_PROFILE`; it then uses separate exact-host profiles for MASQ and Direct.
  Cross-site top-frame links and redirects select the destination profile. Because Android does
  not expose top-frame non-GET requests to the normal navigation policy, those form navigations
  are blocked; sign-in flows that require one may not work. Unsupported runtimes remain temporary.
  Forget-site, clear-all and reset remove retained profiles.
- **Protection off for this site** is an exact-host compatibility exception and does not affect
  other sites.
- The rules are included in the application source and binary. The app does not retrieve EasyList,
  another external list or a remote ruleset, and it does not send visited URLs, matches or block
  counts to MASQ Mobile.

Protection is best effort. Advertising and consent implementations change frequently, first-party
content can be indistinguishable from wanted content, and a rule can occasionally break a page.
Users can disable each protection class independently and reload the active WebView. These local
filters do not hide the device IP in direct mode.

The standard `MasqMobile` configurations compile YouTube-specific filtering out of the native
binary and exclude YouTube pages from the generic iOS ad/tracker request rules. The no-NFT
codebase includes `scripts/install-ios-direct-private.sh` for a separately signed,
direct-install-only build that exposes a **YouTube best effort** experiment. The script enables the
filter through a local compiler override without changing the public Release configuration or
embedding a signing team, device identifier or personal bundle ID in source control.

The experiment targets a narrow set of known ad endpoints and page/player states; it never broadly
blocks `googlevideo` hosts because those hosts also carry requested video. It cannot guarantee an
ad-free experience and may cause YouTube to refuse or interrupt playback. YouTube states that
blocking its ads violates its Terms of Service, and Apple requires apps that display third-party
services to be permitted by those services' terms. See
[YouTube's ad-blocker notice](https://support.google.com/youtube/answer/14129599?hl=en) and
[App Review Guideline 5.2.2](https://developer.apple.com/app-store/review/guidelines/#intellectual-property).
The private experiment must not be described as supported public functionality.

For a direct device installation, provide the values only in the local shell:

```bash
MASQ_IOS_DEVICE_ID='<device-id>' \
APPLE_DEVELOPMENT_TEAM='<team-id>' \
MASQ_BUNDLE_IDENTIFIER='<bundle-id>' \
MASQ_NODE_FINDER_URL='https://nodes.example.org' \
./scripts/install-ios-direct-private.sh
```

## Verify

```bash
npm run verify
npm run test:engine
npm run test:tunnel
npm run verify:privacy
```

`verify` runs TypeScript, ESLint, Jest, and the fail-closed Rust core tests. `test:engine` compiles
the complete MASQ Node integration. `test:tunnel` verifies the isolated packet adapter, and the
privacy check rejects personal paths, Apple signing identities, provisioning data, and private-key
blocks. `npm run verify:all` runs all four. GitHub Actions also makes an unsigned iPhone build and a
self-contained unsigned Android release APK with commit-pinned actions.

## Security boundaries

- The MASQ browser is blocked when the native core is unavailable; no transparent direct fallback
  exists. Direct browsing is a separate user action and works without the MASQ core.
- The MASQ WebView proxy is enabled only when MASQ has a peer, and no private page is shown until an
  exit-route `CONNECT` preflight succeeds.
- iOS uses separate non-persistent stores for MASQ and direct sessions. Strong native link
  contracts make a missing binding fail the build. Only the direct store can have an empty proxy
  configuration, and only after the explicit direct action.
- iOS uses separate ephemeral stores by default. Android uses separate MASQ and Direct WebView
  profiles when the runtime supports them, and offers exact-host persistence only after opt-in;
  unsupported runtimes remain temporary. The browser closes on background, routing returns to a
  blocked sink, and MASQ never permits proxy failover.
- Browser-protection preferences are local app settings. Protection does not send browsing
  telemetry, but destination websites still receive normal requests and can detect or react to
  blocked resources. In direct mode they also receive the public IP of the current connection or
  VPN.
- Browser navigation accepts HTTPS only and blocks localhost, private, and link-local addresses.
  Free text entered in the browser bar opens the public Timpi Search website through the selected
  MASQ Private or Direct routing mode; MASQ Mobile does not embed Timpi's private data API.
- Normalized `.eth` addresses are rewritten locally to eth.limo HTTPS transport. Gateway failure
  is reported and never falls back to Timpi, ordinary DNS or Direct networking.
- After import, the wallet secret is removed from React state and temporary Rust copies are
  zeroized. iOS persists it with `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`; Android uses
  AES-GCM with a non-exportable key held by Android Keystore.
- The Android `VpnService` keeps an established TUN descriptor open when its translator stops, but
  whole-device and selected-app routing remain disabled in public previews until process-death,
  service restart, network handover and leak testing also pass. Android Always-on VPN support is
  disabled while that validation is incomplete.
- iOS system routing returns an explicit unsupported result until a correctly provisioned Packet
  Tunnel extension exists.
- MASQ Node remains beta software. Before store publication, conduct a mobile security review,
  privacy review, and live network test.

## License

The mobile core and derivative MASQ Node adaptations use GPL-3.0-only, matching the upstream MASQ
Node project.
