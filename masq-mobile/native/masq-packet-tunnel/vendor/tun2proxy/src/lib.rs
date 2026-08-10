#[cfg(target_os = "linux")]
extern crate bincode_next as bincode;

#[cfg(feature = "udpgw")]
use crate::udpgw::UdpGwClient;
use crate::{
    directions::{IncomingDataEvent, IncomingDirection, OutgoingDirection},
    http::HttpManager,
    no_proxy::NoProxyManager,
    session_info::{IpProtocol, SessionInfo},
    session_metrics::{SessionPermit, SessionRejection, record_session_rejection},
    virtual_dns::VirtualDns,
};
pub use clap::ValueEnum;
use ipstack::{IpStackStream, IpStackTcpStream, IpStackUdpStream};
use proxy_handler::{ProxyHandler, ProxyHandlerManager};
use socks::SocksProxyManager;
pub use socks5_impl::protocol::UserKey;
#[cfg(feature = "udpgw")]
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};
use std::{
    collections::VecDeque,
    future::Future,
    io::ErrorKind,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpSocket, TcpStream, UdpSocket},
    sync::{Mutex, mpsc::Receiver, watch},
};
pub use tokio_util::sync::CancellationToken;
use tproxy_config::is_private_ip;
pub use tun::DEFAULT_MTU;
use udp_stream::UdpStream;
#[cfg(feature = "udpgw")]
use udpgw::{UDPGW_KEEPALIVE_TIME, UDPGW_MAX_CONNECTIONS, UdpGwClientStream, UdpGwResponse};

pub use {
    args::{ArgDns, ArgProxy, ArgVerbosity, Args, ProxyType, SessionPolicy},
    error::{BoxError, Error, Result},
    session_metrics::{SessionMetrics, reset_session_metrics, session_metrics_snapshot},
    traffic_status::{TrafficStatus, tun2proxy_set_traffic_status_callback},
};

pub use general_api::general_run_async;

pub const FORCE_EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

mod android;
mod args;
mod directions;
mod dns;
mod dump_logger;
mod error;
mod general_api;
mod http;
mod no_proxy;
mod proxy_handler;
mod session_info;
mod session_metrics;
pub mod socket_transfer;
mod socks;
mod traffic_status;
#[cfg(feature = "udpgw")]
pub mod udpgw;
mod virtual_dns;
#[doc(hidden)]
pub mod win_svc;

const DNS_PORT: u16 = 53;
const IPV4_HEADER_BYTES: u16 = 20;
const TCP_HEADER_BYTES: u16 = 20;

fn tcp_config_for_tun_mtu(mtu: u16, timeout_secs: u64, advertise_ipv4_mss: bool) -> ipstack::TcpConfig {
    let mut config = ipstack::TcpConfig::default();
    config.timeout = Duration::from_secs(timeout_secs);

    // The MASQ Android translator currently admits only IPv4 TCP streams.
    // Advertising the payload budget in the SYN/ACK keeps the protected app
    // from sending an MTU-sized TCP payload plus IP/TCP headers into the TUN.
    // Invalid sub-header MTUs retain ipstack's default option behavior and are
    // rejected by the surrounding tunnel configuration path.
    if advertise_ipv4_mss {
        if let Some(mss) = mtu.checked_sub(IPV4_HEADER_BYTES + TCP_HEADER_BYTES).filter(|mss| *mss > 0) {
            config
                .options
                .get_or_insert_with(Vec::new)
                .push(ipstack::TcpOptions::MaximumSegmentSize(mss));
        }
    }
    config
}

fn pre_accounting_rejection(args: &Args, info: &SessionInfo) -> Option<SessionRejection> {
    if args.session_policy.reject_ipv6 && (info.src.is_ipv6() || info.dst.is_ipv6()) {
        return Some(SessionRejection::Ipv6);
    }
    match info.protocol {
        IpProtocol::Udp
            if args.session_policy.reject_udp_except_virtual_dns && !(info.dst.port() == DNS_PORT && args.dns == ArgDns::Virtual) =>
        {
            Some(SessionRejection::Udp)
        }
        IpProtocol::Tcp
            if !args.session_policy.allowed_tcp_ports.is_empty() && !args.session_policy.allowed_tcp_ports.contains(&info.dst.port()) =>
        {
            Some(SessionRejection::TcpPort)
        }
        _ => None,
    }
}

#[allow(unused)]
#[derive(Hash, Copy, Clone, Eq, PartialEq, Debug)]
#[cfg_attr(
    target_os = "linux",
    derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)
)]
pub enum SocketProtocol {
    Tcp,
    Udp,
}

#[allow(unused)]
#[derive(Hash, Copy, Clone, Eq, PartialEq, Debug)]
#[cfg_attr(
    target_os = "linux",
    derive(bincode::Encode, bincode::Decode, serde::Serialize, serde::Deserialize)
)]
pub enum SocketDomain {
    IpV4,
    IpV6,
}

impl From<IpAddr> for SocketDomain {
    fn from(value: IpAddr) -> Self {
        match value {
            IpAddr::V4(_) => Self::IpV4,
            IpAddr::V6(_) => Self::IpV6,
        }
    }
}

struct SocketQueue {
    tcp_v4: Mutex<Receiver<TcpSocket>>,
    tcp_v6: Mutex<Receiver<TcpSocket>>,
    udp_v4: Mutex<Receiver<UdpSocket>>,
    udp_v6: Mutex<Receiver<UdpSocket>>,
}

impl SocketQueue {
    async fn recv_tcp(&self, domain: SocketDomain) -> Result<TcpSocket, std::io::Error> {
        match domain {
            SocketDomain::IpV4 => &self.tcp_v4,
            SocketDomain::IpV6 => &self.tcp_v6,
        }
        .lock()
        .await
        .recv()
        .await
        .ok_or(ErrorKind::Other.into())
    }
    async fn recv_udp(&self, domain: SocketDomain) -> Result<UdpSocket, std::io::Error> {
        match domain {
            SocketDomain::IpV4 => &self.udp_v4,
            SocketDomain::IpV6 => &self.udp_v6,
        }
        .lock()
        .await
        .recv()
        .await
        .ok_or(ErrorKind::Other.into())
    }
}

