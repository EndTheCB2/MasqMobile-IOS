// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::discriminator::Discriminator;
use crate::discriminator::DiscriminatorFactory;
use crate::stream_messages::*;
use crate::sub_lib::dispatcher;
use crate::sub_lib::dispatcher::StreamShutdownMsg;
use crate::sub_lib::http_packet_framer::MAX_HTTP_HEADER_BYTES;
use crate::sub_lib::sequencer::Sequencer;
use crate::sub_lib::tokio_wrappers::ReadHalfWrapper;
use crate::sub_lib::utils::{indicates_dead_stream, MAX_CONSECUTIVE_READ_ERRORS};
use actix::Recipient;
use masq_lib::logger::Logger;
use masq_lib::utils::index_of;
use std::net::SocketAddr;
use std::time::SystemTime;
use tokio::prelude::Async;
use tokio::prelude::Future;

pub struct StreamReaderReal {
    stream: Box<dyn ReadHalfWrapper>,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
    reception_port_opt: Option<u16>,
    ibcd_sub: Recipient<dispatcher::InboundClientData>,
    remove_sub: Recipient<RemoveStreamMsg>,
    dispatcher_stream_shutdown_sub: Recipient<StreamShutdownMsg>,
    discriminators: Vec<Discriminator>,
    is_clandestine: bool,
    logger: Logger,
    sequencer: Sequencer,
    connect_probe_opt: Option<Vec<u8>>,
}

impl Future for StreamReaderReal {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Result<Async<()>, ()> {
        let mut buf = [0u8; 0x0001_0000];
        let mut consecutive_read_errors = 0usize;
        loop {
            match self.stream.poll_read(&mut buf) {
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Ok(Async::Ready(0)) => {
                    // see RETURN VALUE section of recv man page (Unix)
                    debug!(
                        self.logger,
                        "Stream has shut down (0-byte read); local and peer addresses redacted"
                    );
                    self.shutdown();
                    return Ok(Async::Ready(()));
                }
                Ok(Async::Ready(length)) => {
                    consecutive_read_errors = 0;
                    debug!(
                        self.logger,
                        "Read {}-byte chunk; local and peer addresses redacted", length
                    );
                    if let Err(_error) = self.wrangle_discriminators(&buf, length) {
                        warning!(
                            self.logger,
                            "Stopping stream after dispatcher backpressure; details redacted"
                        );
                        self.shutdown();
                        return Err(());
                    }
                }
                Err(error) => {
                    if indicates_dead_stream(error.kind()) {
                        debug!(
                            self.logger,
                            "Stream is dead; local and peer addresses and transport error redacted"
                        );
                        self.shutdown();
                        return Err(());
                    } else {
                        consecutive_read_errors += 1;
                        if consecutive_read_errors >= MAX_CONSECUTIVE_READ_ERRORS {
                            error!(
                                self.logger,
                                "Abandoning stream after {} consecutive read errors; transport error redacted",
                                consecutive_read_errors
                            );
                            self.shutdown();
                            return Err(());
                        }
                        warning!(
                            self.logger,
                            "Continuing after read error; local and peer addresses and transport error redacted"
                        )
                    }
                }
            }
        }
    }
}

