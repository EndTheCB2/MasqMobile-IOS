# MASQ Mobile Privacy Policy

Effective: 23 July 2026

MASQ Mobile is an independent, open-source, consume-only client. It is not an official release of
the MASQ Project. This direct-download preview is maintained and distributed through the
`EndTheCB2` GitHub account, with public support through this repository's issue tracker. It is
available from GitHub Releases wherever direct Android installation is permitted; users are
responsible for complying with local law.

## Summary

The app has no maintainer-operated account, advertising SDK, analytics SDK or cross-app tracking.
It does not sell personal data. It stores the consumer wallet secret and network profile on the
device. Using MASQ sends data to a blockchain RPC, a node-finder, MASQ peers and the websites the
user chooses to visit. Free-text browser searches are sent to Timpi Search. Explicit direct
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
- On Android, cookies and website storage are cleared when a temporary browser session starts and
  closes. Android WebView may still use app-private storage while that session is active.
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

### Browser protection

Browser protection is performed on the device. The public build can block a limited set of known
advertising and tracking requests, strip cookies from cross-site requests and hide recognized
cookie-consent banners. The feature does not contact a filter-list service and does not report
visited URLs, matching rules, blocked-resource counts or page contents to the maintainer.
Destination sites still receive the requests that are not blocked and may detect that resources
were filtered.

Cookie-banner hiding is a visual best-effort feature; by itself, it does not accept or reject
consent on the user's behalf. A separate **Reject optional cookies** preference is disabled by
default. When enabled, it currently recognizes DPG Media/HLN dialogs and deliberately follows
**Configure → Reject all**. It never chooses **Accept**. An unrecognized full-page consent gate is
left visible because hiding it could leave the page inaccessible.

The app reapplies the user's rejection preference in each temporary browser session. iOS does not
durably retain the website's consent cookie in its non-persistent stores. Android clears cookies
and website storage when the browser starts and closes, although WebView may use app-private
storage while a session is active. A site may therefore display and reject its dialog again in a
later session. Blocking, hiding and rejection rules cannot recognize every advertisement, tracker
or banner and may occasionally make a site less functional.

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

The software uses operating-system secure storage, encrypted MASQ transport and isolated temporary
browser sessions. iOS uses non-persistent WebKit stores; Android clears WebView cookies and website
storage at session boundaries. Direct browsing deliberately does not provide MASQ anonymity. No
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