async fn create_tcp_stream(socket_queue: &Option<Arc<SocketQueue>>, peer: SocketAddr) -> std::io::Result<TcpStream> {
    match &socket_queue {
        None => TcpStream::connect(peer).await,
        Some(queue) => queue.recv_tcp(peer.ip().into()).await?.connect(peer).await,
    }
}

async fn create_udp_stream(socket_queue: &Option<Arc<SocketQueue>>, peer: SocketAddr) -> std::io::Result<UdpStream> {
    match &socket_queue {
        None => {
            let bind_addr = match peer {
                SocketAddr::V4(_) => SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 0)),
                SocketAddr::V6(_) => SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0)),
            };
            let socket = UdpSocket::bind(bind_addr).await?;
            socket.connect(peer).await?;
            UdpStream::from_tokio(socket, peer).await
        }
        Some(queue) => {
            let socket = queue.recv_udp(peer.ip().into()).await?;
            socket.connect(peer).await?;
            UdpStream::from_tokio(socket, peer).await
        }
    }
}

/// Run the proxy server
/// # Arguments
/// * `device` - The network device to use
/// * `mtu` - The MTU of the network device
/// * `args` - The arguments to use
/// * `shutdown_token` - The token to exit the server
/// # Returns
/// * The number of sessions while exiting
pub async fn run<D>(device: D, mtu: u16, args: Args, shutdown_token: CancellationToken) -> crate::Result<usize>
where
    D: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    log::info!("{} {} starting...", env!("CARGO_PKG_NAME"), version_info!());
    log::info!("Proxy {} server: {}", args.proxy.proxy_type, args.proxy.addr);
    reset_session_metrics(args.max_sessions);

    let server_addr = args.proxy.addr;
    let key = args.proxy.credentials.clone();
    let proxy_handshake_timeout = args.session_policy.proxy_handshake_timeout;
    let tcp_idle_timeout = args.session_policy.tcp_idle_timeout;
    let dns_addr = args.dns_addr;
    let ipv6_enabled = args.ipv6_enabled;
    let virtual_dns = if args.dns == ArgDns::Virtual {
        Some(Arc::new(Mutex::new(VirtualDns::new(args.virtual_dns_pool))))
    } else {
        None
    };

    #[cfg(target_os = "linux")]
    let socket_queue = match args.socket_transfer_fd {
        None => None,
        Some(fd) => {
            use crate::socket_transfer::{reconstruct_socket, reconstruct_transfer_socket, request_sockets};
            use tokio::sync::mpsc::channel;

            let fd = reconstruct_socket(fd)?;
            let socket = reconstruct_transfer_socket(fd)?;
            let socket = Arc::new(Mutex::new(socket));

            macro_rules! create_socket_queue {
                ($domain:ident) => {{
                    const SOCKETS_PER_REQUEST: usize = 64;

                    let socket = socket.clone();
                    let (tx, rx) = channel(SOCKETS_PER_REQUEST);
                    tokio::spawn(async move {
                        loop {
                            let sockets =
                                match request_sockets(socket.lock().await, SocketDomain::$domain, SOCKETS_PER_REQUEST as u32).await {
                                    Ok(sockets) => sockets,
                                    Err(err) => {
                                        log::warn!("Socket allocation request failed: {err}");
                                        continue;
                                    }
                                };
                            for s in sockets {
                                if let Err(_) = tx.send(s).await {
                                    return;
                                }
                            }
                        }
                    });
                    Mutex::new(rx)
                }};
            }

            Some(Arc::new(SocketQueue {
                tcp_v4: create_socket_queue!(IpV4),
                tcp_v6: create_socket_queue!(IpV6),
                udp_v4: create_socket_queue!(IpV4),
                udp_v6: create_socket_queue!(IpV6),
            }))
        }
    };

    #[cfg(not(target_os = "linux"))]
    let socket_queue = None;

    use socks5_impl::protocol::Version::{V4, V5};
    let mgr: Arc<dyn ProxyHandlerManager> = match args.proxy.proxy_type {
        ProxyType::Socks5 => Arc::new(SocksProxyManager::new(server_addr, V5, key)),
        ProxyType::Socks4 => Arc::new(SocksProxyManager::new(server_addr, V4, key)),
        ProxyType::Http => Arc::new(HttpManager::new(server_addr, key)),
        ProxyType::None => Arc::new(NoProxyManager::new()),
    };

    let mut ipstack_config = ipstack::IpStackConfig::default();
    ipstack_config.mtu(mtu)?;
    let tcp_cfg = tcp_config_for_tun_mtu(mtu, args.tcp_timeout, args.session_policy.reject_ipv6);
    ipstack_config.with_tcp_config(tcp_cfg);
    ipstack_config.udp_timeout(std::time::Duration::from_secs(args.udp_timeout));

    let mut ip_stack = ipstack::IpStack::new(ipstack_config, device);

    #[cfg(feature = "udpgw")]
    let udpgw_client = args.udpgw_server.map(|addr| {
        log::info!("UDP Gateway enabled, server: {addr}");
        use std::time::Duration;
        let client = Arc::new(UdpGwClient::new(
            mtu,
            args.udpgw_connections.unwrap_or(UDPGW_MAX_CONNECTIONS),
            args.udpgw_keepalive.map(Duration::from_secs).unwrap_or(UDPGW_KEEPALIVE_TIME),
            args.udp_timeout,
            addr,
        ));
        let client_keepalive = client.clone();
        let shutdown_clone = shutdown_token.clone();
        tokio::spawn(async move {
            if let Err(err) = client_keepalive.heartbeat_task(shutdown_clone).await {
                log::error!("UDP Gateway heartbeat task error: {err}");
            }
        });
        client
    });

    loop {
        let virtual_dns = virtual_dns.clone();
        let ip_stack_stream = tokio::select! {
            _ = shutdown_token.cancelled() => {
                log::info!("Shutdown received");
                break;
            }
            ip_stack_stream = ip_stack.accept() => {
                ip_stack_stream?
            }
        };
        let max_sessions = args.max_sessions;
        match ip_stack_stream {
            IpStackStream::Tcp(tcp) => {
                let info = SessionInfo::new(tcp.local_addr(), tcp.peer_addr(), IpProtocol::Tcp);
                if let Some(rejection) = pre_accounting_rejection(&args, &info) {
                    record_session_rejection(rejection);
                    log::debug!("Rejecting unsupported {rejection:?} flow before session accounting");
                    if let Err(error) = tcp.abort_with_reset() {
                        log::debug!("Rejected TCP flow reset error: {error}");
                    }
                    continue;
                }
                let Some(session_permit) = SessionPermit::try_acquire(max_sessions) else {
                    if args.exit_on_fatal_error {
                        log::info!("Too many sessions that over {max_sessions}, exiting...");
                        break;
                    }
                    log::warn!("Too many sessions that over {max_sessions}, dropping new session");
                    if let Err(error) = tcp.abort_with_reset() {
                        log::debug!("Capacity-rejected TCP flow reset error: {error}");
                    }
                    continue;
                };
                let domain_name = if let Some(virtual_dns) = &virtual_dns {
                    let mut virtual_dns = virtual_dns.lock().await;
                    virtual_dns.touch_ip(&tcp.peer_addr().ip());
                    virtual_dns.resolve_ip(&tcp.peer_addr().ip())
                } else {
                    None
                };
                let proxy_handler = match mgr.new_proxy_handler(info, domain_name, false).await {
                    Ok(proxy_handler) => proxy_handler,
                    Err(error) => {
                        log::error!("Failed to create TCP proxy handler: {error}");
                        continue;
                    }
                };
                let socket_queue = socket_queue.clone();
                tokio::spawn(async move {
                    let _session_permit = session_permit;
                    if let Err(err) = handle_tcp_session(tcp, proxy_handler, socket_queue, proxy_handshake_timeout, tcp_idle_timeout).await
                    {
                        log::error!("{info} error \"{err}\"");
                    }
                });
            }
            IpStackStream::Udp(udp) => {
                let mut info = SessionInfo::new(udp.local_addr(), udp.peer_addr(), IpProtocol::Udp);
                if let Some(rejection) = pre_accounting_rejection(&args, &info) {
                    record_session_rejection(rejection);
                    log::debug!("Rejecting unsupported {rejection:?} flow before session accounting");
                    if let Err(error) = udp.abort_with_port_unreachable() {
                        log::debug!("Rejected UDP flow ICMP error: {error}");
                    }
                    continue;
                }
                let Some(session_permit) = SessionPermit::try_acquire(max_sessions) else {
                    if args.exit_on_fatal_error {
                        log::info!("Too many sessions that over {max_sessions}, exiting...");
                        break;
                    }
                    log::warn!("Too many sessions that over {max_sessions}, dropping new session");
                    continue;
                };
                if info.dst.port() == DNS_PORT {
                    if is_private_ip(info.dst.ip()) {
                        info.dst.set_ip(dns_addr); // !!! Here we change the destination address to remote DNS server!!!
                    }
                    if args.dns == ArgDns::OverTcp {
                        info.protocol = IpProtocol::Tcp;
                        let proxy_handler = match mgr.new_proxy_handler(info, None, false).await {
                            Ok(proxy_handler) => proxy_handler,
                            Err(error) => {
                                log::error!("Failed to create DNS-over-TCP proxy handler: {error}");
                                continue;
                            }
                        };
                        let socket_queue = socket_queue.clone();
                        tokio::spawn(async move {
                            let _session_permit = session_permit;
                            if let Err(err) = handle_dns_over_tcp_session(udp, proxy_handler, socket_queue, ipv6_enabled).await {
                                log::error!("{info} error \"{err}\"");
                            }
                        });
                        continue;
                    }
                    if args.dns == ArgDns::Virtual {
                        tokio::spawn(async move {
                            let _session_permit = session_permit;
                            if let Some(virtual_dns) = virtual_dns {
                                if let Err(err) = handle_virtual_dns_session(udp, virtual_dns).await {
                                    log::error!("{info} error \"{err}\"");
                                }
                            }
                        });
                        continue;
                    }
                    assert_eq!(args.dns, ArgDns::Direct);
                }
                let domain_name = if let Some(virtual_dns) = &virtual_dns {
                    let mut virtual_dns = virtual_dns.lock().await;
                    virtual_dns.touch_ip(&udp.peer_addr().ip());
                    virtual_dns.resolve_ip(&udp.peer_addr().ip())
                } else {
                    None
                };
                #[cfg(feature = "udpgw")]
                if let Some(udpgw) = udpgw_client.clone() {
                    let tcp_src = match udp.peer_addr() {
                        SocketAddr::V4(_) => SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)),
                        SocketAddr::V6(_) => SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0)),
                    };
                    let tcpinfo = SessionInfo::new(tcp_src, udpgw.get_udpgw_server_addr(), IpProtocol::Tcp);
                    let proxy_handler = match mgr.new_proxy_handler(tcpinfo, None, false).await {
                        Ok(proxy_handler) => proxy_handler,
                        Err(error) => {
                            log::error!("Failed to create UDP gateway proxy handler: {error}");
                            continue;
                        }
                    };
                    let queue = socket_queue.clone();
                    tokio::spawn(async move {
                        let _session_permit = session_permit;
                        let dst = info.dst; // real UDP destination address
                        let dst_addr = match domain_name {
                            Some(ref d) => socks5_impl::protocol::Address::from((d.clone(), dst.port())),
                            None => dst.into(),
                        };
                        if let Err(e) = handle_udp_gateway_session(udp, udpgw, &dst_addr, proxy_handler, queue, ipv6_enabled).await {
                            log::info!("Ending {info} with \"{e}\"");
                        }
                    });
                    continue;
                }
                match mgr.new_proxy_handler(info, domain_name, true).await {
                    Ok(proxy_handler) => {
                        let socket_queue = socket_queue.clone();
                        tokio::spawn(async move {
                            let _session_permit = session_permit;
                            let ty = args.proxy.proxy_type;
                            if let Err(err) = handle_udp_associate_session(udp, ty, proxy_handler, socket_queue, ipv6_enabled).await {
                                log::info!("Ending {info} with \"{err}\"");
                            }
                        });
                    }
                    Err(e) => {
                        log::error!("Failed to create UDP connection: {e}");
                    }
                }
            }
            IpStackStream::UnknownTransport(u) => {
                let len = u.payload().len();
                log::info!("#0 unhandled transport - Ip Protocol {:?}, length {}", u.ip_protocol(), len);
                continue;
            }
            IpStackStream::UnknownNetwork(pkt) => {
                log::info!("#0 unknown transport - {} bytes", pkt.len());
                continue;
            }
        }
    }
    Ok(session_metrics_snapshot().active_sessions as usize)
}

