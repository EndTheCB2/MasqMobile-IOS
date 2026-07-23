# iOS App Store Release Checklist

This checklist prepares a clean, browser-only iOS release. It does not guarantee App Review
acceptance. Do not upload a binary until every **release gate** below has written evidence.

## Release gates outside the codebase

- **Organization membership:** Apple's review rules restrict cryptocurrency-wallet and VPN apps to
  developers enrolled as organizations. Confirm the membership type; an individual membership is
  not enough while wallet functionality remains.
- **MASQ brand permission:** obtain written permission to publish under the MASQ name and logo, or
  rebrand the app and its identifiers before submission.
- **GPL/App Store distribution:** the embedded MASQ core is statically linked and GPL-3.0-only.
  Obtain qualified legal review or additional permission covering App Store terms, code signing and
  device restrictions. Preserve the exact corresponding source for every submitted binary.
- **Token-funded routes:** obtain a documented App Review/product decision on MASQ-token payments
  for digital routing functionality. Do not conceal this mechanism in review notes.
- **Production services:** obtain written confirmation of the production node-finder operator,
  availability, retention policy and support contact. `dev2.api.masq.ai` is not treated as a
  production endpoint by the archive workflow unless explicitly approved.
- **Network data map:** document what the default RPC and entry/relay/exit nodes receive, their
  purpose and retention. Reconcile that map with `PrivacyInfo.xcprivacy`, App Privacy answers and
  the hosted privacy policy.
- **Browser-protection scope:** document that the public build uses only its bundled, MASQ
  Mobile-authored rules and collects no browser-protection telemetry. Confirm that
  YouTube-specific filtering remains compiled out. Apple requires permission under the terms of
  any third-party service the app accesses or displays; do not submit or market a YouTube-ad-
  blocking claim without written authorization.
- **Encryption export classification:** the Rust core includes non-Apple cryptography. Complete the
  App Store Connect export questionnaire and obtain any required documents before adding an export
  declaration to `Info.plist`. Do not set `ITSAppUsesNonExemptEncryption` to `NO` by assumption.

