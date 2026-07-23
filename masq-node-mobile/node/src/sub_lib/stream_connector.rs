// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::sub_lib::tokio_wrappers::ReadHalfWrapper;
use crate::sub_lib::tokio_wrappers::ReadHalfWrapperReal;
use crate::sub_lib::tokio_wrappers::WriteHalfWrapper;
use crate::sub_lib::tokio_wrappers::WriteHalfWrapperReal;
#[cfg(target_os = "ios")]
use futures::future;
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
use tokio::timer::Timeout;

pub const CONNECT_TIMEOUT_MS: u64 = 5000;
pub type ConnectionInfoFuture = Box<dyn Future<Item = ConnectionInfo, Error = io::Error> + Send>;

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
            let future_logger = logger.clone();
            Box::new(
                Timeout::new(
                    TcpStream::connect(&socket_addr).then(move |result| match result {
                        Ok(stream) => {
                            let local_addr = stream.local_addr().unwrap_or_else(|_| {
                                panic!("Newly-connected stream has no local_addr; remote redacted")
                            });
                            let peer_addr = match stream.peer_addr() {
                                Ok(addr) => addr,
                                // Untested code below: we couldn't figure out how to make this happen in captivity
                                Err(e) => {
                                    error!(
                                        future_logger,
                                        "Newly-connected stream has no peer_addr; remote redacted"
                                    );
                                    return Err(e);
                                }
                            };
                            let (read_half, write_half) = stream.split();
                            Ok(ConnectionInfo {
                                reader: Box::new(ReadHalfWrapperReal::new(read_half)),
                                writer: Box::new(WriteHalfWrapperReal::new(write_half)),
                                local_addr,
                                peer_addr,
                            })
                        }
                        Err(e) => {
                            error!(
                                future_logger,
                                "Could not connect TCP stream; remote redacted"
                            );
                            Err(e)
                        }
                    }),
                    Duration::from_millis(CONNECT_TIMEOUT_MS),
                )
                .map_err(|wrapped_error| match wrapped_error.into_inner() {
                    Some(error) => error,
                    None => io::Error::from(ErrorKind::TimedOut),
                }),
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
    let stream = StdTcpStream::connect(&socket_addr)?;
    let tokio_stream = TcpStream::from_std(stream, &Handle::default())?;
    StreamConnectorReal {}
        .split_stream(tokio_stream, logger)
        .ok_or_else(|| io::Error::new(ErrorKind::Other, "Stream could not be split"))
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