async fn handle_virtual_dns_session(mut udp: IpStackUdpStream, dns: Arc<Mutex<VirtualDns>>) -> crate::Result<()> {
    let mut buf = [0_u8; 4096];
    loop {
        let len = match udp.read(&mut buf).await {
            Err(e) => {
                // indicate UDP read fails not an error.
                log::debug!("Virtual DNS session error: {e}");
                break;
            }
            Ok(len) => len,
        };
        if len == 0 {
            break;
        }
        let msg = dns.lock().await.generate_query(&buf[..len])?;
        udp.write_all(&msg).await?;
    }
    Ok(())
}

async fn copy_and_record_traffic<R, W>(reader: &mut R, writer: &mut W, is_tx: bool, activity: watch::Sender<u64>) -> tokio::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin + ?Sized,
    W: tokio::io::AsyncWrite + Unpin + ?Sized,
{
    let mut buf = vec![0; 8192];
    let mut total = 0;
    loop {
        match reader.read(&mut buf).await? {
            0 => break, // EOF
            n => {
                total += n as u64;
                writer.write_all(&buf[..n]).await?;
                let (tx, rx) = if is_tx { (n, 0) } else { (0, n) };
                if let Err(e) = crate::traffic_status::traffic_status_update(tx, rx) {
                    log::debug!("Record traffic status error: {e}");
                }
                activity.send_modify(|generation| *generation = generation.wrapping_add(1));
            }
        }
    }
    Ok(total)
}