impl StreamReaderReal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        stream: Box<dyn ReadHalfWrapper>,
        reception_port_opt: Option<u16>,
        ibcd_sub: Recipient<dispatcher::InboundClientData>,
        remove_sub: Recipient<RemoveStreamMsg>,
        dispatcher_sub: Recipient<StreamShutdownMsg>,
        discriminator_factories: Vec<Box<dyn DiscriminatorFactory>>,
        is_clandestine: bool,
        peer_addr: SocketAddr,
        local_addr: SocketAddr,
    ) -> StreamReaderReal {
        if discriminator_factories.is_empty() {
            panic!("Internal error: no Discriminator factories!")
        }
        let discriminators: Vec<Discriminator> = discriminator_factories
            .into_iter()
            .map(|df| (df.make()))
            .collect();
        let connect_probe_opt = if discriminators.len() > 1 {
            Some(Vec::new())
        } else {
            None
        };
        StreamReaderReal {
            stream,
            local_addr,
            peer_addr,
            reception_port_opt,
            ibcd_sub,
            remove_sub,
            dispatcher_stream_shutdown_sub: dispatcher_sub,
            discriminators,
            is_clandestine,
            logger: Logger::new("StreamReader"),
            sequencer: Sequencer::new(),
            connect_probe_opt,
        }
    }

    fn wrangle_discriminators(&mut self, buf: &[u8], length: usize) -> Result<(), String> {
        const CONNECT_PREFIX: &[u8] = b"CONNECT ";
        const HEADER_END: &[u8] = b"\r\n\r\n";
        let data = &buf[..length];
        let mut connect_probe = match self.connect_probe_opt.take() {
            Some(connect_probe) => connect_probe,
            None => {
                return self.process_discriminator_data(0, data, false);
            }
        };
        connect_probe.extend_from_slice(data);
        if CONNECT_PREFIX.starts_with(&connect_probe) {
            self.connect_probe_opt = Some(connect_probe);
            return Ok(());
        }
        if !connect_probe.starts_with(CONNECT_PREFIX) {
            return self.process_discriminator_data(0, &connect_probe, false);
        }
        let header_end = match index_of(&connect_probe, HEADER_END) {
            Some(offset) => offset + HEADER_END.len(),
            None => {
                if connect_probe.len() > MAX_HTTP_HEADER_BYTES {
                    warning!(
                        self.logger,
                        "Discarding CONNECT request whose header exceeds {} bytes",
                        MAX_HTTP_HEADER_BYTES
                    );
                } else {
                    self.connect_probe_opt = Some(connect_probe);
                }
                return Ok(());
            }
        };
        let tls_data = connect_probe.split_off(header_end);
        self.process_discriminator_data(1, &connect_probe, true)?;
        if !tls_data.is_empty() {
            self.process_discriminator_data(0, &tls_data, false)?;
        }
        Ok(())
    }

    fn process_discriminator_data(
        &mut self,
        discriminator_index: usize,
        data: &[u8],
        is_connect: bool,
    ) -> Result<(), String> {
        debug!(self.logger, "Adding {} bytes to discriminator", data.len());
        self.discriminators[discriminator_index].add_data(data);
        loop {
            match self.discriminators[discriminator_index].take_chunk() {
                Some(unmasked_chunk) => {
                    // For Proxy Clients that send an Http Connect message via TLS, sequence_number
                    // should be Some(0). The next message the ProxyClient will send begins the TLS
                    // handshake and should start the sequence at Some(0) as well, the ProxyServer will
                    // handle the sequenced packet offset before sending them through the stream_writer
                    // and avoid dropping duplicate packets.
                    let sequence_number_opt = if unmasked_chunk.sequenced && !is_connect {
                        Some(self.sequencer.next_sequence_number())
                    } else if is_connect {
                        // This case needs to explicitly be Some(0) instead of None so that the StreamHandlerPool does
                        // not masquerade it.
                        Some(0)
                    } else {
                        None
                    };
                    match sequence_number_opt {
                        Some(num) => debug!(
                            self.logger,
                            "Read {} bytes of clear data (#{})",
                            unmasked_chunk.chunk.len(),
                            num
                        ),
                        None => debug!(
                            self.logger,
                            "Read {} bytes of clandestine data",
                            unmasked_chunk.chunk.len()
                        ),
                    };
                    let msg = dispatcher::InboundClientData {
                        timestamp: SystemTime::now(),
                        client_addr: self.peer_addr,
                        reception_port_opt: self.reception_port_opt,
                        last_data: false,
                        is_clandestine: self.is_clandestine,
                        sequence_number_opt,
                        data: unmasked_chunk.chunk.clone(),
                    };
                    debug!(self.logger, "Discriminator framed and unmasked {} bytes for endpoint-redacted stream; transmitting via Hopper",
                                              unmasked_chunk.chunk.len());
                    self.ibcd_sub
                        .try_send(msg)
                        .map_err(|_| "Dispatcher rejected inbound data".to_string())?;
                }
                None => {
                    debug!(self.logger, "Discriminator has no more data framed");
                    break;
                }
            }
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        debug!(self.logger, "Directing removal of {}clandestine StreamReader with reception_port {:?}; local and peer addresses redacted", if self.is_clandestine {""} else {"non-"}, self.reception_port_opt);
        let stream_type = if self.is_clandestine {
            RemovedStreamType::Clandestine
        } else {
            match self.reception_port_opt {
                Some(reception_port) => {
                    RemovedStreamType::NonClandestine(NonClandestineAttributes {
                        reception_port,
                        sequence_number: self.sequencer.next_sequence_number(),
                    })
                }
                None => {
                    error!(
                        self.logger,
                        "Cannot remove non-clandestine stream without a reception port"
                    );
                    return;
                }
            }
        };
        if let Err(_error) = self.remove_sub.try_send(RemoveStreamMsg {
            peer_addr: self.peer_addr,
            local_addr: self.local_addr,
            stream_type,
            dispatcher_sub: self.dispatcher_stream_shutdown_sub.clone(),
        }) {
            warning!(
                self.logger,
                "Unable to notify StreamHandlerPool of stream removal; details redacted"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_request_start_finder::HttpRequestDiscriminatorFactory;
    use crate::json_discriminator_factory::JsonDiscriminatorFactory;
    use crate::json_masquerader::JsonMasquerader;
    use crate::masquerader::Masquerader;
    use crate::node_test_utils::{check_timestamp, make_stream_handler_pool_subs_from_recorder};
    use crate::stream_handler_pool::StreamHandlerPoolSubs;
    use crate::stream_messages::RemovedStreamType::NonClandestine;
    use crate::sub_lib::dispatcher::DispatcherSubs;
    use crate::test_utils::recorder::make_dispatcher_subs_from_recorder;
    use crate::test_utils::recorder::make_recorder;
    use crate::test_utils::recorder::Recorder;
    use crate::test_utils::recorder::Recording;
    use crate::test_utils::tokio_wrapper_mocks::ReadHalfWrapperMock;
    use crate::tls_discriminator_factory::TlsDiscriminatorFactory;
    use actix::Actor;
    use actix::Addr;
    use actix::Arbiter;
    use actix::System;
    use futures::future;
    use masq_lib::constants::HTTP_PORT;
    use masq_lib::test_utils::logging::init_test_logging;
    use masq_lib::test_utils::logging::TestLogHandler;
    use std::io;
    use std::io::ErrorKind;
    use std::net::SocketAddr;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    fn stream_handler_pool_stuff() -> (Arc<Mutex<Recording>>, StreamHandlerPoolSubs) {
        let (shp, _, recording) = make_recorder();
        (
            recording,
            make_stream_handler_pool_subs_from_recorder(&shp.start()),
        )
    }

    fn dispatcher_stuff() -> (Arc<Mutex<Recording>>, DispatcherSubs) {
        let (dispatcher, _, recording) = make_recorder();
        let addr: Addr<Recorder> = dispatcher.start();
        (recording, make_dispatcher_subs_from_recorder(&addr))
    }

    struct SaturatedInboundMailbox;

    impl Actor for SaturatedInboundMailbox {
        type Context = actix::Context<Self>;
    }

    impl actix::Handler<dispatcher::InboundClientData> for SaturatedInboundMailbox {
        type Result = ();

        fn handle(&mut self, _msg: dispatcher::InboundClientData, _ctx: &mut Self::Context) {}
    }

    #[test]
    fn stream_reader_reports_dispatcher_mailbox_backpressure_without_panicking() {
        init_test_logging();
        let system =
            System::new("stream_reader_reports_dispatcher_mailbox_backpressure_without_panicking");
        let (_, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (_, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let saturated_addr = SaturatedInboundMailbox::create(|ctx| {
            ctx.set_mailbox_capacity(1);
            SaturatedInboundMailbox
        });
        let ibcd_sub = saturated_addr.recipient();
        let request = Vec::from("GET http://here.com HTTP/1.1\r\n\r\n".as_bytes());
        let reader = ReadHalfWrapperMock {
            poll_read_results: vec![
                (request.clone(), Ok(Async::Ready(request.len()))),
                (vec![], Ok(Async::NotReady)),
            ],
        };
        let subject = StreamReaderReal::new(
            Box::new(reader),
            Some(1234),
            ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub,
            vec![Box::new(HttpRequestDiscriminatorFactory::new())],
            false,
            peer_addr,
            local_addr,
        );

        Arbiter::spawn(future::lazy(move || {
            let mut subject = subject;
            let filler = dispatcher::InboundClientData {
                timestamp: SystemTime::now(),
                client_addr: peer_addr,
                reception_port_opt: Some(1234),
                last_data: false,
                is_clandestine: false,
                sequence_number_opt: Some(0),
                data: vec![0],
            };
            subject.ibcd_sub.try_send(filler.clone()).unwrap();
            assert!(subject.ibcd_sub.try_send(filler).is_err());
            let error = subject
                .wrangle_discriminators(&request, request.len())
                .unwrap_err();
            assert!(error.contains("Dispatcher rejected inbound data"));
            System::current().stop();
            Ok(())
        }));
        assert_eq!(system.run(), 0);
    }

    #[test]
    fn stream_reader_shuts_down_and_returns_ok_on_0_byte_read() {
        init_test_logging();
        let system = System::new("test");
        let (shp_recording_arc, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (_, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> =
            vec![Box::new(HttpRequestDiscriminatorFactory::new())];
        let reader = ReadHalfWrapperMock {
            poll_read_results: vec![(vec![], Ok(Async::Ready(0)))],
        };

        let mut subject = StreamReaderReal::new(
            Box::new(reader),
            None,
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub.clone(),
            discriminator_factories,
            true,
            peer_addr,
            local_addr,
        );

        let result = subject.poll();

        System::current().stop_with_code(0);
        system.run();

        let shp_recording = shp_recording_arc.lock().unwrap();
        assert_eq!(
            shp_recording.get_record::<RemoveStreamMsg>(0),
            &RemoveStreamMsg {
                peer_addr,
                local_addr,
                stream_type: RemovedStreamType::Clandestine,
                dispatcher_sub: dispatcher_subs.stream_shutdown_sub,
            }
        );

        assert_eq!(result, Ok(Async::Ready(())));

        TestLogHandler::new().exists_log_containing(
            "DEBUG: StreamReader: Stream has shut down (0-byte read); local and peer addresses redacted",
        );
    }

    #[test]
    fn stream_reader_shuts_down_and_returns_err_when_it_gets_a_dead_stream_error() {
        init_test_logging();
        let system = System::new("test");
        let (shp_recording_arc, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (_, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> =
            vec![Box::new(HttpRequestDiscriminatorFactory::new())];
        let reader = ReadHalfWrapperMock {
            poll_read_results: vec![(vec![], Err(io::Error::from(ErrorKind::BrokenPipe)))],
        };

        let mut subject = StreamReaderReal::new(
            Box::new(reader),
            None,
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub.clone(),
            discriminator_factories,
            true,
            peer_addr,
            local_addr,
        );

        let result = subject.poll();

        System::current().stop_with_code(0);
        system.run();

        let shp_recording = shp_recording_arc.lock().unwrap();
        assert_eq!(
            shp_recording.get_record::<RemoveStreamMsg>(0),
            &RemoveStreamMsg {
                peer_addr,
                local_addr,
                stream_type: RemovedStreamType::Clandestine,
                dispatcher_sub: dispatcher_subs.stream_shutdown_sub,
            }
        );

        assert_eq!(result, Err(()));

        TestLogHandler::new().exists_log_containing(
            "DEBUG: StreamReader: Stream is dead; local and peer addresses and transport error redacted",
        );
    }

    #[test]
    fn stream_reader_returns_not_ready_when_it_gets_not_ready() {
        init_test_logging();
        let system = System::new("test");
        let (shp_recording_arc, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (d_recording_arc, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> =
            vec![Box::new(HttpRequestDiscriminatorFactory::new())];
        let reader = ReadHalfWrapperMock {
            poll_read_results: vec![(vec![], Ok(Async::NotReady))],
        };

        let mut subject = StreamReaderReal::new(
            Box::new(reader),
            Some(1234 as u16),
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub,
            discriminator_factories,
            true,
            peer_addr,
            local_addr,
        );

        let result = subject.poll();

        System::current().stop_with_code(0);
        system.run();

        assert_eq!(result, Ok(Async::NotReady));

        let shp_recording = shp_recording_arc.lock().unwrap();
        assert_eq!(shp_recording.len(), 0);

        let d_recording = d_recording_arc.lock().unwrap();
        assert_eq!(d_recording.len(), 0);
    }

    #[test]
    fn stream_reader_logs_err_but_does_not_shut_down_when_it_gets_a_non_dead_stream_error() {
        init_test_logging();
        let system = System::new("test");
        let (shp_recording_arc, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (d_recording_arc, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> =
            vec![Box::new(HttpRequestDiscriminatorFactory::new())];
        let reader = ReadHalfWrapperMock {
            poll_read_results: vec![
                (vec![], Err(io::Error::from(ErrorKind::Other))),
                (vec![], Ok(Async::NotReady)),
            ],
        };

        let mut subject = StreamReaderReal::new(
            Box::new(reader),
            Some(1234 as u16),
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub,
            discriminator_factories,
            true,
            peer_addr,
            local_addr,
        );

        let _result = subject.poll();

        System::current().stop_with_code(0);
        system.run();

        TestLogHandler::new().await_log_containing("WARN: StreamReader: Continuing after read error; local and peer addresses and transport error redacted", 1000);

        let shp_recording = shp_recording_arc.lock().unwrap();
        assert_eq!(shp_recording.len(), 0);

        let d_recording = d_recording_arc.lock().unwrap();
        assert_eq!(d_recording.len(), 0);
    }

    #[test]
    fn stream_reader_bounds_consecutive_non_dead_read_errors_and_shuts_down() {
        init_test_logging();
        let system = System::new("test");
        let (shp_recording_arc, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (_, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> =
            vec![Box::new(HttpRequestDiscriminatorFactory::new())];
        let reader = ReadHalfWrapperMock {
            poll_read_results: (0..MAX_CONSECUTIVE_READ_ERRORS)
                .map(|_| (vec![], Err(io::Error::from(ErrorKind::Other))))
                .collect(),
        };
        let mut subject = StreamReaderReal::new(
            Box::new(reader),
            None,
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub.clone(),
            discriminator_factories,
            true,
            peer_addr,
            local_addr,
        );

        let result = subject.poll();

        System::current().stop_with_code(0);
        system.run();
        assert_eq!(result, Err(()));
        assert_eq!(
            shp_recording_arc
                .lock()
                .unwrap()
                .get_record::<RemoveStreamMsg>(0),
            &RemoveStreamMsg {
                peer_addr,
                local_addr,
                stream_type: RemovedStreamType::Clandestine,
                dispatcher_sub: dispatcher_subs.stream_shutdown_sub,
            }
        );
        TestLogHandler::new().exists_log_containing(&format!(
            "ERROR: StreamReader: Abandoning stream after {} consecutive read errors; transport error redacted",
            MAX_CONSECUTIVE_READ_ERRORS
        ));
    }

    #[test]
    #[should_panic(expected = "Internal error: no Discriminator factories!")]
    fn stream_reader_panics_with_no_discriminator_factories() {
        init_test_logging();
        let _system = System::new("test");
        let (_, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (_d_recording_arc, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> = vec![];
        let reader = ReadHalfWrapperMock {
            poll_read_results: vec![(vec![], Ok(Async::Ready(5)))],
        };

        let _subject = StreamReaderReal::new(
            Box::new(reader),
            Some(1234 as u16),
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub,
            discriminator_factories,
            true,
            peer_addr,
            local_addr,
        );
    }

    #[test]
    fn stream_reader_sends_framed_chunks_to_dispatcher() {
        init_test_logging();
        let system = System::new("test");
        let (_, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (d_recording_arc, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> =
            vec![Box::new(HttpRequestDiscriminatorFactory::new())];
        let partial_request = Vec::from("GET http://her".as_bytes());
        let remaining_request = Vec::from("e.com HTTP/1.1\r\n\r\n".as_bytes());
        let reader = ReadHalfWrapperMock {
            poll_read_results: vec![
                (
                    partial_request.clone(),
                    Ok(Async::Ready(partial_request.len())),
                ),
                (
                    remaining_request.clone(),
                    Ok(Async::Ready(remaining_request.len())),
                ),
                (vec![], Ok(Async::NotReady)),
            ],
        };

        let mut subject = StreamReaderReal::new(
            Box::new(reader),
            Some(1234 as u16),
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub,
            discriminator_factories,
            true,
            peer_addr,
            local_addr,
        );
        let before = SystemTime::now();

        subject.poll().err();

        System::current().stop_with_code(0);
        system.run();

        let after = SystemTime::now();
        let d_recording = d_recording_arc.lock().unwrap();
        let d_record = d_recording.get_record::<dispatcher::InboundClientData>(0);
        check_timestamp(before, d_record.timestamp, after);
        assert_eq!(
            d_record,
            &dispatcher::InboundClientData {
                timestamp: d_record.timestamp,
                client_addr: peer_addr,
                reception_port_opt: Some(1234 as u16),
                last_data: false,
                is_clandestine: true,
                sequence_number_opt: Some(0),
                data: Vec::from("GET http://here.com HTTP/1.1\r\n\r\n".as_bytes()),
            }
        );

        TestLogHandler::new().exists_log_containing(
            "DEBUG: StreamReader: Read 14-byte chunk; local and peer addresses redacted",
        );
        TestLogHandler::new().exists_log_containing(
            "DEBUG: StreamReader: Read 18-byte chunk; local and peer addresses redacted",
        );
    }

    #[test]
    fn stream_reader_preserves_tls_data_coalesced_with_http_connect() {
        let system = System::new("test");
        let (_, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (d_recording_arc, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> = vec![
            Box::new(TlsDiscriminatorFactory::new()),
            Box::new(HttpRequestDiscriminatorFactory::new()),
        ];
        let http_connect_request =
            Vec::from("CONNECT www.example.com:443 HTTP/1.1\r\n\r\n".as_bytes());
        // Magic TLS Sauce stolen from Configuration
        let tls_request = Vec::from(&[0x16, 0x03, 0x01, 0x00, 0x03, 0x01, 0x02, 0x03][..]);
        let combined_request = [http_connect_request.clone(), tls_request.clone()].concat();
        let reader = ReadHalfWrapperMock {
            poll_read_results: vec![
                (
                    combined_request.clone(),
                    Ok(Async::Ready(combined_request.len())),
                ),
                (vec![], Ok(Async::NotReady)),
            ],
        };

        let mut subject = StreamReaderReal::new(
            Box::new(reader),
            Some(1234 as u16),
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub,
            discriminator_factories,
            false,
            peer_addr,
            local_addr,
        );

        subject.poll().err();

        System::current().stop();
        system.run();

        let d_recording = d_recording_arc.lock().unwrap();
        assert_eq!(
            http_connect_request,
            d_recording
                .get_record::<dispatcher::InboundClientData>(0)
                .data
        );
        assert_eq!(
            Some(0),
            d_recording
                .get_record::<dispatcher::InboundClientData>(0)
                .sequence_number_opt,
        );
        assert_eq!(
            tls_request,
            d_recording
                .get_record::<dispatcher::InboundClientData>(1)
                .data
        );
        assert_eq!(
            Some(0),
            d_recording
                .get_record::<dispatcher::InboundClientData>(1)
                .sequence_number_opt,
        );
    }

    #[test]
    fn stream_reader_handles_http_connect_fragmented_across_reads() {
        let system = System::new("test");
        let (_, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (d_recording_arc, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> = vec![
            Box::new(TlsDiscriminatorFactory::new()),
            Box::new(HttpRequestDiscriminatorFactory::new()),
        ];
        let connect_pieces = vec![
            Vec::from(&b"CON"[..]),
            Vec::from(&b"NECT www.example.com:443"[..]),
            Vec::from(&b" HTTP/1.1\r\n\r\n"[..]),
        ];
        let http_connect_request = connect_pieces.concat();
        let tls_request = Vec::from(&[0x16, 0x03, 0x01, 0x00, 0x03, 0x01, 0x02, 0x03][..]);
        let reader = ReadHalfWrapperMock {
            poll_read_results: vec![
                (Vec::from(&b"CON"[..]), Ok(Async::Ready(3))),
                (
                    Vec::from(&b"NECT www.example.com:443"[..]),
                    Ok(Async::Ready(24)),
                ),
                (Vec::from(&b" HTTP/1.1\r\n\r\n"[..]), Ok(Async::Ready(13))),
                (tls_request.clone(), Ok(Async::Ready(tls_request.len()))),
                (vec![], Ok(Async::NotReady)),
            ],
        };

        let mut subject = StreamReaderReal::new(
            Box::new(reader),
            Some(1234),
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub,
            discriminator_factories,
            false,
            peer_addr,
            local_addr,
        );

        subject.poll().err();

        System::current().stop();
        system.run();

        let d_recording = d_recording_arc.lock().unwrap();
        let connect_record = d_recording.get_record::<dispatcher::InboundClientData>(0);
        let tls_record = d_recording.get_record::<dispatcher::InboundClientData>(1);
        assert_eq!(connect_record.data, http_connect_request);
        assert_eq!(connect_record.sequence_number_opt, Some(0));
        assert_eq!(tls_record.data, tls_request);
        assert_eq!(tls_record.sequence_number_opt, Some(0));
    }

    #[test]
    fn stream_reader_assigns_a_sequence_to_inbound_client_data_that_are_flagged_as_sequenced() {
        let system = System::new("test");
        let (_, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (d_recording_arc, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> =
            vec![Box::new(HttpRequestDiscriminatorFactory::new())];
        let request1 = Vec::from("GET http://here.com HTTP/1.1\r\n\r\n".as_bytes());
        let request2 = Vec::from("GET http://www.example.com HTTP/1.1\r\n\r\n".as_bytes());
        let reader = ReadHalfWrapperMock {
            poll_read_results: vec![
                (request1.clone(), Ok(Async::Ready(request1.len()))),
                (vec![], Ok(Async::NotReady)),
                (request2.clone(), Ok(Async::Ready(request2.len()))),
                (vec![], Ok(Async::NotReady)),
            ],
        };

        let mut subject = StreamReaderReal::new(
            Box::new(reader),
            Some(1234 as u16),
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub,
            discriminator_factories,
            false,
            peer_addr,
            local_addr,
        );
        let before = SystemTime::now();

        let _result = subject.poll();
        let _result = subject.poll();

        System::current().stop_with_code(0);
        system.run();

        let after = SystemTime::now();
        let d_recording = d_recording_arc.lock().unwrap();
        let d_record = d_recording.get_record::<dispatcher::InboundClientData>(0);
        check_timestamp(before, d_record.timestamp, after);
        assert_eq!(
            d_record,
            &dispatcher::InboundClientData {
                timestamp: d_record.timestamp,
                client_addr: peer_addr,
                reception_port_opt: Some(1234 as u16),
                last_data: false,
                is_clandestine: false,
                sequence_number_opt: Some(0),
                data: Vec::from("GET http://here.com HTTP/1.1\r\n\r\n".as_bytes()),
            }
        );

        let d_record = d_recording.get_record::<dispatcher::InboundClientData>(1);
        check_timestamp(before, d_record.timestamp, after);
        assert_eq!(
            d_record,
            &dispatcher::InboundClientData {
                timestamp: d_record.timestamp,
                client_addr: peer_addr,
                reception_port_opt: Some(1234 as u16),
                last_data: false,
                is_clandestine: false,
                sequence_number_opt: Some(1),
                data: Vec::from("GET http://www.example.com HTTP/1.1\r\n\r\n".as_bytes()),
            }
        );
    }

    #[test]
    fn stream_reader_does_not_assign_sequence_to_inbound_client_data_that_is_not_marked_as_sequence(
    ) {
        let system = System::new("test");
        let (_, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (d_recording_arc, dispatcher_subs) = dispatcher_stuff();
        let client_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> =
            vec![Box::new(JsonDiscriminatorFactory::new())];
        let json_masquerader = JsonMasquerader::new();
        let request = Vec::from(
            json_masquerader
                .mask("GET http://here.com HTTP/1.1\r\n\r\n".as_bytes())
                .unwrap(),
        );
        let reader = ReadHalfWrapperMock {
            poll_read_results: vec![
                (request.clone(), Ok(Async::Ready(request.len()))),
                (vec![], Ok(Async::NotReady)),
            ],
        };

        let mut subject = StreamReaderReal::new(
            Box::new(reader),
            Some(1234 as u16),
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub,
            discriminator_factories,
            true,
            client_addr,
            local_addr,
        );
        let before = SystemTime::now();

        let _result = subject.poll();

        System::current().stop_with_code(0);
        system.run();

        let after = SystemTime::now();
        let d_recording = d_recording_arc.lock().unwrap();
        let d_record = d_recording.get_record::<dispatcher::InboundClientData>(0);
        check_timestamp(before, d_record.timestamp, after);
        assert_eq!(
            d_record,
            &dispatcher::InboundClientData {
                timestamp: d_record.timestamp,
                client_addr,
                reception_port_opt: Some(1234 as u16),
                last_data: false,
                is_clandestine: true,
                sequence_number_opt: None,
                data: Vec::from("GET http://here.com HTTP/1.1\r\n\r\n".as_bytes()),
            }
        );
    }

    #[test]
    fn shutdown_produces_the_correct_stream_shutdown_msg_for_clandestine_reader() {
        let (shp_recording_arc, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (_, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let system = System::new("test");
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> =
            vec![Box::new(JsonDiscriminatorFactory::new())];
        let reader = ReadHalfWrapperMock::new().poll_read_result(vec![], Ok(Async::Ready(0)));
        let mut subject = StreamReaderReal::new(
            Box::new(reader),
            None,
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub.clone(),
            discriminator_factories,
            true,
            peer_addr,
            local_addr,
        );

        subject.shutdown();

        System::current().stop_with_code(0);
        system.run();
        let shp_recording = shp_recording_arc.lock().unwrap();
        let remove_stream_msg = shp_recording.get_record::<RemoveStreamMsg>(0);
        assert_eq!(
            remove_stream_msg,
            &RemoveStreamMsg {
                peer_addr,
                local_addr,
                stream_type: RemovedStreamType::Clandestine,
                dispatcher_sub: dispatcher_subs.stream_shutdown_sub,
            }
        );
    }

    #[test]
    fn shutdown_produces_the_correct_stream_shutdown_msg_for_non_clandestine_reader() {
        let (shp_recording_arc, stream_handler_pool_subs) = stream_handler_pool_stuff();
        let (_, dispatcher_subs) = dispatcher_stuff();
        let peer_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let system = System::new("test");
        let local_addr = SocketAddr::from_str("1.2.3.5:6789").unwrap();
        let discriminator_factories: Vec<Box<dyn DiscriminatorFactory>> =
            vec![Box::new(JsonDiscriminatorFactory::new())];
        let reader = ReadHalfWrapperMock::new().poll_read_result(vec![], Ok(Async::Ready(0)));
        let mut subject = StreamReaderReal::new(
            Box::new(reader),
            Some(HTTP_PORT),
            dispatcher_subs.ibcd_sub,
            stream_handler_pool_subs.remove_sub,
            dispatcher_subs.stream_shutdown_sub.clone(),
            discriminator_factories,
            false,
            peer_addr,
            local_addr,
        );
        subject.sequencer.next_sequence_number(); // just so it's not 0

        subject.shutdown();

        System::current().stop_with_code(0);
        system.run();
        let shp_recording = shp_recording_arc.lock().unwrap();
        let remove_stream_msg = shp_recording.get_record::<RemoveStreamMsg>(0);
        assert_eq!(
            remove_stream_msg,
            &RemoveStreamMsg {
                peer_addr,
                local_addr,
                stream_type: NonClandestine(NonClandestineAttributes {
                    reception_port: HTTP_PORT,
                    sequence_number: 1,
                }),
                dispatcher_sub: dispatcher_subs.stream_shutdown_sub,
            }
        );
    }
}
