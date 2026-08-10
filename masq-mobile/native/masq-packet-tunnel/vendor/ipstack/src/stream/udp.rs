// NOTICE: Modified by the MASQ Mobile community project in 2026.
// See vendor/ipstack/MASQ_PATCHES.md for the complete change description.

use crate::{
    IpStackError, PacketReceiver, PacketSender, TTL,
    packet::{IpHeader, NetworkPacket, TransportHeader},
};
use etherparse::{
    Icmpv4Header, Icmpv4Type, Icmpv6Header, Icmpv6Type, IpNumber, Ipv4Header, Ipv6FlowLabel, Ipv6Header, UdpHeader, icmpv4, icmpv6,
};
use std::{future::Future, net::SocketAddr, pin::Pin, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    time::Sleep,
};

/// A UDP stream in the IP stack.
///
/// This type represents a UDP connection and implements `AsyncRead` and `AsyncWrite`
/// for bidirectional data transfer. UDP streams have a configurable timeout and
/// automatically handle packet fragmentation based on MTU.
///
/// # Examples
///
/// ```no_run
/// use ipstack::{IpStack, IpStackConfig, IpStackStream};
/// use tokio::io::{AsyncReadExt, AsyncWriteExt};
///
/// # async fn example(mut ip_stack: IpStack) -> Result<(), Box<dyn std::error::Error>> {
/// if let IpStackStream::Udp(mut udp_stream) = ip_stack.accept().await? {
///     println!("New UDP stream from {}", udp_stream.peer_addr());
///
///     // Read data
///     let mut buffer = [0u8; 1024];
///     let n = udp_stream.read(&mut buffer).await?;
///
///     // Write data
///     udp_stream.write_all(b"Response").await?;
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct IpStackUdpStream {
    src_addr: SocketAddr,
    dst_addr: SocketAddr,
    stream_sender: PacketSender,
    stream_receiver: PacketReceiver,
    up_pkt_sender: PacketSender,
    first_payload: Option<Vec<u8>>,
    timeout: Pin<Box<Sleep>>,
    timeout_interval: Duration,
    mtu: u16,
    invoking_packet: Option<Vec<u8>>,
    destroy_messenger: Option<::tokio::sync::oneshot::Sender<()>>,
}

impl IpStackUdpStream {
    pub fn new(
        src_addr: SocketAddr,
        dst_addr: SocketAddr,
        payload: Vec<u8>,
        up_pkt_sender: PacketSender,
        mtu: u16,
        timeout_interval: Duration,
        destroy_messenger: Option<::tokio::sync::oneshot::Sender<()>>,
    ) -> Self {
        let (stream_sender, stream_receiver) = mpsc::unbounded_channel::<NetworkPacket>();
        let deadline = tokio::time::Instant::now() + timeout_interval;
        IpStackUdpStream {
            src_addr,
            dst_addr,
            stream_sender,
            stream_receiver,
            up_pkt_sender,
            first_payload: Some(payload),
            timeout: Box::pin(tokio::time::sleep_until(deadline)),
            timeout_interval,
            mtu,
            invoking_packet: None,
            destroy_messenger,
        }
    }

    /// Preserve the original packet for a protocol-correct ICMP error quote.
    ///
    /// The stack supplies this immediately after parsing and before handing the
    /// stream to an embedder. Keeping it separate from `first_payload` retains
    /// the original IP header and UDP tuple without changing the public
    /// constructor used by existing embedders.
    pub(crate) fn with_invoking_packet(mut self, invoking_packet: Vec<u8>) -> Self {
        self.invoking_packet = Some(invoking_packet);
        self
    }

