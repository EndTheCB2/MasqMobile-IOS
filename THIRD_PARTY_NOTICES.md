# Third-party notices

MASQ Mobile contains adapted MASQ Node source under GPL-3.0-only. See `LICENSE`,
`masq-node-mobile/LICENSE`, and `MODIFICATIONS.md`.

The Android packet adapter uses `tun2proxy` 0.8.2 under its MIT licence. Its exact transitive Rust
dependency set is recorded in `masq-mobile/native/masq-packet-tunnel/Cargo.lock`.

MASQ Node uses a vendored `web3` 0.11.0 source copy under the MIT licence, patched with an iOS
CFStream socket connector. The upstream notice is retained in
`masq-node-mobile/vendor/web3-0.11.0/LICENSE`.

React Native, AndroidX, CocoaPods, Cargo, and npm dependencies retain their respective licences.
Exact resolved versions and integrity hashes are recorded in `masq-mobile/package-lock.json`,
`masq-mobile/ios/Podfile.lock`, `masq-node-mobile/node/Cargo.lock`, and the packet-tunnel lockfile.

The browser-protection rules and cosmetic selectors in this repository are authored for MASQ
Mobile. The binary does not bundle EasyList, uBlock rules or another external filter list, and it
does not download a remote list at runtime. It contains no YouTube or Google source code or licensed
Google filter list, and no permission or endorsement by Google or YouTube is implied.

The exact resolved dependency inventories for this preview are recorded in the npm, CocoaPods,
Cargo and Gradle lock/configuration files listed above. Applicable upstream licence files are
retained with the corresponding source. Downstream distributors remain responsible for reviewing
those inventories and including any additional notices required by their distribution channel.
