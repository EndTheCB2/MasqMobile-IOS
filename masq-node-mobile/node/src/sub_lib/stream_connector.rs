// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::sub_lib::tokio_wrappers::ReadHalfWrapper;
use crate::sub_lib::tokio_wrappers::ReadHalfWrapperReal;
use crate::sub_lib::tokio_wrappers::WriteHalfWrapper;
use crate::sub_lib::tokio_wrappers::WriteHalfWrapperReal;
#[cfg(target_os = "ios")]
use futures::future;
#[cfg(not(target_os = "ios"))]
use futures_cpupool::CpuPool;
use masq_lib::logger::Logger;
#[cfg(target_os = "ios")]
use std::ffi::CString;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::net::TcpStream as StdTcpStream;
#[cfg(target_os = "ios")]
use std::os::fd::FromRawFd;
use std::time::Duration;
use tokio::io;
use tokio::io::AsyncRead;
use tokio::net::TcpStream;
use tokio::prelude::Future;
use tokio::reactor::Handle;

pub const CONNECT_TIMEOUT_MS: u64 = 5000;
pub type ConnectionInfoFuture = Box<dyn Future<Item = ConnectionInfo, Error = io::Error> + Send>;

#[cfg(not(target_os = "ios"))]
lazy_static::lazy_static! {
    // Blocking connect_timeout calls cannot stall an Actix/Tokio reactor. Two
    // workers are sufficient for the two entry nodes used by mobile consume
    // mode and bound the amount of native work queued by repeated retries.
    static ref TCP_CONNECT_POOL: CpuPool = CpuPool::new(2);
}

pub struct ConnectionInfo {
    pub reader: Box<dyn ReadHalfWrapper>,
    pub writer: Box<dyn WriteHalfWrapper>,
    pub local_addr: SocketAddr,
    pub peer_addr: SocketAddr,
}

pub trait StreamConnector: Send {
    fn connect(&self, socket_addr: SocketAddr, logger: &Logger) -> ConnectionInfoFuture;
    fn connect_one(
        &self,
        ip_addrs: Vec<IpAddr>,
        target_hostname: &str,
        target_port: u16,
        logger: &Logger,
    ) -> Result<ConnectionInfo, io::Error>;
    fn split_stream(&self, stream: TcpStream, logger: &Logger) -> Option<ConnectionInfo>;
}

#[derive(Clone)]
pub struct StreamConnectorReal {}

#[cfg(target_os = "ios")]
extern "C" {
    fn masq_apple_tcp_connect(
        host: *const std::os::raw::c_char,
        port: u16,
        timeout_ms: i32,
        error_code: *mut i32,
    ) -> i32;
}

impl StreamConnector for StreamConnectorReal {
    fn connect(&self, socket_addr: SocketAddr, logger: &Logger) -> ConnectionInfoFuture {
        #[cfg(target_os = "ios")]
        {
            let future_logger = logger.clone();
            return Box::new(future::lazy(move || {
                connect_with_apple_stream(socket_addr, &future_logger)
            }));
        }

        #[cfg(not(target_os = "ios"))]
        {
            let connect_logger = logger.clone();
            let conversion_logger = logger.clone();
            let connect_job = crate::mobile_runtime::track_stream_connect_job();
            blocking_connect_then_convert(
                move || {
                    let _connect_job = connect_job;
                    connect_standard_socket(socket_addr).map_err(|error| {
                        error!(
                            connect_logger,
                            "Could not connect TCP stream; remote redacted"
                        );
                        error
                    })
                },
                move |stream| finish_standard_socket(stream, &conversion_logger),
            )
        }
    }

    fn connect_one(
        &self,
        ip_addrs: Vec<IpAddr>,
        _target_hostname: &str,
        target_port: u16,
        logger: &Logger,
    ) -> Result<ConnectionInfo, io::Error> {
        let mut last_error = io::Error::from(ErrorKind::Other);
        let mut address_count = 0usize;

        for ip_addr in ip_addrs {
            address_count += 1;
            let socket_addr = SocketAddr::new(ip_addr, target_port);

            match connect_one_socket(socket_addr, logger) {
                Ok(connection_info) => {
                    debug!(logger, "Connected new stream; remote redacted");
                    return Ok(connection_info);
                }
                Err(e) => {
                    last_error = e;
                    continue;
                }
            };
        }

        error!(
            logger,
            "Could not connect to any of {} candidate IP address(es); destination and addresses redacted",
            address_count
        );
        Err(last_error)
    }