    /// Reject this accepted UDP flow immediately with a destination/port-
    /// unreachable response directed back into the TUN device.
    ///
    /// This is a synchronous enqueue onto ipstack's unbounded upstream packet
    /// channel. It performs no network I/O and never waits for a peer timeout,
    /// so a rejected QUIC datagram cannot head-of-line block the accept loop.
    /// Consuming `self` also releases the session identity through `Drop`.
    pub fn abort_with_port_unreachable(self) -> std::io::Result<()> {
        // RFC 1122/1812 and RFC 4443 prohibit ICMP errors to invalid source
        // targets and in response to multicast/broadcast destinations. The
        // policy rejection remains fail-closed in these cases; it simply drops
        // the stream without injecting a prohibited response.
        if !self.may_emit_icmp_error() {
            return Ok(());
        }
        let packet = self.create_port_unreachable_packet()?;
        self.up_pkt_sender
            .send(packet)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::BrokenPipe, format!("send error: {error}")))
    }

    fn may_emit_icmp_error(&self) -> bool {
        match (self.src_addr.ip(), self.dst_addr.ip()) {
            (std::net::IpAddr::V4(source), std::net::IpAddr::V4(destination)) => {
                let invalid_common = |address: std::net::Ipv4Addr| {
                    address.is_unspecified() || address.is_multicast() || address == std::net::Ipv4Addr::BROADCAST
                };
                let invalid_source = source.is_loopback() || source.octets()[0] == 0 || source.octets()[0] >= 240;
                !invalid_common(source) && !invalid_source && !invalid_common(destination)
            }
            (std::net::IpAddr::V6(source), std::net::IpAddr::V6(destination)) => {
                let invalid = |address: std::net::Ipv6Addr| address.is_unspecified() || address.is_multicast();
                !invalid(source) && !source.is_loopback() && !invalid(destination)
            }
            _ => false,
        }
    }

    fn create_port_unreachable_packet(&self) -> std::io::Result<NetworkPacket> {
        let quote = self.bounded_invoking_packet_quote()?;
        match (self.dst_addr.ip(), self.src_addr.ip()) {
            (std::net::IpAddr::V4(dst), std::net::IpAddr::V4(src)) => {
                let icmp_header =
                    Icmpv4Header::with_checksum(Icmpv4Type::DestinationUnreachable(icmpv4::DestUnreachableHeader::Port), quote);
                let mut payload = Vec::with_capacity(icmp_header.header_len() + quote.len());
                payload.extend_from_slice(&icmp_header.to_bytes());
                payload.extend_from_slice(quote);
                let payload_length =
                    u16::try_from(payload.len()).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
                let ip_header = Ipv4Header::new(payload_length, TTL, IpNumber::ICMP, dst.octets(), src.octets())
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
                Ok(NetworkPacket {
                    ip: IpHeader::Ipv4(ip_header),
                    transport: TransportHeader::Unknown,
                    payload: Some(payload),
                })
            }
            (std::net::IpAddr::V6(dst), std::net::IpAddr::V6(src)) => {
                let icmp_header = Icmpv6Header::with_checksum(
                    Icmpv6Type::DestinationUnreachable(icmpv6::DestUnreachableCode::Port),
                    dst.octets(),
                    src.octets(),
                    quote,
                )
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
                let mut payload = Vec::with_capacity(icmp_header.header_len() + quote.len());
                payload.extend_from_slice(&icmp_header.to_bytes());
                payload.extend_from_slice(quote);
                let payload_length =
                    u16::try_from(payload.len()).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
                Ok(NetworkPacket {
                    ip: IpHeader::Ipv6(Ipv6Header {
                        traffic_class: 0,
                        flow_label: Ipv6FlowLabel::ZERO,
                        payload_length,
                        next_header: IpNumber::IPV6_ICMP,
                        hop_limit: TTL,
                        source: dst.octets(),
                        destination: src.octets(),
                    }),
                    transport: TransportHeader::Unknown,
                    payload: Some(payload),
                })
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "UDP source and destination IP versions differ",
            )),
        }
    }

    fn bounded_invoking_packet_quote(&self) -> std::io::Result<&[u8]> {
        const ICMP_HEADER_BYTES: usize = 8;
        const IPV4_HEADER_MIN_BYTES: usize = 20;
        const IPV6_HEADER_BYTES: usize = 40;
        const UDP_HEADER_BYTES: usize = 8;
        const IPV4_ERROR_DATAGRAM_MAX_BYTES: usize = 576;
        const IPV6_ERROR_DATAGRAM_MAX_BYTES: usize = 1_280;

        let packet = self.invoking_packet.as_deref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the original UDP packet is unavailable for an ICMP error quote",
            )
        })?;
        let version = packet.first().map(|byte| byte >> 4);
        let (minimum_quote_len, maximum_response_len, outer_ip_header_len) = match version {
            Some(4) => {
                let ip_header_len = usize::from(packet[0] & 0x0f) * 4;
                if ip_header_len < IPV4_HEADER_MIN_BYTES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "invalid IPv4 header length in UDP error quote",
                    ));
                }
                (
                    ip_header_len + UDP_HEADER_BYTES,
                    usize::from(self.mtu).min(IPV4_ERROR_DATAGRAM_MAX_BYTES),
                    IPV4_HEADER_MIN_BYTES,
                )
            }
            Some(6) => (
                IPV6_HEADER_BYTES + UDP_HEADER_BYTES,
                usize::from(self.mtu).min(IPV6_ERROR_DATAGRAM_MAX_BYTES),
                IPV6_HEADER_BYTES,
            ),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "invalid IP version in UDP error quote",
                ));
            }
        };
        let maximum_quote_len = maximum_response_len
            .checked_sub(outer_ip_header_len + ICMP_HEADER_BYTES)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "MTU is too small for an ICMP error"))?;
        if packet.len() < minimum_quote_len || maximum_quote_len < minimum_quote_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "UDP packet or MTU is too small for a protocol-correct ICMP error quote",
            ));
        }
        Ok(&packet[..packet.len().min(maximum_quote_len)])
    }

    pub(crate) fn stream_sender(&self) -> PacketSender {
        self.stream_sender.clone()
    }

    fn create_rev_packet(&self, ttl: u8, mut payload: Vec<u8>) -> std::io::Result<NetworkPacket> {
        const UHS: usize = 8; // udp header size is 8
        match (self.dst_addr.ip(), self.src_addr.ip()) {
            (std::net::IpAddr::V4(dst), std::net::IpAddr::V4(src)) => {
                let mut ip_h = Ipv4Header::new(0, ttl, IpNumber::UDP, dst.octets(), src.octets()).map_err(IpStackError::from)?;
                let line_buffer = self.mtu.saturating_sub((ip_h.header_len() + UHS) as u16);
                payload.truncate(line_buffer as usize);
                ip_h.set_payload_len(payload.len() + UHS).map_err(IpStackError::from)?;
                let udp_header = UdpHeader::with_ipv4_checksum(self.dst_addr.port(), self.src_addr.port(), &ip_h, &payload)
                    .map_err(IpStackError::from)?;
                Ok(NetworkPacket {
                    ip: IpHeader::Ipv4(ip_h),
                    transport: TransportHeader::Udp(udp_header),
                    payload: Some(payload),
                })
            }
            (std::net::IpAddr::V6(dst), std::net::IpAddr::V6(src)) => {
                let mut ip_h = Ipv6Header {
                    traffic_class: 0,
                    flow_label: Ipv6FlowLabel::ZERO,
                    payload_length: 0,
                    next_header: IpNumber::UDP,
                    hop_limit: ttl,
                    source: dst.octets(),
                    destination: src.octets(),
                };
                let line_buffer = self.mtu.saturating_sub((ip_h.header_len() + UHS) as u16);

                payload.truncate(line_buffer as usize);

                ip_h.payload_length = (payload.len() + UHS) as u16;
                let udp_header = UdpHeader::with_ipv6_checksum(self.dst_addr.port(), self.src_addr.port(), &ip_h, &payload)
                    .map_err(IpStackError::from)?;
                Ok(NetworkPacket {
                    ip: IpHeader::Ipv6(ip_h),
                    transport: TransportHeader::Udp(udp_header),
                    payload: Some(payload),
                })
            }
            _ => unreachable!(),
        }
    }

    /// Returns the local socket address of the UDP stream.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ipstack::IpStackUdpStream;
    /// # fn example(udp_stream: &IpStackUdpStream) {
    /// let local_addr = udp_stream.local_addr();
    /// println!("Local address: {}", local_addr);
    /// # }
    /// ```
    pub fn local_addr(&self) -> SocketAddr {
        self.src_addr
    }

    /// Returns the remote socket address of the UDP stream.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ipstack::IpStackUdpStream;
    /// # fn example(udp_stream: &IpStackUdpStream) {
    /// let peer_addr = udp_stream.peer_addr();
    /// println!("Peer address: {}", peer_addr);
    /// # }
    /// ```
    pub fn peer_addr(&self) -> SocketAddr {
        self.dst_addr
    }

    fn reset_timeout(&mut self) {
        let deadline = tokio::time::Instant::now() + self.timeout_interval;
        self.timeout.as_mut().reset(deadline);
    }
}

