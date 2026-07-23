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
- local MASQ Mobile-authored ad/tracker, cross-site-cookie and cookie-banner protection, plus an
  opt-in **Configure → Reject all** flow for supported DPG Media/HLN dialogs, without an external
  filter list or browsing telemetry; and
- iOS/Android build scripts, tests, diagnostics, and fail-closed security controls.

The standard iOS build compiles YouTube-specific filtering out. Its generic protection deliberately
avoids broad `googlevideo` blocking because those hosts also deliver requested media.

The complete modified source is present in `masq-node-mobile/`; the original MASQ copyright and
GPL-3.0-only licence notices are preserved. This file is a summary, not a substitute for the Git
diff or individual source history.
