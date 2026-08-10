# MASQ Mobile — architecture and project analysis

## Analyzed foundation

The implementation builds on `MASQ-Project/Node` v1.0.0 plus the locally available security and
product patches. The relevant upstream properties are:

- `consume-only` already exists as an official neighborhood mode. In this mode, Node starts no
  `ProxyClient` (exit/serving) and no automatic router port mapping.
- The local TLS listener already accepts HTTP `CONNECT` and passes the tunneled TLS stream through
  the existing `ProxyServer`, Hopper, and Neighborhood actors.
- A route is usable only at `OverallConnectionStage::RouteFound`; an open socket or TCP connection
  to a single neighbor is not sufficient.
- The desktop bootstrap normally requires root privileges because it binds ports 80 and 443 and
  then drops privileges. That model does not fit a mobile app sandbox.

## Mobile adaptations to Node

`--mobile-proxy-port PORT` adds a protected bootstrap path:

1. The option is valid only together with `--neighborhood-mode consume-only`.
2. Ports 80 and 443 are not bound.
3. One high-numbered port is bound exclusively to `127.0.0.1`.
4. The listener reports reception port 443 internally so that the existing TLS/CONNECT
   discriminators and ProxyServer logic continue to work unchanged.
5. The service does not require a root process.
6. The embedded Actix runtime reports start, pause, shutdown, and route status to the mobile core
   within the process. Ordinary **Pause MASQ** disables browser routing but keeps the consume-only
   mesh warm. Explicit Direct Browse first disables system and WebView routing, then stops and joins
   the actor-system thread without calling `process::exit`. Process-global mobile logger state is
   cleared during teardown, so the saved wallet and profile can start a fresh MASQ actor system
   later in the same app process.

Historically, the database also creates an unused clandestine-port value in consume-only mode. The
port probe now uses a regular operating-system listener instead of Tokio before the reactor is
active. The same deferred-registration fix is applied to the real listeners on modern Apple
platforms.

## Traffic paths

```text
MASQ Private WebView
  -> HTTP CONNECT to 127.0.0.1:<random high-numbered port>
  -> HttpRequestDiscriminator (logical reception port 443)
  -> ProxyServer
  -> Hopper / Neighborhood route
  -> MASQ routing nodes
  -> selected MASQ exit node
  -> HTTPS destination

Explicit Direct WebView
  -> ordinary device internet connection
  -> HTTPS destination
```

A proxy configuration without a route is rejected. Android receives no `addDirect()` rule; iOS
sets `failoverAllowed` to false. Losing the route can therefore produce an error, but it must not
cause an invisible direct connection. Direct browsing is a separately confirmed UI action, never a
recovery path. During a temporary app switch, the active WebView remains mounted behind the privacy
shield and retains its exact MASQ Private or Direct routing lease; its page can continue network
activity. Explicitly closing the browser selects `blocked`. Operating-system process eviction can
still discard the page and route lease.

The separately compiled Android dogfood package can additionally capture IP packets from a system
`VpnService` TUN. A separately locked Rust library converts only IPv4 TCP/443 and virtual-DNS flows
to the local MASQ HTTP CONNECT proxy. All other captured IP traffic—including other TCP ports,
non-DNS UDP, IPv6, ICMP and unknown transports—remains blocked while capture is valid. MASQ packages
installed when the TUN is created are excluded so Node, explicit Direct browsing and translator
control sockets do not loop through that TUN. Android snapshots package-to-UID scope at establish
time; dogfood users must turn routing off before package changes and reapply it. If translation
exits unexpectedly, Android retains an exact still-valid TUN lease as a blocker until explicit
cleanup or safe same-descriptor recovery. Activation makes a real CONNECT to `example.com:443`
through the exit as a no-page/body reachability check. Revocation invalidates capture immediately,
and service/process death can restore direct traffic. The temporary loopback proxy is unauthenticated,
which blocks external dogfood distribution pending per-run authentication or peer-UID enforcement.

## Platform choices

### iOS

