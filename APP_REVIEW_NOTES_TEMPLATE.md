# App Review Notes Template

Copy this into App Store Connect and replace every bracketed item. Never commit the test wallet's
recovery phrase or private key to GitHub.

## Product scope

MASQ Mobile is an independent, consume-only MASQ client. It embeds a MASQ Node runtime but cannot
serve relay or exit traffic for other users. On iOS, **MASQ Private** protects only its
non-persistent in-app browser. A separate, explicitly confirmed direct mode first stops any active
MASQ connection and system routing, uses the device connection, and continuously warns that sites
see the public IP of the current connection or VPN. This submission contains no Network Extension
and makes no whole-device or per-app VPN claim.

The user imports an existing consumer wallet. The wallet pays the decentralized MASQ network for
routing and exit services. [EXPLAIN THE APPROVED APP REVIEW/IAP TREATMENT AND REFERENCE PRIOR
CORRESPONDENCE IF AVAILABLE.]

The private browser uses a small, bundled MASQ Mobile-authored ruleset for known ad/tracker
resources, cross-site cookies and common cookie banners. It downloads no external filter list and
sends no browsing telemetry or rule-match reports. The submitted `MasqMobile` binary compiles
YouTube-specific filtering out and contains no YouTube player manipulation or broad `googlevideo`
blocking.

## Review configuration

- Network: Base Sepolia
- Test wallet: [PROVIDE ONLY IN THE PRIVATE APP STORE CONNECT FIELD]
- Test funds and expiry: [DESCRIBE]
- Production node-finder operator and URL: [DESCRIBE]
- Default RPC operator and URL: [DESCRIBE]
- Support contact available during review: [NAME, EMAIL, PHONE]

## Steps to test

1. Before configuring a wallet, choose **Browse without MASQ**, read the disclosure, confirm
   **Browse directly**, and load the supplied HTTPS test URL. Verify the amber
   **DIRECT · MASQ OFF** warning remains visible, then close the browser.
2. Choose **Set up consumer wallet**, select **Base Sepolia**, let the app discover two entry nodes
   and import the supplied 12-word test
   wallet.
3. Save, choose one hop for the initial availability test, then tap **Connect to MASQ**.
4. Wait for **MASQ route ready**, tap **Open private browser**, and load the supplied HTTPS test URL.
5. Review **Ads & trackers**, **Cross-site cookies**, **Cookie banners** and
   **Reject optional cookies**. Toggle each setting and confirm that the active WebView reloads.
   There is intentionally no **YouTube best effort** control in this submitted public build.
6. Close the browser, change the hop count without resetting the wallet, reconnect and verify that
   no automatic direct fallback occurs if the MASQ route is unavailable.
7. Open **Privacy & legal** to review local storage and external network processing disclosures.

## Privacy and security

The app contains no advertising, analytics or tracking SDK. The wallet secret is stored in
device-bound secure storage and is removed only by an explicit wallet/full reset. The RPC,
node-finder, MASQ peers and destination sites receive the data explained at [PUBLIC PRIVACY POLICY
URL]. Direct browsing sends web requests through the normal internet connection, exposes the
public IP of the current connection or VPN to destinations, and does not apply MASQ hops or
exit-country settings.
Browser website data in either mode is not written to persistent WebKit storage, but may remain in
memory within an active session; iOS clears it before preparing a later browser session. Camera,
microphone and geolocation are denied.
Browser protection uses only bundled rules and reports no visited URLs or matches. Cookie-banner
hiding makes no consent choice for the user and filtering is described as best effort. Diagnostics
are redacted and shared only after an explicit user action.

## Rights and compliance attachments

- MASQ trademark/content permission: [ATTACH OR REFERENCE]
- GPL/App Store distribution clearance and exact source tag: [ATTACH OR REFERENCE]
- Encryption export determination/document reference: [ATTACH OR REFERENCE]
- Applicable regional VPN/cryptocurrency licences: [ATTACH OR REFERENCE]
- Third-party service terms review, including confirmation that this public build does not target
  YouTube advertising: [ATTACH OR REFERENCE]