fn timed_out_error(message: &'static str) -> crate::Error {
    std::io::Error::new(ErrorKind::TimedOut, message).into()
}

async fn with_proxy_handshake_timeout<F, T>(future: F, timeout: Option<Duration>) -> crate::Result<T>
where
    F: Future<Output = crate::Result<T>>,
{
    match timeout {
        Some(timeout) => tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| timed_out_error("proxy handshake timed out"))?,
        None => future.await,
    }
}

/// Relay both halves concurrently while one shared inactivity deadline tracks
/// successful payload delivery in either direction. A clean half-close keeps
/// the reverse half alive (important for request-then-response protocols), but
/// a fully half-open connection cannot retain capacity forever.
async fn relay_tcp_with_idle_timeout<C, S>(client: C, server: S, idle_timeout: Option<Duration>) -> tokio::io::Result<(u64, u64)>
where
    C: AsyncRead + AsyncWrite + Unpin,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut client_rx, mut client_tx) = tokio::io::split(client);
    let (mut server_rx, mut server_tx) = tokio::io::split(server);
    let (activity_tx, mut activity_rx) = watch::channel(0_u64);

    let to_server_activity = activity_tx.clone();
    let mut to_server = Box::pin(async move {
        let result = copy_and_record_traffic(&mut client_rx, &mut server_tx, true, to_server_activity).await;
        let shutdown = server_tx.shutdown().await;
        let bytes = result?;
        shutdown?;
        Ok::<u64, std::io::Error>(bytes)
    });
    let mut to_client = Box::pin(async move {
        let result = copy_and_record_traffic(&mut server_rx, &mut client_tx, false, activity_tx).await;
        let shutdown = client_tx.shutdown().await;
        let bytes = result?;
        shutdown?;
        Ok::<u64, std::io::Error>(bytes)
    });

    let timeout_enabled = idle_timeout.is_some();
    let timeout = idle_timeout.unwrap_or(Duration::from_secs(1));
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut to_server_result = None;
    let mut to_client_result = None;
    let mut activity_open = true;

    loop {
        tokio::select! {
            result = &mut to_server, if to_server_result.is_none() => {
                to_server_result = Some(result?);
            }
            result = &mut to_client, if to_client_result.is_none() => {
                to_client_result = Some(result?);
            }
            changed = activity_rx.changed(), if timeout_enabled && activity_open => {
                if changed.is_ok() {
                    deadline.as_mut().reset(tokio::time::Instant::now() + timeout);
                } else {
                    activity_open = false;
                }
            }
            _ = &mut deadline, if timeout_enabled => {
                return Err(std::io::Error::new(
                    ErrorKind::TimedOut,
                    "established proxy relay idle timeout",
                ));
            }
        }

        if let (Some(to_server), Some(to_client)) = (to_server_result, to_client_result) {
            return Ok((to_server, to_client));
        }
    }
}

async fn handle_tcp_session(
    tcp_stack: IpStackTcpStream,
    proxy_handler: Arc<Mutex<dyn ProxyHandler>>,
    socket_queue: Option<Arc<SocketQueue>>,
    proxy_handshake_timeout: Option<Duration>,
    tcp_idle_timeout: Option<Duration>,
) -> crate::Result<()> {
    let (session_info, server_addr) = {
        let handler = proxy_handler.lock().await;

        (handler.get_session_info(), handler.get_server_addr())
    };

    log::info!("Beginning {session_info}");

    let proxy_setup = async {
        let mut server = create_tcp_stream(&socket_queue, server_addr).await?;
        handle_proxy_session(&mut server, proxy_handler).await?;
        Ok(server)
    };
    let server = match with_proxy_handshake_timeout(proxy_setup, proxy_handshake_timeout).await {
        Ok(server) => server,
        Err(error) => {
            if let Err(reset_error) = tcp_stack.abort_with_reset() {
                log::trace!("{session_info} client reset after proxy setup failure: {reset_error}");
            }
            return Err(error);
        }
    };

    let result = relay_tcp_with_idle_timeout(tcp_stack, server, tcp_idle_timeout).await;
    log::info!("Ending {session_info} with {result:?}");
    if let Err(error) = result {
        return Err(error.into());
    }

    Ok(())
}