- Minimum iOS 17 because of `WKWebsiteDataStore.proxyConfigurations`.
- Network.framework creates an HTTP CONNECT proxy configuration only for the MASQ WebView. The
  bridge owns separate non-persistent `WKWebsiteDataStore` instances for MASQ and explicit direct
  browsing. The patched WebView target is strongly linked to both exact instances. The MASQ store
  is never configured for direct networking; the direct store is cleared only in exact `direct`
  mode. Both use an unreachable localhost sink in `blocked`.
- No Packet Tunnel Provider or system VPN entitlement is included. The private browser requires
  neither, while the system-routing API reports unsupported instead of presenting a fake toggle.
- Route length is passed to the embedded Node through `--min-hops`. Exit-country preferences are
  delivered to the existing Neighborhood actor through its typed `exitLocation` message after the
  actors are bound, and are reapplied when a neighbor becomes available.
- Rust is built for each Xcode architecture and force-loaded as a static library because the
  Objective-C++ bridge resolves FFI symbols dynamically and safely blocks when they are missing.

### Android

- AndroidX WebKit `ProxyController` applies an unreachable sink in `blocked`, the localhost MASQ
  proxy in `masq`, and clears the override only in explicitly selected `direct`.
- Entering `blocked` also clears WebView cookies and website storage. Android WebView may use
  app-private storage while a temporary session is active; each session starts and closes through
  `blocked`.
- Dogfood `VpnService` can capture device scope or an allowlist of launchable package IDs. Android
  owns the one-time consent dialog; package IDs and its timestamp remain local. Android applies
  policy by UID, so shared-UID apps can share routing behavior, attached restricted profiles can
  also receive scope and work-profile copies are a separate user scope. Always-on VPN and lockdown
  are unsupported. On Android 13+, native new/sticky activation is refused without notification
  permission so the ongoing routing-state notice cannot be silently omitted; OFF cleanup is always
  allowed.
- The proxy rule and TUN translator refer exclusively to the local MASQ listener.
- The Node wrapper is built with Rust 1.77.2. The independently locked `tun2proxy` adapter is built
  with Rust 1.97.1. `cargo-ndk` packages both as `.so` files for arm64 and x86_64.

## Secrets and persistence

- The private key is not placed in operating-system process arguments; the argument vector exists
  only inside the embedded Rust thread, and the temporary hexadecimal copy is zeroized.
- React state clears the key after native import.
- Node stores its database in an app-private directory that is excluded from backups.
- The current core keeps wallet material in protected Rust memory during the active app session.
- iOS persists the wallet in an unlocked-only, device-bound Keychain item. Android encrypts it with
  AES-GCM using a non-exportable Android Keystore key and stores only ciphertext and the random IV.

## Explicitly out of scope

- Serving, routing, or exit services for other users.
- Public listening ports and UPnP/PCP/NAT-PMP.
- iOS device-wide routing without an Apple-granted Network Extension entitlement and separate
  Packet Tunnel target.
- Per-app routing on unmanaged iOS devices. Apple exposes per-app VPN assignment through device
  management for managed apps; the consumer app cannot enumerate and intercept arbitrary apps.
- HTTP websites and local/LAN destinations in the embedded browser.
- Automatic or remembered fallback from MASQ Private to direct browsing.
- Hardcoded entry nodes, RPC keys, or wallet secrets.

## Verification strategy

- Pure configuration, URL, wallet, and CONNECT parser tests.
- React Native rendering and validation tests.
- Node tests for mobile port mapping, consume-only validation, and unprivileged run mode.
- Compile test of the complete `node-engine` feature.
- A host smoke test that starts the real actor runtime, creates a database and localhost listeners,
  stops them, and confirms that the app process remains active. Resuming in the app reuses this
  runtime.
- Device validation remains required before store publication, using real entry nodes on both
  chains.
- CI makes an unsigned generic iPhone build and unsigned Android release APK, in addition to the
  UI, core, engine, packet-adapter, and source-privacy checks.
