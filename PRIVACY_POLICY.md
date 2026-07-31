# MASQ Mobile Privacy Policy

Effective: 27 July 2026

MASQ Mobile is an independent, open-source, consume-only client. It is not an official release of
the MASQ Project. This direct-download preview is maintained and distributed through the
`EndTheCB2` GitHub account, with public support through this repository's issue tracker. It is
available from GitHub Releases wherever direct Android installation is permitted; users are
responsible for complying with local law.

## Summary

The app has no maintainer-operated account, advertising SDK, analytics SDK or cross-app tracking.
It does not sell personal data. It stores the consumer wallet secret and network profile on the
device. Using MASQ sends data to a blockchain RPC, a node-finder, MASQ peers and the websites the
user chooses to visit. Free-text browser searches are sent to Timpi Search, while explicit
normalized `.eth` addresses are loaded through the independent eth.limo HTTPS gateway. Direct
browsing instead uses the device's normal internet connection. Those independent services may
process or retain network data under their own policies.

## Data stored on the device

- The recovery phrase or private key is stored in device-bound secure storage. On iOS it uses a
  Keychain item available only while the device is unlocked and is not migrated to another device.
- The wallet address, selected chain, RPC URL, entry-node cache, hop preference, exit-country
  preference and connection state are kept in the app's private storage.
- On iOS, the two embedded browser modes use separate non-persistent WebKit data stores. Website
  cookies, cache and page history are not written to persistent website storage, but may remain in
  process memory until the app's web-content process exits. The app clears that in-memory website
  data before it prepares a new browser session and when protection settings change.
- On Android, cookies and website storage are temporary by default. If the installed Android
  WebView supports isolated profiles, a user can explicitly remember a sign-in for one exact host.
  MASQ and Direct use different profiles, and profiles for different remembered hosts are
  separate. If this runtime capability is unavailable, the control remains disabled and sessions
  stay temporary.
- Remembered profiles contain WebView-managed cookies and website data. MASQ does not extract
  passwords or session tokens. Cross-site top-frame links and redirects switch to the
  destination's own profile. Embedded third-party resources can still retain their own data inside
  the current site's isolated profile. Android blocks top-frame non-GET form navigations because
  the platform does not expose them to the browser's normal navigation policy; sign-in flows that
  require such a form may therefore not work. **Forget this site**, **Clear all remembered
  sign-ins**, and **Reset everything** remove the corresponding retained profiles. Some providers,
  including Google, may refuse authentication inside an embedded WebView even when persistence is
  enabled.
- Browser-protection choices are stored as local app preferences. The advertising/tracker,
  cross-site-cookie, cookie-banner and optional-cookie-rejection rules are authored for MASQ Mobile
  and bundled with the app; no external filter list or browsing history is downloaded or
  synchronized.
- The app does not intentionally include wallet secrets in diagnostics. A user decides whether to
  share the redacted diagnostic report.

The wallet can be removed with **Remove wallet from this device**. Network settings can be removed
separately. **Reset everything** deletes both. Deleting the app also removes app storage, subject to
the operating system's normal Keychain behaviour. Funds remain on the blockchain and can be
recovered only with the user's own recovery phrase or private key.

## Data sent when the app is used

### Blockchain RPC