#[cfg(feature = "udpgw")]
async fn handle_udp_gateway_session(
    mut udp_stack: IpStackUdpStream,
    udpgw_client: Arc<UdpGwClient>,
    udp_dst: &socks5_impl::protocol::Address,
    proxy_handler: Arc<Mutex<dyn ProxyHandler>>,
    socket_queue: Option<Arc<SocketQueue>>,
    ipv6_enabled: bool,
) -> crate::Result<()> {
    let proxy_server_addr = { proxy_handler.lock().await.get_server_addr() };
    let udp_mtu = udpgw_client.get_udp_mtu();
    let udp_timeout = udpgw_client.get_udp_timeout();

    let mut stream = loop {
        match udpgw_client.pop_server_connection_from_queue().await {
            Some(stream) => {
                if stream.is_closed() {
                    continue;
                } else {
                    break stream;
                }
            }
            None => {
                let mut tcp_server_stream = create_tcp_stream(&socket_queue, proxy_server_addr).await?;
                if let Err(e) = handle_proxy_session(&mut tcp_server_stream, proxy_handler).await {
                    return Err(format!("udpgw connection error: {e}").into());
                }
                break UdpGwClientStream::new(tcp_server_stream);
            }
        }
    };

    let tcp_local_addr = stream.local_addr();
    let sn = stream.serial_number();

    log::info!("[UdpGw] Beginning stream {} {} -> {}", sn, &tcp_local_addr, udp_dst);

    let Some(mut reader) = stream.get_reader() else {
        return Err("get reader failed".into());
    };

    let Some(mut writer) = stream.get_writer() else {
        return Err("get writer failed".into());
    };

    let mut tmp_buf = vec![0; udp_mtu.into()];

    loop {
        tokio::select! {
            len = udp_stack.read(&mut tmp_buf) => {
                let read_len = match len {
                    Ok(0) => {
                        log::info!("[UdpGw] Ending stream {} {} <> {}", sn, &tcp_local_addr, udp_dst);
                        break;
                    }
                    Ok(n) => n,
                    Err(e) => {
                        log::info!("[UdpGw] Ending stream {} {} <> {} with udp stack \"{}\"", sn, &tcp_local_addr, udp_dst, e);
                        break;
                    }
                };
                crate::traffic_status::traffic_status_update(read_len, 0)?;
                let sn = stream.serial_number();
                if let Err(e) = UdpGwClient::send_udpgw_packet(ipv6_enabled, &tmp_buf[0..read_len], udp_dst, sn, &mut writer).await {
                    log::info!("[UdpGw] Ending stream {} {} <> {} with send_udpgw_packet {}", sn, &tcp_local_addr, udp_dst, e);
                    break;
                }
                log::debug!("[UdpGw] stream {} {} -> {} send len {}", sn, &tcp_local_addr, udp_dst, read_len);
                stream.update_activity();
            }
            ret = UdpGwClient::recv_udpgw_packet(udp_mtu, udp_timeout, &mut reader) => {
                if let Ok((len, _)) = ret {
                    crate::traffic_status::traffic_status_update(0, len)?;
                }
                match ret {
                    Err(e) => {
                        log::warn!("[UdpGw] Ending stream {} {} <> {} with recv_udpgw_packet {}", sn, &tcp_local_addr, udp_dst, e);
                        stream.close();
                        break;
                    }
                    Ok((_, packet)) => match packet {
                        //should not received keepalive
                        UdpGwResponse::KeepAlive => {
                            log::error!("[UdpGw] Ending stream {} {} <> {} with recv keepalive", sn, &tcp_local_addr, udp_dst);
                            stream.close();
                            break;
                        }
                        //server udp may be timeout,can continue to receive udp data?
                        UdpGwResponse::Error => {
                            log::info!("[UdpGw] Ending stream {} {} <> {} with recv udp error", sn, &tcp_local_addr, udp_dst);
                            stream.update_activity();
                            continue;
                        }
                        UdpGwResponse::TcpClose => {
                            log::error!("[UdpGw] Ending stream {} {} <> {} with tcp closed", sn, &tcp_local_addr, udp_dst);
                            stream.close();
                            break;
                        }
                        UdpGwResponse::Data(data) => {
                            use socks5_impl::protocol::StreamOperation;
                            let len = data.len();
                            let f = data.header.flags;
                            log::debug!("[UdpGw] stream {sn} {} <- {} receive {f} len {len}", &tcp_local_addr, udp_dst);
                            if let Err(e) = udp_stack.write_all(&data.data).await {
                                log::error!("[UdpGw] Ending stream {} {} <> {} with send_udp_packet {}", sn, &tcp_local_addr, udp_dst, e);
                                break;
                            }
                        }
                    }
                }
                stream.update_activity();
            }
        }
    }

    if !stream.is_closed() {
        udpgw_client.store_server_connection_full(stream, reader, writer).await;
    }

    Ok(())
}

async fn handle_udp_associate_session(
    mut udp_stack: IpStackUdpStream,
    proxy_type: ProxyType,
    proxy_handler: Arc<Mutex<dyn ProxyHandler>>,
    socket_queue: Option<Arc<SocketQueue>>,
    ipv6_enabled: bool,
) -> crate::Result<()> {
    use socks5_impl::protocol::{Address, StreamOperation, UdpHeader};

    let (session_info, server_addr, domain_name, udp_addr) = {
        let handler = proxy_handler.lock().await;
        (
            handler.get_session_info(),
            handler.get_server_addr(),
            handler.get_domain_name(),
            handler.get_udp_associate(),
        )
    };

    log::info!("Beginning {session_info}");

    // `_server` is meaningful here, it must be alive all the time
    // to ensure that UDP transmission will not be interrupted accidentally.
    let (_server, udp_addr) = match udp_addr {
        Some(udp_addr) => (None, udp_addr),
        None => {
            let mut server = create_tcp_stream(&socket_queue, server_addr).await?;
            let udp_addr = handle_proxy_session(&mut server, proxy_handler).await?;
            (Some(server), udp_addr.ok_or("udp associate failed")?)
        }
    };

    let mut udp_server = create_udp_stream(&socket_queue, udp_addr).await?;

    let mut buf1 = [0_u8; 4096];
    let mut buf2 = [0_u8; 4096];
    loop {
        tokio::select! {
            len = udp_stack.read(&mut buf1) => {
                let len = len?;
                if len == 0 {
                    break;
                }
                let buf1 = &buf1[..len];

                crate::traffic_status::traffic_status_update(len, 0)?;

                if let ProxyType::Socks4 | ProxyType::Socks5 = proxy_type {
                    let s5addr = if let Some(domain_name) = &domain_name {
                        Address::DomainAddress(domain_name.clone().into(), session_info.dst.port())
                    } else {
                        session_info.dst.into()
                    };

                    // Add SOCKS5 UDP header to the incoming data
                    let mut s5_udp_data = Vec::<u8>::new();
                    UdpHeader::new(0, s5addr).write_to_stream(&mut s5_udp_data)?;
                    s5_udp_data.extend_from_slice(buf1);

                    udp_server.write_all(&s5_udp_data).await?;
                } else {
                    udp_server.write_all(buf1).await?;
                }
            }
            len = udp_server.read(&mut buf2) => {
                let len = len?;
                if len == 0 {
                    break;
                }
                let buf2 = &buf2[..len];

                crate::traffic_status::traffic_status_update(0, len)?;

                if let ProxyType::Socks4 | ProxyType::Socks5 = proxy_type {
                    // Remove SOCKS5 UDP header from the server data
                    let header = UdpHeader::retrieve_from_stream(&mut &buf2[..])?;
                    let data = &buf2[header.len()..];

                    let buf = if session_info.dst.port() == DNS_PORT {
                        let mut message = dns::parse_data_to_dns_message(data, false)?;
                        if !ipv6_enabled {
                            dns::remove_ipv6_entries(&mut message);
                        }
                        message.to_vec()?
                    } else {
                        data.to_vec()
                    };

                    udp_stack.write_all(&buf).await?;
                } else {
                    udp_stack.write_all(buf2).await?;
                }
            }
        }
    }

    log::info!("Ending {session_info}");

    Ok(())
}