    fn split_stream(&self, stream: TcpStream, logger: &Logger) -> Option<ConnectionInfo> {
        let local_addr = stream
            .local_addr()
            .expect("Stream has no local_addr before splitting");
        let peer_addr = match stream.peer_addr() {
            Ok(addr) => addr,
            Err(e) => {
                error!(logger, "Stream has no peer_addr before splitting: {}", e);
                return None;
            }
        };
        let (read_half, write_half) = stream.split();
        Some(ConnectionInfo {
            reader: Box::new(ReadHalfWrapperReal::new(read_half)),
            writer: Box::new(WriteHalfWrapperReal::new(write_half)),
            local_addr,
            peer_addr,
        })
    }
}

#[cfg(target_os = "ios")]
fn connect_one_socket(
    socket_addr: SocketAddr,
    logger: &Logger,
) -> Result<ConnectionInfo, io::Error> {
    // iOS can deny POSIX connect(2) from an embedded Rust runtime even though
    // the app has normal outbound-network access. Use the same CFStream bridge
    // as the asynchronous entry-node path so exit and RPC-adjacent streams do
    // not fail with EPERM.
    connect_with_apple_stream(socket_addr, logger)
}

#[cfg(not(target_os = "ios"))]
fn connect_one_socket(
    socket_addr: SocketAddr,
    logger: &Logger,
) -> Result<ConnectionInfo, io::Error> {
    let stream = connect_standard_socket(socket_addr)?;
    finish_standard_socket(stream, logger)
}

#[cfg(not(target_os = "ios"))]
fn connect_standard_socket(socket_addr: SocketAddr) -> Result<StdTcpStream, io::Error> {
    let stream =
        StdTcpStream::connect_timeout(&socket_addr, Duration::from_millis(CONNECT_TIMEOUT_MS))?;
    stream.set_nonblocking(true)?;
    Ok(stream)
}

#[cfg(not(target_os = "ios"))]
fn finish_standard_socket(
    stream: StdTcpStream,
    logger: &Logger,
) -> Result<ConnectionInfo, io::Error> {
    let tokio_stream = TcpStream::from_std(stream, &Handle::default())?;
    StreamConnectorReal {}
        .split_stream(tokio_stream, logger)
        .ok_or_else(|| io::Error::new(ErrorKind::Other, "Stream could not be split"))
}

#[cfg(not(target_os = "ios"))]
fn blocking_connect_then_convert<BlockingConnect, Convert>(
    blocking_connect: BlockingConnect,
    convert: Convert,
) -> ConnectionInfoFuture
where
    BlockingConnect: FnOnce() -> Result<StdTcpStream, io::Error> + Send + 'static,
    Convert: FnOnce(StdTcpStream) -> Result<ConnectionInfo, io::Error> + Send + 'static,
{
    // Keep only the potentially blocking connect(2) call on the bounded CPU
    // pool. `and_then` is polled by the caller's Actix/Tokio executor, so
    // `TcpStream::from_std` binds the socket to that executor's reactor rather
    // than to a fallback reactor created on a CPU worker.
    Box::new(
        TCP_CONNECT_POOL
            .spawn_fn(blocking_connect)
            .and_then(convert),
    )
}