impl AsyncRead for IpStackUdpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if let Some(p) = self.first_payload.take() {
            // Clamp to `buf.remaining()`: an oversized datagram would otherwise
            // panic `put_slice` (hit under `copy_bidirectional` write
            // backpressure). Unlike the TCP path the remainder is DROPPED, not buffered matches `recvfrom` without MSG_WAITALL.
            let n = p.len().min(buf.remaining());
            buf.put_slice(&p[..n]);
            return std::task::Poll::Ready(Ok(()));
        }
        if matches!(self.timeout.as_mut().poll(cx), std::task::Poll::Ready(_)) {
            return std::task::Poll::Ready(Err(std::io::Error::from(std::io::ErrorKind::TimedOut)));
        }

        self.reset_timeout();

        match self.stream_receiver.poll_recv(cx) {
            std::task::Poll::Ready(Some(p)) => {
                if let Some(payload) = p.payload {
                    // Clamp like the first_payload branch above (drop the tail).
                    let n = payload.len().min(buf.remaining());
                    buf.put_slice(&payload[..n]);
                }
                std::task::Poll::Ready(Ok(()))
            }
            std::task::Poll::Ready(None) => std::task::Poll::Ready(Ok(())),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl AsyncWrite for IpStackUdpStream {
    fn poll_write(mut self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
        self.reset_timeout();
        let packet = self.create_rev_packet(TTL, buf.to_vec())?;
        let payload_len = packet.payload.as_ref().map(|p| p.len()).unwrap_or(0);
        self.up_pkt_sender.send(packet).or(Err(std::io::ErrorKind::UnexpectedEof))?;
        std::task::Poll::Ready(Ok(payload_len))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

impl Drop for IpStackUdpStream {
    fn drop(&mut self) {
        if let Some(messenger) = self.destroy_messenger.take() {
            let _ = messenger.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etherparse::{Ipv4HeaderSlice, Ipv6HeaderSlice, SlicedPacket, TransportSlice, UdpHeaderSlice};
    use tokio::io::AsyncReadExt;
    use tokio::sync::oneshot;

    fn stream(first_payload: Vec<u8>) -> IpStackUdpStream {
        let (up_tx, _up_rx) = mpsc::unbounded_channel();
        IpStackUdpStream::new(
            "127.0.0.1:1234".parse().unwrap(),
            "127.0.0.1:53".parse().unwrap(),
            first_payload,
            up_tx,
            1500,
            Duration::from_secs(30),
            None,
        )
    }

    // A datagram larger than the caller's buffer used to panic `put_slice`; it
    // must now truncate to the buffer instead. Covers both branches of poll_read.

    #[tokio::test]
    async fn poll_read_truncates_oversized_first_payload() {
        let mut s = stream(vec![7u8; 1250]);
        let mut small = [0u8; 502];
        assert_eq!(s.read(&mut small).await.unwrap(), 502);
        assert!(small.iter().all(|&b| b == 7));
    }

    #[tokio::test]
    async fn poll_read_truncates_oversized_relayed_datagram() {
        let mut s = stream(Vec::new());
        s.first_payload = None; // skip to the stream_receiver branch
        let pkt = s.create_rev_packet(64, vec![9u8; 1250]).unwrap();
        s.stream_sender().send(pkt).unwrap();
        let mut small = [0u8; 502];
        assert_eq!(s.read(&mut small).await.unwrap(), 502);
    }

    fn invoking_udp_packet(source: SocketAddr, destination: SocketAddr, payload: Vec<u8>) -> NetworkPacket {
        match (source.ip(), destination.ip()) {
            (std::net::IpAddr::V4(source_ip), std::net::IpAddr::V4(destination_ip)) => {
                let payload_length = u16::try_from(payload.len() + 8).expect("IPv4 payload length");
                let mut ip_header =
                    Ipv4Header::new(payload_length, 37, IpNumber::UDP, source_ip.octets(), destination_ip.octets()).expect("IPv4 header");
                ip_header.dont_fragment = true;
                let udp_header =
                    UdpHeader::with_ipv4_checksum(source.port(), destination.port(), &ip_header, &payload).expect("UDP/IPv4 checksum");
                NetworkPacket {
                    ip: IpHeader::Ipv4(ip_header),
                    transport: TransportHeader::Udp(udp_header),
                    payload: Some(payload),
                }
            }
            (std::net::IpAddr::V6(source_ip), std::net::IpAddr::V6(destination_ip)) => {
                let ip_header = Ipv6Header {
                    traffic_class: 17,
                    flow_label: Ipv6FlowLabel::ZERO,
                    payload_length: u16::try_from(payload.len() + 8).expect("IPv6 payload length"),
                    next_header: IpNumber::UDP,
                    hop_limit: 37,
                    source: source_ip.octets(),
                    destination: destination_ip.octets(),
                };
                let udp_header =
                    UdpHeader::with_ipv6_checksum(source.port(), destination.port(), &ip_header, &payload).expect("UDP/IPv6 checksum");
                NetworkPacket {
                    ip: IpHeader::Ipv6(ip_header),
                    transport: TransportHeader::Udp(udp_header),
                    payload: Some(payload),
                }
            }
            _ => panic!("test packet IP versions must match"),
        }
    }

    fn rejected_stream(
        invoking_packet: &NetworkPacket,
        mtu: u16,
    ) -> (IpStackUdpStream, mpsc::UnboundedReceiver<NetworkPacket>, oneshot::Receiver<()>) {
        let source = invoking_packet.src_addr();
        let destination = invoking_packet.dst_addr();
        let payload = invoking_packet.payload.clone().unwrap_or_default();
        let invoking_packet_bytes = invoking_packet.to_bytes().expect("invoking packet bytes");
        let (packet_sender, packet_receiver) = mpsc::unbounded_channel();
        let (destroy_sender, destroy_receiver) = oneshot::channel();
        let stream = IpStackUdpStream::new(
            source,
            destination,
            payload,
            packet_sender,
            mtu,
            Duration::from_secs(30),
            Some(destroy_sender),
        )
        .with_invoking_packet(invoking_packet_bytes);
        (stream, packet_receiver, destroy_receiver)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn abort_with_port_unreachable_emits_bounded_valid_icmpv4_and_releases_session() {
        let source: SocketAddr = "10.0.0.2:41000".parse().expect("source address");
        let destination: SocketAddr = "198.18.0.1:443".parse().expect("destination address");
        let invoking_packet = invoking_udp_packet(source, destination, vec![0xa5; 1_000]);
        let invoking_bytes = invoking_packet.to_bytes().expect("invoking packet bytes");
        let (stream, mut packet_receiver, destroy_receiver) = rejected_stream(&invoking_packet, 1_500);

        stream.abort_with_port_unreachable().expect("ICMPv4 rejection");

        let response = tokio::time::timeout(Duration::from_millis(250), packet_receiver.recv())
            .await
            .expect("ICMPv4 observation must be bounded")
            .expect("ICMPv4 response packet");
        let response_bytes = response.to_bytes().expect("response bytes");
        assert_eq!(response.src_addr().ip(), destination.ip());
        assert_eq!(response.dst_addr().ip(), source.ip());
        assert_eq!(response_bytes.len(), 576, "ICMPv4 errors must stay within the RFC 1812 bound");

        let sliced = SlicedPacket::from_ip(&response_bytes).expect("parse ICMPv4 response");
        let icmp = match sliced.transport.expect("ICMPv4 transport") {
            TransportSlice::Icmpv4(icmp) => icmp,
            other => panic!("expected ICMPv4 response, got {other:?}"),
        };
        assert_eq!(
            icmp.icmp_type(),
            Icmpv4Type::DestinationUnreachable(icmpv4::DestUnreachableHeader::Port),
        );
        let expected_header = Icmpv4Header::with_checksum(icmp.icmp_type(), icmp.payload());
        assert_eq!(icmp.checksum(), expected_header.checksum, "ICMPv4 checksum");
        assert_eq!(
            icmp.payload(),
            &invoking_bytes[..548],
            "the ICMP quote must preserve the invoking tuple"
        );

        let quoted_ip = Ipv4HeaderSlice::from_slice(icmp.payload()).expect("quoted IPv4 header");
        assert_eq!(quoted_ip.source_addr(), source.ip());
        assert_eq!(quoted_ip.destination_addr(), destination.ip());
        let quoted_udp = UdpHeaderSlice::from_slice(&icmp.payload()[quoted_ip.slice().len()..]).expect("quoted UDP header");
        assert_eq!(quoted_udp.source_port(), source.port());
        assert_eq!(quoted_udp.destination_port(), destination.port());
        tokio::time::timeout(Duration::from_millis(250), destroy_receiver)
            .await
            .expect("the rejected IPv4 session must be released promptly")
            .expect("destroy signal");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn abort_with_port_unreachable_emits_bounded_valid_icmpv6_and_releases_session() {
        let source: SocketAddr = "[fd00::2]:41001".parse().expect("source address");
        let destination: SocketAddr = "[2001:db8::1]:443".parse().expect("destination address");
        let invoking_packet = invoking_udp_packet(source, destination, vec![0x5a; 1_500]);
        let invoking_bytes = invoking_packet.to_bytes().expect("invoking packet bytes");
        let (stream, mut packet_receiver, destroy_receiver) = rejected_stream(&invoking_packet, 1_500);

        stream.abort_with_port_unreachable().expect("ICMPv6 rejection");

        let response = tokio::time::timeout(Duration::from_millis(250), packet_receiver.recv())
            .await
            .expect("ICMPv6 observation must be bounded")
            .expect("ICMPv6 response packet");
        let response_bytes = response.to_bytes().expect("response bytes");
        assert_eq!(response.src_addr().ip(), destination.ip());
        assert_eq!(response.dst_addr().ip(), source.ip());
        assert_eq!(response_bytes.len(), 1_280, "ICMPv6 errors must not exceed the minimum IPv6 MTU");

        let sliced = SlicedPacket::from_ip(&response_bytes).expect("parse ICMPv6 response");
        let icmp = match sliced.transport.expect("ICMPv6 transport") {
            TransportSlice::Icmpv6(icmp) => icmp,
            other => panic!("expected ICMPv6 response, got {other:?}"),
        };
        assert_eq!(
            icmp.icmp_type(),
            Icmpv6Type::DestinationUnreachable(icmpv6::DestUnreachableCode::Port),
        );
        let outer_source = match destination.ip() {
            std::net::IpAddr::V6(ip) => ip.octets(),
            _ => unreachable!(),
        };
        let outer_destination = match source.ip() {
            std::net::IpAddr::V6(ip) => ip.octets(),
            _ => unreachable!(),
        };
        assert!(icmp.is_checksum_valid(outer_source, outer_destination), "ICMPv6 checksum");
        assert_eq!(
            icmp.payload(),
            &invoking_bytes[..1_232],
            "the ICMPv6 quote must preserve the invoking tuple"
        );

        let quoted_ip = Ipv6HeaderSlice::from_slice(icmp.payload()).expect("quoted IPv6 header");
        assert_eq!(quoted_ip.source_addr(), source.ip());
        assert_eq!(quoted_ip.destination_addr(), destination.ip());
        let quoted_udp = UdpHeaderSlice::from_slice(&icmp.payload()[quoted_ip.slice().len()..]).expect("quoted UDP header");
        assert_eq!(quoted_udp.source_port(), source.port());
        assert_eq!(quoted_udp.destination_port(), destination.port());
        tokio::time::timeout(Duration::from_millis(250), destroy_receiver)
            .await
            .expect("the rejected IPv6 session must be released promptly")
            .expect("destroy signal");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn abort_suppresses_icmp_errors_for_invalid_ipv4_targets_and_multicast_or_broadcast() {
        let cases = [
            ("0.0.0.0:41002", "198.18.0.1:443"),
            ("0.1.2.3:41002", "198.18.0.1:443"),
            ("127.0.0.1:41002", "198.18.0.1:443"),
            ("224.0.0.1:41002", "198.18.0.1:443"),
            ("240.0.0.1:41002", "198.18.0.1:443"),
            ("255.255.255.255:41002", "198.18.0.1:443"),
            ("10.0.0.2:41002", "0.0.0.0:443"),
            ("10.0.0.2:41002", "224.0.0.1:443"),
            ("10.0.0.2:41002", "255.255.255.255:443"),
        ];

        for (source, destination) in cases {
            let invoking_packet = invoking_udp_packet(
                source.parse().expect("source address"),
                destination.parse().expect("destination address"),
                vec![0x33; 32],
            );
            let (stream, mut packet_receiver, destroy_receiver) = rejected_stream(&invoking_packet, 1_500);
            stream.abort_with_port_unreachable().expect("silent fail-closed rejection");
            assert!(
                packet_receiver.recv().await.is_none(),
                "a prohibited ICMPv4 error must not be emitted"
            );
            tokio::time::timeout(Duration::from_millis(250), destroy_receiver)
                .await
                .expect("the silently rejected IPv4 session must be released promptly")
                .expect("destroy signal");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn abort_suppresses_icmpv6_errors_for_invalid_targets_and_multicast() {
        let cases = [
            ("[::]:41003", "[2001:db8::1]:443"),
            ("[::1]:41003", "[2001:db8::1]:443"),
            ("[ff02::1]:41003", "[2001:db8::1]:443"),
            ("[fd00::2]:41003", "[::]:443"),
            ("[fd00::2]:41003", "[ff02::1]:443"),
        ];

        for (source, destination) in cases {
            let invoking_packet = invoking_udp_packet(
                source.parse().expect("source address"),
                destination.parse().expect("destination address"),
                vec![0x44; 32],
            );
            let (stream, mut packet_receiver, destroy_receiver) = rejected_stream(&invoking_packet, 1_500);
            stream.abort_with_port_unreachable().expect("silent fail-closed rejection");
            assert!(
                packet_receiver.recv().await.is_none(),
                "a prohibited ICMPv6 error must not be emitted"
            );
            tokio::time::timeout(Duration::from_millis(250), destroy_receiver)
                .await
                .expect("the silently rejected IPv6 session must be released promptly")
                .expect("destroy signal");
        }
    }
}
