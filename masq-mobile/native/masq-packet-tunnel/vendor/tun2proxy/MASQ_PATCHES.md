# MASQ local tun2proxy patch

This directory vendors `tun2proxy` 0.8.2 from crates.io (MIT license, crate
SHA-256 `058486886fa3987ca284673b467f52f7c60e22ac6cf5d1e91f2d63b8f024bad4`).
It is kept local because the Android packet-tunnel adapter needs three small,
auditable changes that are not present in the published crate:

1. Session capacity is owned by an RAII permit. A failed HTTP UDP-associate or
   other proxy-handler setup therefore releases its slot instead of leaking it.
2. An embedding-only policy rejects unsupported IPv6, non-virtual-DNS UDP
   (including QUIC), and disallowed TCP destination ports before session
   accounting. Android still captures both IPv4 and IPv6 default routes so an
   unsupported family cannot bypass protection. Defaults retain upstream
   behavior for other embedders.
3. Optional embedding-only proxy-handshake and bidirectional TCP idle timeouts
   cancel half-open sessions. Activity in either direction resets one shared
   idle deadline, clean TCP half-closes remain supported, and dropping a timed-
   out relay releases its RAII capacity permit. Android uses 15 seconds for
   proxy setup and 120 seconds without payload for established relays.
4. Policy-, capacity-, and proxy-setup rejections use MASQ's vendored
   `ipstack::IpStackTcpStream::abort_with_reset` path. They emit RST/ACK and
   never await a graceful close in the single accept loop, so an early Chrome
   IPv6/non-443 attempt cannot prevent the following IPv4 TCP/443 fallback or
   tunnel cancellation.
5. Virtual DNS only synthesizes an address when the question type and fake-IP
   family match. AAAA, HTTPS/SVCB, and other unsupported questions receive a
   NOERROR/NODATA response rather than an invalid A answer. Fake-IP mappings
   are bounded and shared in process across translator rebinds for longer than
   their advertised TTL; destinations are never written to diagnostics.
6. TCP SYN/ACK packets advertise an IPv4 MSS derived from the TUN MTU
   (`1500 - 40 = 1460`) to reduce fragmentation and packet overhead.
7. Policy-rejected UDP/QUIC receives an immediate, locally injected ICMP
   destination/port-unreachable response through the vendored ipstack. This
   gives protected apps a prompt TCP fallback without proxying or leaking the
   rejected datagram. RFC-prohibited ICMP targets remain silent drops.
8. Payload aggregation and callback dispatch share one reporter lock, keeping
   cumulative TX/RX callbacks monotonic under concurrent relay activity while
   still reporting the first TX and first RX immediately. Registration is a
   completion barrier for the previous callback context. The session-counter
   reset is exported so an embedding can clear counters before atomically
   publishing a new RUNNING generation.

The fork also exposes aggregate, address-free session counters. The MASQ JNI
status reports them under `sessionMetrics` as `sessionCapacity`,
`activeSessions`, `peakSessions`, `rejectedCapacity`, `rejectedUdp`,
`rejectedIpv6`, and `rejectedNon443Tcp`.

When updating upstream, reapply the policy and permit logic, then run:

```sh
cargo test --manifest-path native/masq-packet-tunnel/Cargo.toml --locked
cargo test --manifest-path native/masq-packet-tunnel/vendor/tun2proxy/Cargo.toml --no-default-features
```
