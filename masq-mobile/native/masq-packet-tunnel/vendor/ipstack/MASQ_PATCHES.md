# MASQ local ipstack patch

This directory vendors `ipstack` 1.0.1 from crates.io (Apache-2.0, crate
SHA-256 `e889a45c1ce3e97268ad2249951528563ec1d02a120fecbd4db690beac34f7fd`).
MASQ keeps it local for one Android fail-closed lifecycle change:

- `IpStackTcpStream::abort_with_reset` consumes a policy- or
  capacity-rejected stream, emits RST/ACK, marks its TCB closed, wakes pending
  I/O, and aborts its private worker without waiting for a FIN exchange.
- Dropping a still-live TCP stream uses the same bounded reset/abort path. It
  never synchronously joins a private task from a Tokio runtime worker, which
  avoids a many-session cancellation deadlock during tunnel stop or rebind.
- `IpStackUdpStream::abort_with_port_unreachable` consumes a policy-rejected
  UDP stream and synchronously enqueues a valid ICMPv4 destination/port-
  unreachable or ICMPv6 destination/port-unreachable response into the TUN.
  Its quote preserves the original IP header and UDP tuple, is bounded to 576
  bytes for IPv4 or the 1280-byte minimum IPv6 MTU, and releases the session
  identity without waiting for UDP/QUIC retransmission timeouts. ICMP errors
  prohibited by RFC 1122/1812/4443 (invalid/loopback error targets,
  unspecified or multicast addresses, and IPv4 limited-broadcast
  source/destination) remain a silent fail-closed drop.

`IpStack::accept()` has already accepted a SYN when it returns a TCP stream.
The upstream graceful `shutdown().await` can remain pending in `SYN_RECEIVED`
and block the single TUN accept loop, which prevents a later compatible
TCP/443 fallback from being processed and can also delay tunnel cancellation.
The reset and ICMP rejection paths are bounded, fail-closed, and do not create
one detached task per rejected connection.

The published crate's example/benchmark-only development dependencies and its
Criterion-only benchmark shim are omitted so the security regression suite
remains fully reproducible from the locked application dependency set. Rust
doctests are disabled because the upstream examples import those omitted
example-only crates; the runtime library and its complete unit-test suite stay
enabled.

When updating upstream, reapply the consuming reset API and run the ipstack,
tun2proxy, and `masq-packet-tunnel` test suites.
