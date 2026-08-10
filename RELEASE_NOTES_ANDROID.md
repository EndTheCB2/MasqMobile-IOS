# MASQ Mobile Android 1.0.0-preview.22 private dogfood

This is an experimental, independently maintained, consume-only MASQ Mobile test build. It is
not an official MASQ Project release, has not completed an independent mobile security audit and
must be used only on a test device with a low-value wallet.

## Download

Install `MASQ-Mobile-Android-v1.0.0-preview.22-PRIVATE-DOGFOOD.apk` only from this GitHub
prerelease. The release also contains a SHA-256 checksum and a release-specific README. Although
the filename says `PRIVATE-DOGFOOD`, every asset on this public GitHub release is publicly
downloadable.

## Updating Preview.21

Preview.22 keeps application ID `com.endthecb2.masqmobile.dogfood`, signing-certificate SHA-256
`ef2e8365621df8fcb1854c8fae6fa8f289d912cb524c3d481aa47e989082175b` and raises the Android
version code from 6021 to 6022. It can therefore update Preview.21 without uninstalling it, so
Android preserves the encrypted wallet, browser settings and network profile. Back up the wallet
recovery phrase before every test-build update.

## Changes since Preview.21

- A payable balance is evaluated separately for every creditor Node.
- Approximately 0.01 MASQ becomes eligible for automatic payment after it is more than 20 minutes
  old; eligible balances are normally scanned about every 10 minutes. The official Node scanner
  uses strict comparisons and whole-gwei configuration, so the compatibility floor is one gwei
  below the nominal 0.01-MASQ trigger.
- Existing profiles receive the new threshold at the next complete embedded-Node restart without
  resetting their wallet, route profile or recorded debts.
- The wallet and privacy screens now distinguish automatic Node payments from the optional early
  `Review MASQ debts` flow.
- The app discloses that automatic payments can spend MASQ and Base ETH fees without another
  confirmation prompt. A low threshold may cause more frequent blockchain fees.

## Included experimental functionality

- Consume-only MASQ routing and a fail-closed MASQ Private browser.
- A separately confirmed direct-browser mode.
- Automatic entry-node discovery, configurable hops and preferred exit country.
- Wallet import from 12 recovery words or a private key in Android secure storage.
- Balanced and Strict browser protection, ENS gateway support, Timpi and DuckDuckGo search.
- Experimental whole-device and selected-app routing for captured IPv4 TCP/443 and virtual DNS.

## Important limitations

- Whole-device and selected-app routing are not a production VPN. Unsupported captured traffic is
  blocked only while Android retains the VPN capture; VPN revocation or service/process death can
  restore direct networking. Always-on and lockdown modes are unsupported.
- The temporary loopback MASQ proxy has no per-run client authentication. Another malicious local
  app that discovers its port could consume the route and wallet funds.
- Availability and performance depend on reachable MASQ peers and the development node-finder at
  `https://dev2.api.masq.ai`.
- Automatic and manual payments require enough MASQ, Base ETH and a working blockchain RPC.
- Browser protection is best effort and does not claim YouTube ad blocking.
- GitHub-installed builds do not update automatically.

The corresponding source is available from this release tag under GPL-3.0-only.