#[cfg(target_os = "ios")]
fn connect_with_apple_stream(
    socket_addr: SocketAddr,
    logger: &Logger,
) -> Result<ConnectionInfo, io::Error> {
    let host =
        CString::new(socket_addr.ip().to_string()).expect("numeric socket address contains no NUL");
    let mut error_code = libc::EIO;
    let descriptor = unsafe {
        masq_apple_tcp_connect(
            host.as_ptr(),
            socket_addr.port(),
            CONNECT_TIMEOUT_MS as i32,
            &mut error_code,
        )
    };
    if descriptor < 0 {
        error!(
            logger,
            "Could not connect Apple TCP stream; remote and error details redacted"
        );
        return Err(io::Error::from_raw_os_error(error_code));
    }

    let standard_stream = unsafe { StdTcpStream::from_raw_fd(descriptor) };
    standard_stream.set_nonblocking(true)?;
    let mut stream = TcpStream::from_std(standard_stream, &Handle::default())?;
    let (read_half, write_half) = stream.split();
    Ok(ConnectionInfo {
        reader: Box::new(ReadHalfWrapperReal::new(read_half)),
        writer: Box::new(WriteHalfWrapperReal::new(write_half)),
        local_addr: SocketAddr::new("127.0.0.1".parse().expect("loopback is valid"), 0),
        peer_addr: socket_addr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::little_tcp_server::LittleTcpServer;
    use crossbeam_channel::unbounded;
    use futures::future::lazy;
    use futures::future::ok;
    use masq_lib::test_utils::logging::init_test_logging;
    use masq_lib::test_utils::logging::TestLogHandler;
    use masq_lib::utils::{find_free_port, localhost};
    use std::net::{IpAddr, Shutdown};
    use std::str::FromStr;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::thread;
    use std::time::Duration;
    use tokio;
    use tokio::io::ErrorKind;

    #[test]
    fn constants_have_correct_values() {
        assert_eq!(CONNECT_TIMEOUT_MS, 5000);
    }

    #[test]
    fn stream_connector_can_fail_to_connect() {
        init_test_logging();
        let dead_port = find_free_port();
        let socket_addr = SocketAddr::new(localhost(), dead_port);
        let logger = Logger::new("test");
        let subject = StreamConnectorReal {};

        let future = subject.connect(socket_addr, &logger);

        FutureAsserter::new(future).assert(move |result| {
            let actual = result.err().unwrap().kind();
            assert_eq!(
                actual,
                ErrorKind::ConnectionRefused,
                "Expected {:?}, got {:?}",
                ErrorKind::ConnectionRefused,
                actual
            );
            success()
        });
        TestLogHandler::new()
            .exists_log_containing("ERROR: test: Could not connect TCP stream; remote redacted");
    }

    #[test]
    fn stream_connector_can_succeed_to_connect() {
        let server = LittleTcpServer::start();
        let logger = Logger::new("test");
        let subject = StreamConnectorReal {};

        let future = subject.connect(server.socket_addr(), &logger);

        FutureAsserter::new(future).assert(move |result| {
            let connection_info = result.unwrap();
            assert_eq!(connection_info.local_addr.ip(), localhost());
            assert_eq!(connection_info.peer_addr, server.socket_addr());
            success()
        });
    }

    #[test]
    fn async_connector_defers_tokio_conversion_to_the_polling_executor() {
        let server = LittleTcpServer::start();
        let socket_addr = server.socket_addr();
        let logger = Logger::new("reactor-affinity");
        let (blocking_thread_tx, blocking_thread_rx) = unbounded();
        let (conversion_thread_tx, conversion_thread_rx) = unbounded();

        let future = blocking_connect_then_convert(
            move || {
                blocking_thread_tx.send(thread::current().id()).unwrap();
                connect_standard_socket(socket_addr)
            },
            move |stream| {
                conversion_thread_tx.send(thread::current().id()).unwrap();
                finish_standard_socket(stream, &logger)
            },
        );

        FutureAsserter::new(future).assert(move |result| {
            let connection_info = result.unwrap();
            assert_eq!(connection_info.peer_addr, socket_addr);
            success()
        });

        let blocking_thread = blocking_thread_rx.recv().unwrap();
        let conversion_thread = conversion_thread_rx.recv().unwrap();
        assert_ne!(
            blocking_thread, conversion_thread,
            "Tokio socket conversion must not run on the blocking connect worker"
        );
    }

    #[test]
    fn stream_connector_can_try_connections_until_it_succeeds_then_use_the_successful_one() {
        init_test_logging();
        let logger = Logger::new("test");
        let server = LittleTcpServer::start();
        let socket_addr = server.socket_addr();

        let bogus_ip = IpAddr::from_str("255.255.255.255").unwrap();
        let good_ip = socket_addr.ip();

        let subject = StreamConnectorReal {};
        let ip_addrs = vec![bogus_ip, good_ip];

        let (tx, rx) = unbounded();
        let test_future = lazy(move || {
            let connection_result = subject.connect_one(
                ip_addrs,
                &"some hostname".to_string(),
                socket_addr.port(),
                &logger,
            );
            tx.send(connection_result).unwrap();
            Ok(())
        });

        thread::spawn(move || {
            tokio::run(test_future);
        });

        let connection_result = rx.recv().unwrap();

        assert!(connection_result.is_ok());
        let connection_info = connection_result.unwrap();
        assert_eq!(connection_info.peer_addr, socket_addr);
        assert_eq!(connection_info.local_addr.ip(), socket_addr.ip());
    }

    #[test]
    fn stream_connector_only_tries_connecting_until_successful() {
        init_test_logging();
        let logger = Logger::new("test");
        let server = LittleTcpServer::start();
        let socket_addr = server.socket_addr();

        let ip_addr = socket_addr.ip();

        let subject = StreamConnectorReal {};
        let ip_addrs = vec![ip_addr, ip_addr];

        let (connection_info_tx, connection_info_rx) = unbounded();
        let test_future = lazy(move || {
            let connection_result = subject.connect_one(
                ip_addrs,
                &"some hostname".to_string(),
                socket_addr.port(),
                &logger,
            );
            connection_info_tx.send(connection_result).unwrap();
            Ok(())
        });

        thread::spawn(move || {
            tokio::run(test_future);
        });

        let connection_result = connection_info_rx.recv().unwrap();

        assert!(connection_result.is_ok());
        let connection_info = connection_result.unwrap();
        assert_eq!(connection_info.peer_addr, socket_addr);
        assert_eq!(connection_info.local_addr.ip(), socket_addr.ip());

        assert_eq!(server.count_connections(Duration::from_millis(200)), 1);
    }

    #[test]
    fn stream_connector_returns_err_when_it_cannot_connect_to_any_of_the_provided_ip_addrs() {
        init_test_logging();
        let logger = Logger::new("test");

        let bogus_ip = IpAddr::from_str("255.255.255.255").unwrap();

        let subject = StreamConnectorReal {};
        let ip_addrs = vec![bogus_ip];

        let (tx, rx) = unbounded();
        let test_future = lazy(move || {
            let connection_result =
                subject.connect_one(ip_addrs, &"some hostname".to_string(), 9876, &logger);
            tx.send(connection_result).unwrap();
            Ok(())
        });

        thread::spawn(move || {
            tokio::run(test_future);
        });

        let connection_result = rx.recv().unwrap();

        assert!(connection_result.is_err());
        TestLogHandler::new().exists_log_containing(
            "Could not connect to any of 1 candidate IP address(es); destination and addresses redacted",
        );
    }

    #[test]
    fn closed_stream_either_splits_properly_or_doesnt_split_and_logs() {
        init_test_logging();
        let server = LittleTcpServer::start();
        let std_stream = StdTcpStream::connect(server.socket_addr()).unwrap();
        let local_addr = std_stream.local_addr().unwrap();
        let peer_addr = std_stream.peer_addr().unwrap();
        std_stream.shutdown(Shutdown::Both).unwrap();
        thread::sleep(Duration::from_millis(100)); // Shutdown apparently needs time to propagate
        let stream = TcpStream::from_std(std_stream, &Handle::default()).unwrap();
        let logger = Logger::new("either/or");
        let subject = StreamConnectorReal {};

        let result = subject.split_stream(stream, &logger);

        match result {
            Some(connection_info) => {
                // If the split proceeds (Windows), the ConnectionInfo had better be filled out and there'd better be no log
                assert_eq!(local_addr, connection_info.local_addr);
                assert_eq!(peer_addr, connection_info.peer_addr);
                TestLogHandler::new().exists_no_log_containing("either/or");
            }
            None => {
                // If the split fails (Linux, macOS), there'd better be a log
                TestLogHandler::new().exists_log_containing(&format!(
                    "ERROR: either/or: Stream has no peer_addr before splitting:",
                ));
            }
        }
    }

    struct FutureAsserter<I: 'static, E: 'static> {
        future: Box<dyn Future<Item = I, Error = E> + Send>,
    }

    impl<I: 'static, E: 'static> FutureAsserter<I, E> {
        fn new(future: impl Future<Item = I, Error = E> + Send + 'static) -> FutureAsserter<I, E> {
            FutureAsserter {
                future: Box::new(future),
            }
        }

        fn assert<A: 'static>(self, assertions: A)
        where
            A: Send + FnOnce(Result<I, E>) -> Box<dyn Future<Item = (), Error = ()>>,
        {
            let success = Arc::new(Mutex::new(false));
            let inner_success = Arc::clone(&success);

            tokio::run(self.future.then(move |result| {
                match assertions(result).wait() {
                    Ok(_) => {
                        let mut succ = inner_success.lock().unwrap();
                        *succ = true;
                    }
                    Err(_) => (),
                };
                ok(())
            }));
            assert!(*success.lock().unwrap());
        }
    }

    fn success() -> Box<dyn Future<Item = (), Error = ()>> {
        Box::new(ok(()))
    }
}
