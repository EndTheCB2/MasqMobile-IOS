# Modification notice

This repository contains changes to MASQ Node and new mobile integration work. The mobile adaptation
was developed during 2026 and differs from the upstream MASQ Node 1.0.0 codebase in these principal
areas:

- embedded, consume-only runtime lifecycle for iOS and Android;
- mobile-safe network discovery, route selection, proxying, and status reporting;
- configurable hop count and exit-location preferences;
- wallet import and mobile persistence bridges;
- settlement, receipt-session, and database migration support required by the embedded runtime;
- React Native user interface and native WebView integration with explicit `blocked`, `masq` and
  `direct` routing states, structurally separate non-persistent iOS data stores and no automatic
  direct fallback;
- versioned local ad/tracker, cross-site-cookie and Reject-only consent protection with
  Balanced/Strict presets, exact-host exceptions and a reviewed last-good fallback, without
  browsing telemetry;
- explicit ENS `.eth` translation to eth.limo HTTPS transport with no search or Direct fallback;
- opt-in exact-host remembered WebView profiles, separated by MASQ/Direct mode and enabled only
  after AndroidX WebKit confirms multi-profile support, with cross-site transitions re-profiled
  and Android top-frame non-GET requests blocked fail-closed; and
- iOS/Android build scripts, tests, diagnostics, and fail-closed security controls, including
  Android NDK-only native archiving and final-APK ELF linkage verification.

The standard iOS build compiles YouTube-specific filtering out. Its generic protection deliberately
avoids broad `googlevideo` blocking because those hosts also deliver requested media.

The complete modified source is present in `masq-node-mobile/`; the original MASQ copyright and
GPL-3.0-only licence notices are preserved. This file is a summary, not a substitute for the Git
diff or individual source history.