async fn handle_dns_over_tcp_session(
    mut udp_stack: IpStackUdpStream,
    proxy_handler: Arc<Mutex<dyn ProxyHandler>>,
    socket_queue: Option<Arc<SocketQueue>>,
    ipv6_enabled: bool,
) -> crate::Result<()> {
    let (session_info, server_addr) = {
        let handler = proxy_handler.lock().await;

        (handler.get_session_info(), handler.get_server_addr())
    };

    let mut server = create_tcp_stream(&socket_queue, server_addr).await?;

    log::info!("Beginning {session_info}");

    let _ = handle_proxy_session(&mut server, proxy_handler).await?;

    let mut buf1 = [0_u8; 4096];
    let mut buf2 = [0_u8; 4096];
    loop {
        tokio::select! {
            len = udp_stack.read(&mut buf1) => {
                let len = len?;
                if len == 0 {
                    break;
                }
                let buf1 = &buf1[..len];

                _ = dns::parse_data_to_dns_message(buf1, false)?;

                // Insert the DNS message length in front of the payload
                let len = u16::try_from(buf1.len())?;
                let mut buf = Vec::with_capacity(std::mem::size_of::<u16>() + usize::from(len));
                buf.extend_from_slice(&len.to_be_bytes());
                buf.extend_from_slice(buf1);

                server.write_all(&buf).await?;

                crate::traffic_status::traffic_status_update(buf.len(), 0)?;
            }
            len = server.read(&mut buf2) => {
                let len = len?;
                if len == 0 {
                    break;
                }
                let mut buf = buf2[..len].to_vec();

                crate::traffic_status::traffic_status_update(0, len)?;

                let mut to_send: VecDeque<Vec<u8>> = VecDeque::new();
                loop {
                    if buf.len() < 2 {
                        break;
                    }
                    let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
                    if buf.len() < len + 2 {
                        break;
                    }

                    // remove the length field
                    let data = buf[2..len + 2].to_vec();

                    let mut message = dns::parse_data_to_dns_message(&data, false)?;

                    let name = dns::extract_domain_from_dns_message(&message)?;
                    let ip = dns::extract_ipaddr_from_dns_message(&message);
                    log::trace!("DNS over TCP query result: {name} -> {ip:?}");

                    if !ipv6_enabled {
                        dns::remove_ipv6_entries(&mut message);
                    }

                    to_send.push_back(message.to_vec()?);
                    if len + 2 == buf.len() {
                        break;
                    }
                    buf = buf[len + 2..].to_vec();
                }

                while let Some(packet) = to_send.pop_front() {
                    udp_stack.write_all(&packet).await?;
                }
            }
        }
    }

    log::info!("Ending {session_info}");

    Ok(())
}

/// This function is used to handle the business logic of tun2proxy and SOCKS5 server.
/// When handling UDP proxy, the return value UDP associate IP address is the result of this business logic.
/// However, when handling TCP business logic, the return value Ok(None) is meaningless, just indicating that the operation was successful.
async fn handle_proxy_session<S>(server: &mut S, proxy_handler: Arc<Mutex<dyn ProxyHandler>>) -> crate::Result<Option<SocketAddr>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut launched = false;
    let mut proxy_handler = proxy_handler.lock().await;
    let dir = OutgoingDirection::ToServer;

    loop {
        if proxy_handler.connection_established() {
            break;
        }

        if !launched {
            let data = proxy_handler.peek_data(dir).buffer;
            let len = data.len();
            if len == 0 {
                return Err("proxy_handler launched went wrong".into());
            }
            server.write_all(data).await?;
            proxy_handler.consume_data(dir, len);

            launched = true;
        }

        let mut buf = [0_u8; 4096];
        let len = server.read(&mut buf).await?;
        if len == 0 {
            return Err("server closed accidentially".into());
        }
        let event = IncomingDataEvent {
            direction: IncomingDirection::FromServer,
            buffer: &buf[..len],
        };
        proxy_handler.push_data(event).await?;

        let data = proxy_handler.peek_data(dir).buffer;
        let len = data.len();
        if len > 0 {
            server.write_all(data).await?;
            proxy_handler.consume_data(dir, len);
        }
    }
    // Proxy negotiation is local adapter control-plane traffic. Reporting the
    // CONNECT/SOCKS handshake here would let readiness masquerade as captured
    // application payload. Only the post-negotiation relay records traffic.
    Ok(proxy_handler.get_udp_associate())
}

