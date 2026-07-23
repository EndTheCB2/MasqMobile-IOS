# MASQ Mobile — prioritized product roadmap

## Implemented in the current consumer build

- Automatic entry-node refresh during connection attempts, with a last-known-good cache, parallel
  reachability tests, backoff, and actionable diagnostics.
- A configurable route length from one through six MASQ hops. The default remains one hop because
  the current public neighborhood is often too sparse for the desktop three-hop default.
- An optional ISO exit-country preference with either strict fail-closed behavior or a user-approved
  fallback to another available country.
- A fail-closed Android `VpnService` with whole-device and selected-app scopes. iOS remains
  browser-only and explicitly reports the missing Packet Tunnel entitlement.
- Saved network and routing preferences are restored into the setup screen. A user can change route
  preferences without re-entering or deleting the wallet protected by iOS Keychain or Android
  Keystore.
- Separate network-profile, wallet, and full-reset actions, each with an explicit confirmation and
  without deleting unrelated state.
- iOS/Android path monitoring, foreground recovery, race-free status polling, and automatic reconnection
  after a recoverable interface change.
- Explicit connection cancellation, fail-safe core shutdown, and automatic cleanup of transient
  warnings after the monitored network or MASQ route has demonstrably recovered.
- A real proxy preflight before the private browser opens, five visible connection stages, actual
  route-hop reporting, and bounded recovery from transient WebKit connection failures.
- A live exit-country inventory plus MASQ/Base ETH balances and current gas-reserve warnings.
- A hardened WebView profile for both embedded modes that blocks mixed content and local-file
  access, isolates the process pool, disables popup windows and debugging, and retains the
  HTTPS-only navigation gate.
- An explicit direct browser with a public-IP warning and native `blocked | masq | direct` routing
  states. It stops active MASQ/system routing first, and MASQ Private never falls back to it.
- A privacy-safe, redacted diagnostics report that excludes wallet material, URLs, IP addresses,
  node descriptors, and local filesystem paths.
- Commit-pinned CI for UI/core/engine checks, an unsigned iPhone build, an unsigned Android release,
  a source-privacy gate, weekly dependency updates, security reporting guidance, and lockfile-based
  dependency notices.

## Next improvements

1. **iOS Packet Tunnel distribution.** Add a separately signed Network Extension target, shared
   app-group configuration, key handoff, on-demand rules, and Apple Developer provisioning. The
   current app already fails closed and reports unsupported until that entitled target exists.
2. **Android tunnel hardening.** Independently review the TUN adapter, add explicit MASQ-native UDP
   support, persist service status across process death, and run long-duration Wi-Fi/mobile-data
   handover tests before Play distribution.
3. **Live actor reconfiguration.** iOS path changes and foreground recovery are handled now, but an
   already-running MASQ actor system cannot yet replace every lost neighbour without a controlled
   restart. Add live neighbour replacement, pause new browser loads during repair, and keep all
   recovery inside MASQ without a direct-network fallback.
4. **Cost prediction.** Extend current balance and gas warnings with per-route spend estimates and
   price impact before a hop-count change.
5. **Biometric wallet protection.** Add optional Face ID/device-passcode or Android biometric access
   control to the protected wallet key, with a carefully tested recovery and reset flow.
6. **Structured support bundle.** Extend the current redacted diagnostics report with opt-in route
   timings and categorized local logs, retaining the existing exclusion of wallet material, URLs,
   IP addresses, node keys, and local user paths.
7. **Accessibility and localization.** Finish the VoiceOver traversal audit, Dynamic Type edge
   cases, reduced-motion behavior, contrast auditing, and localized user-facing strings.
8. **Signed distribution.** Add protected TestFlight/Play testing delivery, provenance/SBOM
   attestations, reproducible release signing, and an independent mobile security review.

## iOS per-app routing boundary

Apple per-app VPN on iPhone is assigned to managed apps by a device-management service. A normal
consumer app cannot enumerate all installed apps and let an unmanaged user selectively intercept
their traffic. An enterprise/organisation edition can support per-app VPN through MDM; the consumer
edition should instead offer whole-device protection or transparent domain rules within MASQ's own
browser.