Apple references: [App Review Guidelines](https://developer.apple.com/app-store/review/guidelines/),
[App Privacy](https://developer.apple.com/app-store/app-privacy-details/), and
[Export Compliance](https://developer.apple.com/help/app-store-connect/manage-app-information/overview-of-export-compliance).

## Technical release scope

- iOS 17 or later, iPhone only.
- Bundle identifier: `com.endthecb2.masqmobile` unless a different permanent identifier is chosen
  before the first App Store Connect record is created.
- Marketing version `1.0.0`; build number starts at `1` and must increase for every upload.
- Consume-only embedded browser. **MASQ Private** uses the paid decentralized route; a separately
  confirmed **Browse without MASQ** mode stops any active MASQ connection and system routing, uses
  the ordinary device connection, and displays a persistent warning about the public IP of the
  current connection or VPN. The app does not serve peer or exit traffic.
- No iOS whole-device or per-app VPN claim. There is no Packet Tunnel extension or Network
  Extension entitlement in this browser-only target. Android system-routing UI is hidden on iOS.
- Strict ATS and HTTPS navigation remain enabled. MASQ and direct sessions use separate
  non-persistent WebKit stores. MASQ remains fail-closed; no error path selects direct mode.
- Local WebKit browser protection covers a bounded set of ad/tracker requests, cross-site cookies
  and common cookie banners. It uses no external filter list or browsing telemetry.
- `MASQ_PRIVATE_YOUTUBE_AD_BLOCKER=0` is fixed in both public iOS configurations. The public binary
  has no YouTube-specific toggle, player manipulation or broad `googlevideo` blocking.

## Prepare App Store Connect

1. Use Xcode 26 or later with the iOS 26 SDK, as required for submissions from 28 April 2026.
2. Register the permanent bundle ID and create the App Store Connect app record before upload.
3. Keep the Team ID, certificates, provisioning profiles and App Store Connect API keys outside
   this repository.
4. Host `PRIVACY_POLICY.md` at a public HTTPS URL and provide a public Support URL. Replace its
   placeholder maintainer/contact language with the legal distributor's details.
5. Complete App Privacy labels from the final privacy report and network-data map.
6. Mark unrestricted web access in the age-rating questionnaire. Complete EU DSA trader status and
   content-rights declarations for every selected storefront.
7. Prepare screenshots from the signed Release build. Never show a real wallet, seed, IP address or
   personal notification in screenshots.
8. Attach evidence of MASQ trademark/content rights and any required VPN/cryptography licences in
   App Review notes.
9. Describe browser protection as bounded and local. State that cookie-banner hiding does not make
   consent choices, and that the submitted binary contains no YouTube-specific filtering.

Apple references: [create an app record](https://developer.apple.com/help/app-store-connect/create-an-app-record/add-a-new-app/),
[upload builds](https://developer.apple.com/help/app-store-connect/manage-builds/upload-builds), and
[upcoming requirements](https://developer.apple.com/news/upcoming-requirements/).

## Create a signed archive

Install dependencies and select the exact production node-finder first:

```bash
cd masq-mobile
npm ci
rustup toolchain install 1.77.2 --profile minimal
rustup target add --toolchain 1.77.2 aarch64-apple-ios
bundle install
cd ios
bundle exec pod install
cd ..
```

Archive without writing private signing values into source control:

```bash
APPLE_DEVELOPMENT_TEAM='YOUR_TEAM_ID' \
MASQ_NODE_FINDER_URL='https://production-node-finder.example' \
MASQ_BUILD_NUMBER='1' \
npm run archive:ios
```

If the reviewed operator confirms that an endpoint with `dev`, `test` or `staging` in its hostname
is production-approved, add `ALLOW_DEVELOPMENT_NODE_FINDER=YES`. Record that approval with the
release evidence. The script validates the identifier, version, node-finder setting and scans the
archive for local user paths and development signing strings.

Do not publish a signed `.ipa` or `.xcarchive` on GitHub: provisioning profiles can disclose the
team and distribution context, and GitHub is not an iOS installation channel. Publish the source
commit/tag on GitHub; distribute the binary through TestFlight or the App Store.

## Validate before upload

1. Open the archive in Xcode Organizer and run **Validate App**.
2. Generate the archive's Privacy Report. Verify Hermes and every included SDK manifest/signature.
3. Run the app on a clean device, Wi-Fi, cellular and an IPv6-only/DNS64/NAT64 network.
4. Test first launch, direct browsing without a wallet, 12-word import, wallet replacement/removal,
   offline recovery, automatic fresh entry-node retry, one through six hops, exit-country
   fallback/block, background privacy shield, private browser redirects and fail-closed behaviour.
5. In **MASQ Private**, confirm the destination sees the exit IP. In **Browse without MASQ**,
   confirm the destination sees the IP of the ordinary connection or active third-party VPN and
   the in-app warning remains visible. Confirm that MASQ is disconnected and that reconnecting is
   required before **MASQ Private** can open again. Force a MASQ error and confirm it never opens
   or selects direct mode. The iOS release must never claim that other apps are protected.
6. Confirm camera, microphone, geolocation, local-file navigation, HTTP downgrade, private/local
   addresses, popups and automatic direct WebView fallback remain denied in both modes.
7. Test all four browser-protection toggles and sites that require first-party login cookies in
   both modes.
   Confirm the public build has no **YouTube best effort** option and normal YouTube playback is not
   targeted by the generic iOS rules.
8. Re-run `npm run verify:all` and preserve its log, the source commit, archive, dSYMs, privacy
   report and legal approvals as one release record.

Upload first to internal TestFlight, complete a review rehearsal with the template in
`APP_REVIEW_NOTES_TEMPLATE.md`, and only then submit the production build.

Relevant policy references:
[App Review Guideline 2.5.6](https://developer.apple.com/app-store/review/guidelines/#software-requirements)
requires the appropriate WebKit framework for web browsing, while
[Guideline 5.2.2](https://developer.apple.com/app-store/review/guidelines/#intellectual-property)
requires permission under third-party service terms. YouTube's own
[ad-blocker notice](https://support.google.com/youtube/answer/14129599?hl=en) says ad blocking
violates its Terms and may cause playback to be blocked.