#[cfg(test)]
mod masq_patch_tests {
    use super::*;
    use crate::session_metrics::SESSION_METRICS_TEST_LOCK;
    use crate::traffic_status::TRAFFIC_STATUS_TEST_LOCK;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static PAYLOAD_CALLBACK_COUNT: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn count_payload_callback(_status: *const TrafficStatus, _ctx: *mut c_void) {
        PAYLOAD_CALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    fn restrictive_args() -> Args {
        let mut args = Args::default();
        args.dns = ArgDns::Virtual;
        args.ipv6_enabled = false;
        args.session_policy = SessionPolicy {
            reject_ipv6: true,
            reject_udp_except_virtual_dns: true,
            allowed_tcp_ports: vec![443],
            ..SessionPolicy::default()
        };
        args
    }

    fn session(src: &str, dst: &str, protocol: IpProtocol) -> SessionInfo {
        SessionInfo::new(
            src.parse().expect("source address"),
            dst.parse().expect("destination address"),
            protocol,
        )
    }

    fn configured_mss(config: &ipstack::TcpConfig) -> Option<u16> {
        config.options.as_ref()?.iter().find_map(|option| match option {
            ipstack::TcpOptions::MaximumSegmentSize(mss) => Some(*mss),
            _ => None,
        })
    }

    #[test]
    fn tunnel_tcp_config_advertises_mss_derived_from_ipv4_mtu() {
        let config = tcp_config_for_tun_mtu(1_500, 37, true);

        assert_eq!(configured_mss(&config), Some(1_460));
        assert_eq!(config.timeout, Duration::from_secs(37));
        assert_eq!(configured_mss(&tcp_config_for_tun_mtu(1_280, 37, true)), Some(1_240));
    }

    #[test]
    fn default_or_invalid_mtu_does_not_advertise_an_ipv4_mss() {
        assert_eq!(configured_mss(&tcp_config_for_tun_mtu(1_500, 37, false)), None);
        assert_eq!(configured_mss(&tcp_config_for_tun_mtu(40, 37, true)), None);
        assert_eq!(configured_mss(&tcp_config_for_tun_mtu(39, 37, true)), None);
    }

    #[test]
    fn restrictive_policy_allows_only_ipv4_tcp_443_and_virtual_dns() {
        let args = restrictive_args();

        assert_eq!(
            pre_accounting_rejection(&args, &session("10.0.0.2:1234", "198.18.0.1:443", IpProtocol::Tcp),),
            None,
        );
        assert_eq!(
            pre_accounting_rejection(&args, &session("10.0.0.2:1234", "198.18.0.1:80", IpProtocol::Tcp),),
            Some(SessionRejection::TcpPort),
        );
        assert_eq!(
            pre_accounting_rejection(&args, &session("10.0.0.2:1234", "198.18.0.1:443", IpProtocol::Udp),),
            Some(SessionRejection::Udp),
        );
        assert_eq!(
            pre_accounting_rejection(&args, &session("10.0.0.2:1234", "10.111.0.2:53", IpProtocol::Udp),),
            None,
        );
        assert_eq!(
            pre_accounting_rejection(&args, &session("[fd00::2]:1234", "[2001:db8::1]:443", IpProtocol::Tcp),),
            Some(SessionRejection::Ipv6),
        );
    }

    #[test]
    fn more_than_capacity_quic_rejections_leave_tcp_443_admissible() {
        let _lock = SESSION_METRICS_TEST_LOCK.lock().expect("test lock");
        let args = restrictive_args();
        reset_session_metrics(256);

        for _ in 0..300 {
            let quic = session("10.0.0.2:1234", "198.18.0.1:443", IpProtocol::Udp);
            let rejection = pre_accounting_rejection(&args, &quic).expect("QUIC must be rejected");
            assert_eq!(rejection, SessionRejection::Udp);
            record_session_rejection(rejection);
        }

        let after_quic = session_metrics_snapshot();
        assert_eq!(after_quic.rejected_udp, 300);
        assert_eq!(after_quic.active_sessions, 0);
        assert_eq!(after_quic.rejected_capacity, 0);

        let https = session("10.0.0.2:1234", "198.18.0.1:443", IpProtocol::Tcp);
        assert_eq!(pre_accounting_rejection(&args, &https), None);
        let https_permit = SessionPermit::try_acquire(256).expect("TCP/443 capacity");
        assert_eq!(session_metrics_snapshot().active_sessions, 1);
        drop(https_permit);
        assert_eq!(session_metrics_snapshot().active_sessions, 0);
    }

    #[test]
    fn default_policy_preserves_upstream_transport_acceptance() {
        let args = Args::default();
        assert_eq!(args.session_policy.proxy_handshake_timeout, None);
        assert_eq!(args.session_policy.tcp_idle_timeout, None);
        assert_eq!(
            pre_accounting_rejection(&args, &session("10.0.0.2:1234", "8.8.8.8:80", IpProtocol::Tcp),),
            None,
        );
        assert_eq!(
            pre_accounting_rejection(&args, &session("10.0.0.2:1234", "8.8.8.8:443", IpProtocol::Udp),),
            None,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hung_http_connect_times_out_and_releases_capacity() {
        let _lock = SESSION_METRICS_TEST_LOCK.lock().expect("test lock");
        reset_session_metrics(1);

        let proxy_addr = "127.0.0.1:1".parse().expect("proxy address");
        let (proxy_client, mut proxy_server) = tokio::io::duplex(1024);
        let fake_proxy = tokio::spawn(async move {
            let mut request = Vec::new();
            let mut chunk = [0_u8; 256];
            loop {
                let bytes = proxy_server.read(&mut chunk).await.expect("read CONNECT");
                if bytes == 0 {
                    return;
                }
                request.extend_from_slice(&chunk[..bytes]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            assert!(request.starts_with(b"CONNECT "));
            std::future::pending::<()>().await;
        });

        let info = session("10.0.0.2:1234", "198.18.0.1:443", IpProtocol::Tcp);
        let handler = HttpManager::new(proxy_addr, None)
            .new_proxy_handler(info, None, false)
            .await
            .expect("HTTP handler");
        let setup = async move {
            let mut server = proxy_client;
            handle_proxy_session(&mut server, handler).await?;
            Ok(server)
        };
        let error = {
            let _permit = SessionPermit::try_acquire(1).expect("initial session capacity");
            with_proxy_handshake_timeout(setup, Some(Duration::from_millis(100)))
                .await
                .expect_err("hung CONNECT must time out")
        };
        assert!(matches!(
            error,
            crate::Error::Io(ref io_error) if io_error.kind() == ErrorKind::TimedOut
        ));
        fake_proxy.abort();

        assert_eq!(session_metrics_snapshot().active_sessions, 0);
        let replacement = SessionPermit::try_acquire(1).expect("timed-out CONNECT must restore capacity");
        drop(replacement);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn successful_http_connect_does_not_report_application_payload() {
        let _lock = TRAFFIC_STATUS_TEST_LOCK.lock().expect("traffic status test lock");
        PAYLOAD_CALLBACK_COUNT.store(0, Ordering::Relaxed);
        unsafe {
            tun2proxy_set_traffic_status_callback(60, Some(count_payload_callback), std::ptr::null_mut());
        }

        let proxy_addr = "127.0.0.1:1".parse().expect("proxy address");
        let (mut proxy_client, mut proxy_server) = tokio::io::duplex(1024);
        let fake_proxy = tokio::spawn(async move {
            let mut request = Vec::new();
            let mut chunk = [0_u8; 256];
            loop {
                let bytes = proxy_server.read(&mut chunk).await.expect("read CONNECT");
                assert!(bytes > 0);
                request.extend_from_slice(&chunk[..bytes]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            assert!(request.starts_with(b"CONNECT "));
            proxy_server
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await
                .expect("write CONNECT response");
        });
        let handler = HttpManager::new(proxy_addr, None)
            .new_proxy_handler(session("10.0.0.2:1234", "198.18.0.1:443", IpProtocol::Tcp), None, false)
            .await
            .expect("HTTP handler");

        handle_proxy_session(&mut proxy_client, handler).await.expect("successful CONNECT");
        fake_proxy.await.expect("fake proxy task");

        assert_eq!(PAYLOAD_CALLBACK_COUNT.load(Ordering::Relaxed), 0);
        unsafe {
            tun2proxy_set_traffic_status_callback(60, None, std::ptr::null_mut());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn successful_relay_reports_first_tx_and_rx_without_waiting_for_interval() {
        let _lock = TRAFFIC_STATUS_TEST_LOCK.lock().expect("traffic status test lock");
        PAYLOAD_CALLBACK_COUNT.store(0, Ordering::Relaxed);
        unsafe {
            tun2proxy_set_traffic_status_callback(60, Some(count_payload_callback), std::ptr::null_mut());
        }

        let (tunnel_side, mut app_side) = tokio::io::duplex(1024);
        let (proxy_side, mut remote_side) = tokio::io::duplex(1024);
        let relay = tokio::spawn(async move { relay_tcp_with_idle_timeout(tunnel_side, proxy_side, None).await });

        app_side.write_all(b"request").await.expect("send request payload");
        let mut request = [0_u8; 7];
        remote_side.read_exact(&mut request).await.expect("receive request payload");
        assert_eq!(PAYLOAD_CALLBACK_COUNT.load(Ordering::Relaxed), 1);

        remote_side.write_all(b"reply").await.expect("send returned payload");
        let mut reply = [0_u8; 5];
        app_side.read_exact(&mut reply).await.expect("receive returned payload");
        assert_eq!(PAYLOAD_CALLBACK_COUNT.load(Ordering::Relaxed), 2);

        app_side.shutdown().await.expect("close app half");
        remote_side.shutdown().await.expect("close remote half");
        drop(app_side);
        drop(remote_side);
        assert_eq!(relay.await.expect("relay task").expect("relay result"), (7, 5));

        unsafe {
            tun2proxy_set_traffic_status_callback(60, None, std::ptr::null_mut());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn idle_half_open_relay_times_out_and_releases_capacity() {
        let _lock = SESSION_METRICS_TEST_LOCK.lock().expect("test lock");
        reset_session_metrics(1);

        let (tunnel_side, _app_side) = tokio::io::duplex(1024);
        let (proxy_side, _remote_side) = tokio::io::duplex(1024);
        let error = {
            let _permit = SessionPermit::try_acquire(1).expect("idle session capacity");
            assert_eq!(session_metrics_snapshot().active_sessions, 1);
            relay_tcp_with_idle_timeout(tunnel_side, proxy_side, Some(Duration::from_millis(75)))
                .await
                .expect_err("half-open relay must expire")
        };

        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert_eq!(session_metrics_snapshot().active_sessions, 0);
        let replacement = SessionPermit::try_acquire(1).expect("idle timeout must restore capacity");
        drop(replacement);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tcp_443_is_admitted_and_relays_tls_after_timeout_recovery() {
        let _lock = SESSION_METRICS_TEST_LOCK.lock().expect("test lock");
        reset_session_metrics(1);

        let args = restrictive_args();
        let https = session("10.0.0.2:1234", "198.18.0.1:443", IpProtocol::Tcp);
        assert_eq!(pre_accounting_rejection(&args, &https), None);
        let permit = SessionPermit::try_acquire(1).expect("recovered TCP/443 capacity");

        let (tunnel_side, mut app_side) = tokio::io::duplex(1024);
        let (proxy_side, mut remote_side) = tokio::io::duplex(1024);
        let relay = tokio::spawn(async move {
            let _permit = permit;
            relay_tcp_with_idle_timeout(tunnel_side, proxy_side, Some(Duration::from_millis(250))).await
        });

        let tls_client_hello = b"\x16\x03\x01\x00\x04MASQ";
        app_side.write_all(tls_client_hello).await.expect("send TLS-like payload");
        let mut received = vec![0_u8; tls_client_hello.len()];
        remote_side.read_exact(&mut received).await.expect("relay request");
        assert_eq!(received, tls_client_hello);

        // Keep the relay alive for longer than its original deadline. Each
        // payload crosses before the current deadline and must reset the one
        // shared activity timer for both directions.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let tls_keep_alive = b"\x17\x03\x03\x00\x01K";
        app_side.write_all(tls_keep_alive).await.expect("send TLS keep-alive payload");
        let mut keep_alive = vec![0_u8; tls_keep_alive.len()];
        remote_side.read_exact(&mut keep_alive).await.expect("relay keep-alive");
        assert_eq!(keep_alive, tls_keep_alive);

        tokio::time::sleep(Duration::from_millis(150)).await;
        let tls_server_reply = b"\x16\x03\x03\x00\x02OK";
        remote_side.write_all(tls_server_reply).await.expect("send TLS-like reply");
        let mut reply = vec![0_u8; tls_server_reply.len()];
        app_side.read_exact(&mut reply).await.expect("relay response");
        assert_eq!(reply, tls_server_reply);

        app_side.shutdown().await.expect("close app half");
        remote_side.shutdown().await.expect("close remote half");
        drop(app_side);
        drop(remote_side);
        let totals = relay.await.expect("relay task").expect("healthy relay");
        assert_eq!(
            totals,
            (
                (tls_client_hello.len() + tls_keep_alive.len()) as u64,
                tls_server_reply.len() as u64,
            )
        );
        assert_eq!(session_metrics_snapshot().active_sessions, 0);
    }
}
