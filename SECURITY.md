# Security policy

MASQ Mobile is experimental consume-only software and has not yet completed an independent mobile
security audit. Do not treat development builds as production wallet software. Keep an offline
backup of the recovery phrase and use only funds you can afford to expose to beta software.

## Reporting a vulnerability

Do not open a public issue for a vulnerability, wallet secret, node key, IP address, or other
sensitive diagnostic. Use GitHub's private vulnerability reporting feature for this repository.
Include the affected commit, platform and OS version, a minimal reproduction, and redacted logs.
Never include a real recovery phrase or private key.

## Supported versions

Only the latest commit on the default development branch receives security fixes. No distributed
build is considered supported unless its source commit and signing identity are documented by the
distributor.

## Security boundaries

- Missing native routing components block **MASQ Private** traffic; they do not silently bypass
  MASQ.
- **MASQ Private** is HTTPS-only and fail-closed. **Browse without MASQ** is a separate explicit
  mode that stops any active MASQ connection and system routing and visibly warns that sites see
  the public IP of the current connection or VPN. A MASQ failure never selects it automatically.
- iOS binds MASQ and direct browsing to separate non-persistent WebKit stores. Android clears
  WebView cookies and website storage when a temporary session starts and closes; WebView may use
  app-private storage while that session is active.
- Native browser routing has three temporary states: `blocked`, `masq` and `direct`. Closing or
  backgrounding the browser returns to `blocked`; the selected mode is not persisted.
- Android system routing uses `VpnService`; captured traffic stays blocked if the packet translator
  fails. Non-DNS UDP is currently blocked.
- iOS whole-device routing is unavailable until a signed Packet Tunnel extension and Apple Network
  Extension entitlement are supplied. The app does not claim device protection without them.
- Release signing keys, Apple teams, provisioning profiles, wallets, logs, local paths, and built
  artifacts must stay outside source control.