The selected RPC receives the device's public IP address, the public wallet address, chain queries
and transaction-related JSON-RPC requests. It does not need the recovery phrase or private key.
This preview defaults to the public
[Nodies Base endpoint](https://docs.nodies.app/rpc-services/pokt-public-rpc-endpoints) on Base
Mainnet and the public [PublicNode Base Sepolia endpoint](https://base-sepolia.publicnode.com/) on
Base Sepolia. These are independent services whose availability, privacy and retention practices
are controlled by their operators. A user-selected RPC is governed by its own operator.

### Entry-node discovery

The node-finder receives the public IP address, requested MASQ chain and public network/suburb. It
returns candidate public node descriptors. This experimental preview uses
`https://dev2.api.masq.ai`, a development endpoint on the MASQ domain. It is not operated by this
repository's maintainer, and its availability and retention practices are controlled by that
independent operator. Do not use this preview if that development-service dependency is
unacceptable.

### MASQ network and destination sites

MASQ entry, relay and exit nodes process the routing metadata required for a route. Different nodes
can observe different portions of the connection. The destination site receives the exit node's IP
address and normal web requests, including any data the user submits to that site. MASQ Mobile does
not control independent nodes or destination sites and cannot promise that they retain no data.

In **MASQ Private**, the browser is fail-closed: if its local proxy or private route is unavailable,
the page is blocked rather than loaded through the device's direct connection. A MASQ error never
switches the browser to direct networking. The public iOS build does not claim to protect traffic
from other apps.

### Android system-routing dogfood

Whole-device and selected-app routing are available only in a separately compiled Android
**MASQ Dogfood (Unsafe)** package, not in the public preview. When the user explicitly selects
apps, their Android package IDs and the consent timestamp are stored only in that dogfood app's
private local preferences; they are not sent to the maintainer. Android applies VPN inclusion by
UID, so apps that share a UID can share routing behavior, attached restricted profiles may also
receive the scope, and a work-profile copy belongs to a separate Android user/profile scope. MASQ
packages installed when the route is established are excluded so their Node/control traffic and
explicit Direct browsers do not loop through that TUN.

This internal adapter translates only captured IPv4 TCP connections to port 443 and virtual DNS
through MASQ. All other captured IP traffic—including other TCP ports, non-DNS UDP, IPv6, ICMP and
unknown transports—remains blocked only while Android still reports a valid TUN capture. Activation
opens a real HTTP CONNECT tunnel to `example.com:443` through the current MASQ exit as a reachability
test; it does not request a page or response body. Android snapshots package-to-UID inclusion and
exclusion when the TUN is established. Turn routing off before installing, removing, enabling,
disabling or updating apps, then reapply it; otherwise the saved selection/exclusion may no longer
match the running UID scope. On Android 13 or later, activation is refused unless notification
permission is granted so the ongoing unsafe-routing state remains visible; turning routing off
does not require that permission.

Revoking VPN permission, or termination of the service or app process, can restore direct
networking; the dogfood build is not a fail-closed VPN guarantee. Android Always-on VPN and “Block
connections without VPN”/lockdown mode are unsupported. The local loopback MASQ proxy is
unauthenticated; a malicious local app that discovers its temporary port could use the route and
consume wallet funds. This dogfood mode must not be represented as protecting all traffic and must
not be publicly distributed until package-change recovery and per-run local proxy authentication
or peer-UID enforcement are implemented.

### Direct browsing

**Browse without MASQ** is a separate action that requires an explicit confirmation for every
session. Before opening it, the app stops any active MASQ connection and system routing. It does
not use MASQ entry, relay or exit nodes; hop-count and exit-country preferences do not apply.
Destination sites receive the public IP used by the device's current network, VPN or relay. The
internet provider and DNS resolver can observe the network metadata normally available to them,
including connection destinations where applicable. HTTPS encrypts request and response contents
in transit, but does not hide the public IP addresses or all destination metadata. The user must
reconnect before opening **MASQ Private** again.

The selected browser routing mode is not stored. Closing the browser or moving the app to the
background returns the native browser network state to blocked. Local browser-protection rules can
still filter recognized resources in direct mode, but filtering does not hide the public IP
address or provide MASQ routing.

### Timpi Search

When browser input is not recognized as a public web address, the app opens the public Timpi Search
website with that input as the search query. In **MASQ Private**, the request follows the selected
MASQ route and Timpi sees the exit node's IP address. In **Direct**, Timpi sees the public IP used by
the device's current connection or VPN. MASQ Mobile does not call Timpi's private data API and does
not separately log, synchronize or send the query to the maintainer.

Timpi is an independent service. Its published privacy policy states that it may process search
queries, country-level location, temporary IP data, server-log information and information used for
advertising. Its current terms and retention practices are controlled by Timpi:
[Timpi privacy policy](https://timpi.io/wp-content/uploads/2025/07/Timpi-International-Ltd-Privacy-Policy.pdf).

### ENS websites

Normalized ASCII or punycode `.eth` browser addresses are translated locally to the matching
`eth.limo` HTTPS gateway address. The original `.eth` name remains visible in the address bar.
eth.limo and its infrastructure can process the requested ENS name, path and normal connection
metadata. In **MASQ Private**, the gateway sees the MASQ exit IP; in **Direct**, it sees the public
IP of the current connection or VPN. A gateway failure is shown as an error and never falls back to
Timpi Search, ordinary DNS, or Direct browsing.

### Browser protection

Browser protection is performed on the device. The public build can block a limited set of known
advertising and tracking requests, strip cookies from cross-site requests and hide recognized
cookie-consent banners. The feature does not contact a filter-list service and does not report
visited URLs, matching rules, blocked-resource counts or page contents to the maintainer.
Destination sites still receive the requests that are not blocked and may detect that resources
were filtered.

Balanced and Strict presets use a versioned rule bundle shipped with the app. A malformed future
bundle falls back to the reviewed last-known-good rules. Cookie-banner hiding is never used as a
substitute for consent. A separate **Reject optional cookies** preference is disabled by default.
When enabled, exact Reject controls are used for supported OneTrust, Cookiebot, Didomi,
Usercentrics and DPG Media dialogs. A banner is hidden only after a verified Reject action. The app
never chooses **Accept**, and an unrecognized consent gate stays visible.

Protection can be disabled for one exact host to restore compatibility. This permits that site's
cookies, advertising and trackers under its own policy; it does not disable protection elsewhere.
Temporary sessions reapply the rejection preference. A remembered session may retain the site's
consent cookie until the user forgets it. Blocking and rejection rules cannot recognize every
advertisement, tracker or banner and may occasionally make a site less functional.

YouTube-specific filtering is compiled out of the public app. A separately signed, direct-install
iOS build from the no-NFT codebase can enable an optional, narrow best-effort experiment for known
YouTube advertising endpoints and player states. It does not broadly block `googlevideo` hosts,
cannot guarantee an ad-free experience and may interrupt playback. YouTube states that ad blocking
violates its Terms of Service and may result in blocked playback:
[YouTube ad-blocker notice](https://support.google.com/youtube/answer/14129599?hl=en). The private
experiment adds no telemetry or external filter-list service.

## Sharing and disclosure

The app itself does not transmit data to the maintainer except when a user deliberately opens an
external support channel or shares diagnostics. Information voluntarily posted to a public GitHub
issue is public. Never post recovery words, private keys, exact browsing history or unredacted logs.
Data may otherwise be processed by the RPC operator, node-finder operator, MASQ peers, destination
sites and infrastructure providers described above.

## Security and limitations

The software uses operating-system secure storage, encrypted MASQ transport and temporary browser
sessions by default. Remembered Android sessions are available only with runtime-verified WebView
profile isolation and require an exact-host opt-in. Direct browsing deliberately does not provide
MASQ anonymity. No
software or decentralized network can guarantee complete anonymity or security. Keep a separate
offline backup of the recovery phrase and use only funds appropriate for experimental software
until the distributor completes an independent security review.

## Children and web content

The app can access unrestricted web content and is not designed for children. This direct-download
preview has no app-store age rating; users and downstream distributors must follow applicable
local age and content rules.

## Changes and contact

Source-policy changes are recorded in this repository. Project and privacy questions can be opened
through [GitHub Issues](https://github.com/EndTheCB2/MasqMobile-IOS/issues). Do not include
wallet secrets, private keys, IP addresses, exact browsing history, or other sensitive data in a
public issue.
