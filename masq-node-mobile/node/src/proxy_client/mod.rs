// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

#[cfg(test)]
mod local_test_utils;
mod resolver_wrapper;
mod stream_establisher;
mod stream_handler_pool;
mod stream_reader;
mod stream_writer;

use crate::bootstrapper::CryptDEPair;
use crate::proxy_client::resolver_wrapper::ResolverWrapperFactory;
use crate::proxy_client::resolver_wrapper::ResolverWrapperFactoryReal;
use crate::proxy_client::stream_handler_pool::StreamHandlerPool;
use crate::proxy_client::stream_handler_pool::StreamHandlerPoolFactory;
use crate::proxy_client::stream_handler_pool::StreamHandlerPoolFactoryReal;
use crate::sub_lib::accountant::ReportExitServiceProvidedMessage;
use crate::sub_lib::cryptde::{CryptData, PublicKey};
use crate::sub_lib::hopper::MessageType;
use crate::sub_lib::hopper::{ExpiredCoresPackage, IncipientCoresPackage};
use crate::sub_lib::peer_actors::BindMessage;
use crate::sub_lib::proxy_client::InboundServerData;
use crate::sub_lib::proxy_client::ProxyClientSubs;
use crate::sub_lib::proxy_client::{
    ClientResponsePayload_0v1, DnsResolveFailure_0v1, MeteredClientResponsePayload_0v1,
};
use crate::sub_lib::proxy_client::{ProxyClientConfig, ReceiptSessionValidation};
use crate::sub_lib::proxy_server::ClientRequestPayload_0v1;
use crate::sub_lib::route::Route;
use crate::sub_lib::sequence_buffer::SequencedPacket;
use crate::sub_lib::service_receipt::{
    ReceiptSessionRequest, ServiceKind, ServiceReceipt, ServiceReceiptOfferPayload_0v1,
    SignedServiceReceipt,
};
use crate::sub_lib::stream_key::StreamKey;
use crate::sub_lib::utils::{handle_ui_crash_request, NODE_MAILBOX_CAPACITY};
use crate::sub_lib::versioned_data::VersionedData;
use crate::sub_lib::wallet::Wallet;
use actix::Actor;
use actix::Addr;
use actix::Context;
use actix::Handler;
use actix::Recipient;
use masq_lib::logger::Logger;
use masq_lib::ui_gateway::NodeFromUiMessage;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};
use trust_dns_resolver::config::NameServerConfig;
use trust_dns_resolver::config::Protocol;
use trust_dns_resolver::config::ResolverConfig;
use trust_dns_resolver::config::ResolverOpts;

pub const CRASH_KEY: &str = "PROXYCLIENT";

pub struct ProxyClient {
    dns_servers: Vec<SocketAddr>,
    resolver_wrapper_factory: Box<dyn ResolverWrapperFactory>,
    stream_handler_pool_factory: Box<dyn StreamHandlerPoolFactory>,
    cryptde_pair: CryptDEPair,
    to_hopper: Option<Recipient<IncipientCoresPackage>>,
    to_accountant: Option<Recipient<ReportExitServiceProvidedMessage>>,
    pool: Option<Box<dyn StreamHandlerPool>>,
    stream_contexts: HashMap<StreamKey, StreamContext>,
    exit_service_rate: u64,
    exit_byte_rate: u64,
    is_decentralized: bool,
    crashable: bool,
    receipt_session_validation_opt: Option<ReceiptSessionValidation>,
    logger: Logger,
}

impl Actor for ProxyClient {
    type Context = Context<Self>;
}

impl Handler<BindMessage> for ProxyClient {
    type Result = ();

    fn handle(&mut self, msg: BindMessage, ctx: &mut Self::Context) -> Self::Result {
        debug!(self.logger, "Handling BindMessage");
        ctx.set_mailbox_capacity(NODE_MAILBOX_CAPACITY);
        self.to_hopper = Some(msg.peer_actors.hopper.from_hopper_client);
        self.to_accountant = Some(msg.peer_actors.accountant.report_exit_service_provided);
        let mut config = ResolverConfig::new();
        for dns_server_ref in &self.dns_servers {
            info!(
                self.logger,
                "Adding configured DNS server; address redacted"
            );
            config.add_name_server(NameServerConfig {
                socket_addr: *dns_server_ref,
                protocol: Protocol::Udp,
                tls_dns_name: None,
            })
        }
        let opts = ResolverOpts::default();
        let resolver = self.resolver_wrapper_factory.make(config, opts);
        self.pool = Some(self.stream_handler_pool_factory.make(
            resolver,
            self.cryptde_pair.main.as_ref(),
            self.to_accountant.clone().expect("Accountant is unbound"),
            msg.peer_actors.proxy_client_opt.unwrap(),
            self.exit_service_rate,
            self.exit_byte_rate,
        ));
    }
}

impl Handler<ExpiredCoresPackage<ClientRequestPayload_0v1>> for ProxyClient {
    type Result = ();

    fn handle(
        &mut self,
        msg: ExpiredCoresPackage<ClientRequestPayload_0v1>,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let payload = msg.payload;
        let paying_wallet = msg.paying_wallet;
        if paying_wallet.is_some() || !self.is_decentralized {
            let receipt_request_opt = payload.receipt_session_request_opt.clone();
            if let Some(receipt_request) = receipt_request_opt.as_ref() {
                let validation = match self.receipt_session_validation_opt.as_ref() {
                    Some(validation) => validation,
                    None => {
                        warning!(
                            self.logger,
                            "Refusing metered client request because receipt validation is unavailable"
                        );
                        return;
                    }
                };
                let now_unix_s = match Self::unix_time_now() {
                    Ok(now_unix_s) => now_unix_s,
                    Err(error) => {
                        warning!(self.logger, "Refusing metered client request: {}", error);
                        return;
                    }
                };
                if let Err(error) = receipt_request.verify(
                    validation.chain_id,
                    validation.settlement_contract,
                    now_unix_s,
                ) {
                    warning!(
                        self.logger,
                        "Refusing client request with invalid receipt authorization: {:?}",
                        error
                    );
                    return;
                }
                if let Some(existing_request) = self
                    .stream_contexts
                    .get(&payload.stream_key)
                    .and_then(|context| context.receipt_state_opt.as_ref())
                    .map(|state| &state.request)
                {
                    if existing_request != receipt_request {
                        warning!(
                            self.logger,
                            "Refusing receipt-session change on an active exit stream"
                        );
                        return;
                    }
                }
            }
            let return_route = msg.remaining_route;
            let mut routing_receipt_offers = self
                .stream_contexts
                .get(&payload.stream_key)
                .map(|context| context.routing_receipt_offers.clone())
                .unwrap_or_default();
            routing_receipt_offers.extend(msg.routing_receipt_offers);
            let previous_receipt_state_opt = self
                .stream_contexts
                .get(&payload.stream_key)
                .and_then(|context| context.receipt_state_opt.clone());
            let receipt_state_opt = match receipt_request_opt {
                Some(request) => {
                    let mut state = previous_receipt_state_opt.unwrap_or(ProviderReceiptState {
                        request,
                        last_signed_receipt_opt: None,
                        pending_request_payload_size: 0,
                        pending_request_service_units: 0,
                    });
                    if !payload.sequenced_packet.data.is_empty() {
                        let payload_size = match u64::try_from(payload.sequenced_packet.data.len())
                        {
                            Ok(payload_size) => payload_size,
                            Err(_) => {
                                warning!(
                                    self.logger,
                                    "Refusing metered client request whose size does not fit in a receipt"
                                );
                                return;
                            }
                        };
                        state.pending_request_payload_size =
                            match state.pending_request_payload_size.checked_add(payload_size) {
                                Some(total) => total,
                                None => {
                                    warning!(
                                    self.logger,
                                    "Refusing metered client request after payload-size overflow"
                                );
                                    return;
                                }
                            };
                        state.pending_request_service_units =
                            match state.pending_request_service_units.checked_add(1) {
                                Some(total) => total,
                                None => {
                                    warning!(
                                    self.logger,
                                    "Refusing metered client request after service-unit overflow"
                                );
                                    return;
                                }
                            };
                    }
                    Some(state)
                }
                None => None,
            };
            let latest_stream_context = StreamContext {
                return_route,
                payload_destination_key: payload.originator_public_key.clone(),
                paying_wallet: paying_wallet.clone(),
                dns_attempt_id_opt: payload.dns_attempt_id_opt,
                receipt_state_opt,
                routing_receipt_offers,
            };
            debug!(
                self.logger,
                "Received ClientRequestPayload: stream identity redacted, sequence {}, length {}",
                payload.sequenced_packet.sequence_number,
                payload.sequenced_packet.data.len()
            );
            self.stream_contexts
                .insert(payload.stream_key, latest_stream_context);
            self.pool
                .as_mut()
                .expect("StreamHandlerPool unbound")
                .process_package(payload, paying_wallet);
        } else {
            warning!(self.logger, "Refusing to provide exit services for CORES package with {}-byte payload without paying wallet", payload.sequenced_packet.data.len());
        }
    }
}

impl Handler<InboundServerData> for ProxyClient {
    type Result = ();

    fn handle(&mut self, msg: InboundServerData, _ctx: &mut Self::Context) -> Self::Result {
        let msg_data_len = msg.data.len();
        let msg_sequence_number = msg.sequence_number;
        let msg_last_data = msg.last_data;
        let msg_stream_key = msg.stream_key;
        let stream_context = match self.stream_contexts.get(&msg.stream_key) {
            Some(sc) => sc.clone(),
            None => {
                error!(
                    self.logger,
                    "Received InboundServerData{} for an unknown stream: identity, source and contents redacted; sequence {}, length {} - ignoring",
                    if msg_last_data {" (last_data)"} else {""},
                    msg_sequence_number,
                    msg_data_len
                );
                return;
            }
        };
        let receipt_offer_opt = if stream_context.receipt_state_opt.is_some() {
            match self.make_service_receipt_offer(msg_stream_key, msg_sequence_number, msg_data_len)
            {
                Ok(offer) => Some(offer),
                Err(error) => {
                    warning!(
                        self.logger,
                        "Unable to create exit-service receipt offer; using legacy accounting: {}",
                        error
                    );
                    None
                }
            }
        } else {
            None
        };
        if self
            .send_response_to_hopper(msg, &stream_context, receipt_offer_opt.clone())
            .is_err()
        {
            return;
        };
        if !stream_context.routing_receipt_offers.is_empty() {
            if let Some(context) = self.stream_contexts.get_mut(&msg_stream_key) {
                context.routing_receipt_offers.clear();
            }
        }
        if let Some(offer) = receipt_offer_opt {
            if let Err(error) = self.commit_service_receipt_offer(msg_stream_key, offer) {
                warning!(
                    self.logger,
                    "Unable to retain exit-service receipt offer for cumulative progress: {}",
                    error
                );
            }
        } else {
            self.report_response_exit_to_accountant(&stream_context, msg_data_len);
        }
        if msg_last_data {
            debug!(
                self.logger,
                "Retiring exit stream: no more data; identity redacted"
            );
            self.stream_contexts.remove(&msg_stream_key);
        }
    }
}

impl Handler<DnsResolveFailure_0v1> for ProxyClient {
    type Result = ();

    fn handle(&mut self, msg: DnsResolveFailure_0v1, _ctx: &mut Self::Context) -> Self::Result {
        let stream_key = msg.stream_key;
        let stream_context_opt = self.stream_contexts.get(&stream_key);
        match stream_context_opt {
            Some(stream_context) => {
                if stream_context.dns_attempt_id_opt != msg.dns_attempt_id_opt {
                    warning!(
                        self.logger,
                        "Discarding stale DNS resolution failure for a redacted stream: expected attempt {:?}, received {:?}",
                        stream_context.dns_attempt_id_opt,
                        msg.dns_attempt_id_opt
                    );
                    return;
                }
                let package = match IncipientCoresPackage::new(
                    self.cryptde_pair.main.as_ref(),
                    stream_context.return_route.clone(),
                    MessageType::DnsResolveFailed(VersionedData::new(
                        &crate::sub_lib::migrations::dns_resolve_failure::MIGRATIONS,
                        &msg,
                    )),
                    &stream_context.payload_destination_key,
                ) {
                    Ok(package) => package
                        .with_routing_receipt_offers(stream_context.routing_receipt_offers.clone()),
                    Err(_) => {
                        error!(
                            self.logger,
                            "Could not create DNS-failure CORES package; stream and error details redacted - retiring stream"
                        );
                        self.stream_contexts.remove(&stream_key);
                        return;
                    }
                };
                match self.to_hopper.as_ref() {
                    Some(hopper) if hopper.try_send(package).is_ok() => {}
                    Some(_) => error!(
                        self.logger,
                        "Could not send DNS-failure CORES package for a redacted stream: Hopper is dead"
                    ),
                    None => error!(
                        self.logger,
                        "Could not send DNS-failure CORES package for a redacted stream: Hopper is unbound"
                    ),
                }
                debug!(
                    self.logger,
                    "Removing redacted stream for DnsResolveFailure"
                );
                self.stream_contexts.remove(&stream_key);
            }
            None => error!(
                self.logger,
                "DNS resolution for nonexistent stream failed; identity redacted."
            ),
        }
    }
}

impl Handler<NodeFromUiMessage> for ProxyClient {
    type Result = ();

    fn handle(&mut self, msg: NodeFromUiMessage, _ctx: &mut Self::Context) -> Self::Result {
        handle_ui_crash_request(msg, &self.logger, self.crashable, CRASH_KEY)
    }
}

impl ProxyClient {
    pub fn new(config: ProxyClientConfig) -> ProxyClient {
        if config.dns_servers.is_empty() {
            panic!("ProxyClient requires at least one DNS server IP address after the --dns-servers parameter")
        }
        ProxyClient {
            dns_servers: config.dns_servers,
            resolver_wrapper_factory: Box::new(ResolverWrapperFactoryReal {}),
            stream_handler_pool_factory: Box::new(StreamHandlerPoolFactoryReal {}),
            cryptde_pair: config.cryptde_pair.clone(),
            to_hopper: None,
            to_accountant: None,
            pool: None,
            stream_contexts: HashMap::new(),
            exit_service_rate: config.exit_service_rate,
            exit_byte_rate: config.exit_byte_rate,
            is_decentralized: config.is_decentralized,
            crashable: config.crashable,
            receipt_session_validation_opt: config.receipt_session_validation_opt,
            logger: Logger::new("ProxyClient"),
        }
    }

    pub fn make_subs_from(addr: &Addr<ProxyClient>) -> ProxyClientSubs {
        ProxyClientSubs {
            bind: recipient!(addr, BindMessage),
            from_hopper: recipient!(addr, ExpiredCoresPackage<ClientRequestPayload_0v1>),
            inbound_server_data: recipient!(addr, InboundServerData),
            dns_resolve_failed: recipient!(addr, DnsResolveFailure_0v1),
            node_from_ui: recipient!(addr, NodeFromUiMessage),
        }
    }

    fn send_response_to_hopper(
        &self,
        msg: InboundServerData,
        stream_context: &StreamContext,
        receipt_offer_opt: Option<ServiceReceiptOfferPayload_0v1>,
    ) -> Result<(), ()> {
        let msg_data_len = msg.data.len() as u32;
        let msg_sequence_number = msg.sequence_number;
        let response = ClientResponsePayload_0v1 {
            stream_key: msg.stream_key,
            sequenced_packet: SequencedPacket {
                data: msg.data,
                sequence_number: msg.sequence_number,
                last_data: msg.last_data,
            },
        };
        let payload = match receipt_offer_opt {
            Some(receipt_offer) => MessageType::MeteredClientResponse(VersionedData::new(
                &crate::sub_lib::migrations::metered_client_response_payload::MIGRATIONS,
                &MeteredClientResponsePayload_0v1 {
                    response,
                    receipt_offer,
                },
            )),
            None => MessageType::ClientResponse(VersionedData::new(
                &crate::sub_lib::migrations::client_response_payload::MIGRATIONS,
                &response,
            )),
        };
        debug!(
            self.logger,
            "Sending ClientResponsePayload to Hopper: stream identity redacted, sequence {}, length {}",
            msg_sequence_number,
            msg_data_len
        );
        let icp = match IncipientCoresPackage::new(
            self.cryptde_pair.main.as_ref(),
            stream_context.return_route.clone(),
            payload,
            &stream_context.payload_destination_key,
        ) {
            Ok(icp) => {
                icp.with_routing_receipt_offers(stream_context.routing_receipt_offers.clone())
            }
            Err(_) => {
                error!(self.logger, "Could not create CORES package for {}-byte response, seq {}; source and error details redacted - ignoring", msg_data_len, msg_sequence_number);
                return Err(());
            }
        };
        self.to_hopper
            .as_ref()
            .expect("Hopper unbound")
            .try_send(icp)
            .expect("Hopper is dead");
        Ok(())
    }

    fn report_response_exit_to_accountant(
        &self,
        stream_context: &StreamContext,
        msg_data_len: usize,
    ) {
        if let Some(paying_wallet) = stream_context.paying_wallet.clone() {
            let exit_report = ReportExitServiceProvidedMessage {
                timestamp: SystemTime::now(),
                paying_wallet,
                payload_size: msg_data_len,
                service_rate: self.exit_service_rate,
                byte_rate: self.exit_byte_rate,
            };
            self.to_accountant
                .as_ref()
                .expect("Accountant unbound")
                .try_send(exit_report)
                .expect("Accountant is dead");
        } else {
            debug!(
                self.logger,
                "Relayed {}-byte response without paying wallet for free", msg_data_len
            );
        }
    }

    fn make_service_receipt_offer(
        &self,
        stream_key: StreamKey,
        sequence: u64,
        payload_size: usize,
    ) -> Result<ServiceReceiptOfferPayload_0v1, String> {
        let (
            request,
            last_signed_receipt_opt,
            pending_request_payload_size,
            pending_request_service_units,
        ) = self
            .stream_contexts
            .get(&stream_key)
            .and_then(|context| {
                context.receipt_state_opt.as_ref().map(|state| {
                    (
                        state.request.clone(),
                        state.last_signed_receipt_opt.clone(),
                        state.pending_request_payload_size,
                        state.pending_request_service_units,
                    )
                })
            })
            .ok_or_else(|| "metered stream context disappeared".to_string())?;
        let response_payload_size = u64::try_from(payload_size)
            .map_err(|_| "response payload size does not fit in a receipt".to_string())?;
        let payload_size = pending_request_payload_size
            .checked_add(response_payload_size)
            .ok_or_else(|| "aggregate exit payload size does not fit in a receipt".to_string())?;
        let service_units = pending_request_service_units
            .checked_add(1)
            .ok_or_else(|| "aggregate exit service units do not fit in a receipt".to_string())?;
        let receipt = match last_signed_receipt_opt {
            Some(previous) => previous
                .receipt
                .next_with_service_units(
                    sequence,
                    ServiceKind::Exit,
                    payload_size,
                    service_units,
                    self.exit_service_rate,
                    self.exit_byte_rate,
                )
                .map_err(|error| format!("cannot advance receipt: {:?}", error))?,
            None => ServiceReceipt::new_with_service_units(
                request.route_epoch,
                sequence,
                ServiceKind::Exit,
                self.cryptde_pair.main.public_key().clone(),
                request.accounting_commitment,
                payload_size,
                service_units,
                self.exit_service_rate,
                self.exit_byte_rate,
            ),
        };
        let signed_receipt = receipt
            .sign(self.cryptde_pair.main.as_ref())
            .map_err(|error| format!("cannot sign receipt: {:?}", error))?;
        Ok(ServiceReceiptOfferPayload_0v1 { signed_receipt })
    }

    fn commit_service_receipt_offer(
        &mut self,
        stream_key: StreamKey,
        offer: ServiceReceiptOfferPayload_0v1,
    ) -> Result<(), String> {
        let state = self
            .stream_contexts
            .get_mut(&stream_key)
            .and_then(|context| context.receipt_state_opt.as_mut())
            .ok_or_else(|| "metered stream context disappeared after send".to_string())?;
        state.last_signed_receipt_opt = Some(offer.signed_receipt);
        state.pending_request_payload_size = 0;
        state.pending_request_service_units = 0;
        Ok(())
    }

    fn unix_time_now() -> Result<u64, String> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .map_err(|_| "system clock is before the Unix epoch".to_string())
    }
}

#[derive(Clone)]
struct StreamContext {
    return_route: Route,
    payload_destination_key: PublicKey,
    paying_wallet: Option<Wallet>,
    dns_attempt_id_opt: Option<u64>,
    receipt_state_opt: Option<ProviderReceiptState>,
    routing_receipt_offers: Vec<CryptData>,
}

#[derive(Clone)]
struct ProviderReceiptState {
    request: ReceiptSessionRequest,
    last_signed_receipt_opt: Option<SignedServiceReceipt>,
    pending_request_payload_size: u64,
    pending_request_service_units: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrapper::CryptDEPair;
    use crate::node_test_utils::check_timestamp;
    use crate::proxy_client::local_test_utils::ResolverWrapperFactoryMock;
    use crate::proxy_client::local_test_utils::ResolverWrapperMock;
    use crate::proxy_client::resolver_wrapper::ResolverWrapper;
    use crate::proxy_client::stream_handler_pool::StreamHandlerPool;
    use crate::proxy_client::stream_handler_pool::StreamHandlerPoolFactory;
    use crate::sub_lib::accountant::ReportExitServiceProvidedMessage;
    use crate::sub_lib::cryptde::PublicKey;
    use crate::sub_lib::cryptde::{decodex, CryptDE, CryptData};
    use crate::sub_lib::cryptde_real::CryptDEReal;
    use crate::sub_lib::dispatcher::Component;
    use crate::sub_lib::hopper::MessageType;
    use crate::sub_lib::proxy_client::ClientResponsePayload_0v1;
    use crate::sub_lib::proxy_server::ClientRequestPayload_0v1;
    use crate::sub_lib::proxy_server::ProxyProtocol;
    use crate::sub_lib::route::{Route, RouteSegment};
    use crate::sub_lib::sequence_buffer::SequencedPacket;
    use crate::sub_lib::service_receipt::ReceiptSessionPolicy;
    use crate::sub_lib::versioned_data::VersionedData;
    use crate::sub_lib::wallet::Wallet;
    use crate::test_utils::make_wallet;
    use crate::test_utils::recorder::make_recorder;
    use crate::test_utils::recorder::peer_actors_builder;
    use crate::test_utils::recorder::Recorder;
    use crate::test_utils::unshared_test_utils::prove_that_crash_request_handler_is_hooked_up;
    use crate::test_utils::*;
    use actix::System;
    use lazy_static::lazy_static;
    use masq_lib::blockchains::chains::Chain;
    use masq_lib::test_utils::logging::{init_test_logging, TestLogHandler};
    use masq_lib::test_utils::utils::TEST_DEFAULT_CHAIN;
    use std::cell::RefCell;
    use std::net::SocketAddr;
    use std::net::{IpAddr, SocketAddrV4};
    use std::str::FromStr;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::thread;
    use std::time::SystemTime;

    lazy_static! {
        static ref CRYPTDE_PAIR: CryptDEPair = CryptDEPair::null();
    }

    #[test]
    fn constants_have_correct_values() {
        assert_eq!(CRASH_KEY, "PROXYCLIENT");
    }

    fn dnss() -> Vec<SocketAddr> {
        vec![SocketAddr::from_str("8.8.8.8:53").unwrap()]
    }

    pub struct StreamHandlerPoolMock {
        process_package_parameters: Arc<Mutex<Vec<(ClientRequestPayload_0v1, Option<Wallet>)>>>,
    }

    impl StreamHandlerPool for StreamHandlerPoolMock {
        fn process_package(
            &self,
            payload: ClientRequestPayload_0v1,
            paying_wallet: Option<Wallet>,
        ) {
            self.process_package_parameters
                .lock()
                .unwrap()
                .push((payload, paying_wallet));
        }
    }

    impl StreamHandlerPoolMock {
        pub fn new() -> StreamHandlerPoolMock {
            StreamHandlerPoolMock {
                process_package_parameters: Arc::new(Mutex::new(vec![])),
            }
        }

        pub fn process_package_parameters(
            self,
            parameters: &mut Arc<Mutex<Vec<(ClientRequestPayload_0v1, Option<Wallet>)>>>,
        ) -> StreamHandlerPoolMock {
            *parameters = self.process_package_parameters.clone();
            self
        }
    }

    pub struct StreamHandlerPoolFactoryMock {
        make_parameters: Arc<
            Mutex<
                Vec<(
                    Box<dyn ResolverWrapper>,
                    Box<dyn CryptDE>,
                    Recipient<ReportExitServiceProvidedMessage>,
                    ProxyClientSubs,
                    u64,
                    u64,
                )>,
            >,
        >,
        make_results: RefCell<Vec<Box<dyn StreamHandlerPool>>>,
    }

    impl StreamHandlerPoolFactory for StreamHandlerPoolFactoryMock {
        fn make(
            &self,
            resolver: Box<dyn ResolverWrapper>,
            cryptde: &dyn CryptDE,
            accountant_sub: Recipient<ReportExitServiceProvidedMessage>,
            proxy_client_subs: ProxyClientSubs,
            exit_service_rate: u64,
            exit_byte_rate: u64,
        ) -> Box<dyn StreamHandlerPool> {
            self.make_parameters.lock().unwrap().push((
                resolver,
                cryptde.dup(),
                accountant_sub,
                proxy_client_subs,
                exit_service_rate,
                exit_byte_rate,
            ));
            self.make_results.borrow_mut().remove(0)
        }
    }

    impl StreamHandlerPoolFactoryMock {
        pub fn new() -> StreamHandlerPoolFactoryMock {
            StreamHandlerPoolFactoryMock {
                make_parameters: Arc::new(Mutex::new(vec![])),
                make_results: RefCell::new(vec![]),
            }
        }

        pub fn make_parameters(
            self,
            parameters: &mut Arc<
                Mutex<
                    Vec<(
                        Box<dyn ResolverWrapper>,
                        Box<dyn CryptDE>,
                        Recipient<ReportExitServiceProvidedMessage>,
                        ProxyClientSubs,
                        u64,
                        u64,
                    )>,
                >,
            >,
        ) -> StreamHandlerPoolFactoryMock {
            *parameters = self.make_parameters.clone();
            self
        }

        pub fn make_result(
            self,
            result: Box<dyn StreamHandlerPool>,
        ) -> StreamHandlerPoolFactoryMock {
            self.make_results.borrow_mut().push(result);
            self
        }
    }

    #[test]
    fn is_decentralized_flag_is_passed_through_constructor() {
        let config_factory = |is_decentralized: bool| ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: vec![SocketAddr::V4(
                SocketAddrV4::from_str("1.2.3.4:4560").unwrap(),
            )],
            exit_service_rate: 100,
            exit_byte_rate: 200,
            is_decentralized,
            crashable: false,
            receipt_session_validation_opt: None,
        };

        let zero_hop = ProxyClient::new(config_factory(false));
        let standard = ProxyClient::new(config_factory(true));

        assert_eq!(zero_hop.is_decentralized, false);
        assert_eq!(standard.is_decentralized, true);
    }

    #[test]
    #[should_panic(
        expected = "panic message (processed with: node_lib::sub_lib::utils::crash_request_analyzer)"
    )]
    fn proxy_client_can_be_crashed_properly_but_not_improperly() {
        let proxy_client = ProxyClient::new(ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: vec![SocketAddr::V4(
                SocketAddrV4::from_str("1.2.3.4:4560").unwrap(),
            )],
            exit_service_rate: 100,
            exit_byte_rate: 200,
            is_decentralized: true,
            crashable: true,
            receipt_session_validation_opt: None,
        });

        prove_that_crash_request_handler_is_hooked_up(proxy_client, CRASH_KEY);
    }

    #[test]
    #[should_panic(
        expected = "ProxyClient requires at least one DNS server IP address after the --dns-servers parameter"
    )]
    fn at_least_one_dns_server_must_be_provided() {
        ProxyClient::new(ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: vec![],
            exit_service_rate: 100,
            exit_byte_rate: 200,
            is_decentralized: true,
            crashable: false,
            receipt_session_validation_opt: None,
        });
    }

    #[test]
    fn bind_operates_properly() {
        let system = System::new("bind_initializes_resolver_wrapper_properly");
        let resolver_wrapper = ResolverWrapperMock::new();
        let mut resolver_wrapper_new_parameters_arc: Arc<
            Mutex<Vec<(ResolverConfig, ResolverOpts)>>,
        > = Arc::new(Mutex::new(vec![]));
        let resolver_wrapper_factory = ResolverWrapperFactoryMock::new()
            .new_parameters(&mut resolver_wrapper_new_parameters_arc)
            .new_result(Box::new(resolver_wrapper));
        let pool = StreamHandlerPoolMock::new();
        let mut pool_factory_make_parameters = Arc::new(Mutex::new(vec![]));
        let pool_factory = StreamHandlerPoolFactoryMock::new()
            .make_parameters(&mut pool_factory_make_parameters)
            .make_result(Box::new(pool));
        let peer_actors = peer_actors_builder().build();
        let mut subject = ProxyClient::new(ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: vec![
                SocketAddr::from_str("4.3.2.1:4321").unwrap(),
                SocketAddr::from_str("5.4.3.2:5432").unwrap(),
            ],
            exit_service_rate: 100,
            exit_byte_rate: 200,
            is_decentralized: true,
            crashable: false,
            receipt_session_validation_opt: None,
        });
        subject.resolver_wrapper_factory = Box::new(resolver_wrapper_factory);
        subject.stream_handler_pool_factory = Box::new(pool_factory);
        let subject_addr: Addr<ProxyClient> = subject.start();

        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        System::current().stop_with_code(0);
        system.run();

        let mut resolver_wrapper_new_parameters =
            resolver_wrapper_new_parameters_arc.lock().unwrap();
        let (config, opts) = resolver_wrapper_new_parameters.remove(0);
        assert_eq!(config.domain(), None);
        assert_eq!(config.search(), &[]);
        assert_eq!(
            config.name_servers(),
            &[
                NameServerConfig {
                    socket_addr: SocketAddr::from_str("4.3.2.1:4321").unwrap(),
                    protocol: Protocol::Udp,
                    tls_dns_name: None,
                },
                NameServerConfig {
                    socket_addr: SocketAddr::from_str("5.4.3.2:5432").unwrap(),
                    protocol: Protocol::Udp,
                    tls_dns_name: None,
                },
            ]
        );
        assert_eq!(opts, ResolverOpts::default());
        assert_eq!(resolver_wrapper_new_parameters.is_empty(), true);
    }

    #[test]
    #[should_panic(expected = "StreamHandlerPool unbound")]
    fn panics_if_unbound() {
        let request = ClientRequestPayload_0v1 {
            stream_key: StreamKey::make_meaningless_stream_key(),
            sequenced_packet: SequencedPacket {
                data: b"HEAD http://www.nyan.cat/ HTTP/1.1\r\n\r\n".to_vec(),
                sequence_number: 0,
                last_data: false,
            },
            target_hostname: String::from("target.hostname.com"),
            target_port: 1234,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: PublicKey::new(&b"originator_public_key"[..]),
            dns_attempt_id_opt: None,
            receipt_session_request_opt: None,
        };
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            Some(make_wallet("consuming")),
            route_to_proxy_client(&cryptde.public_key(), cryptde, false),
            request,
            0,
        );
        let system = System::new("panics_if_hopper_is_unbound");
        let subject = ProxyClient::new(ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: dnss(),
            exit_service_rate: 100,
            exit_byte_rate: 200,
            is_decentralized: true,
            crashable: false,
            receipt_session_validation_opt: None,
        });
        let subject_addr: Addr<ProxyClient> = subject.start();

        subject_addr.try_send(package).unwrap();

        System::current().stop_with_code(0);
        system.run();
    }

    #[test]
    fn logs_nonexistent_stream_key_during_dns_resolution_failure() {
        init_test_logging();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let stream_key_inner = stream_key.clone();
        thread::spawn(move || {
            let system = System::new("logs_nonexistent_stream_key_during_dns_resolution_failure");
            let subject = ProxyClient::new(ProxyClientConfig {
                cryptde_pair: CRYPTDE_PAIR.clone(),
                dns_servers: vec![SocketAddr::from_str("1.1.1.1:53").unwrap()],
                exit_service_rate: 0,
                exit_byte_rate: 0,
                is_decentralized: true,
                crashable: false,
                receipt_session_validation_opt: None,
            });
            let subject_addr = subject.start();
            let subject_subs = ProxyClient::make_subs_from(&subject_addr);

            subject_subs
                .dns_resolve_failed
                .try_send(DnsResolveFailure_0v1::new(stream_key_inner))
                .unwrap();

            system.run();
        });
        TestLogHandler::new().await_log_containing(
            "ERROR: ProxyClient: DNS resolution for nonexistent stream failed; identity redacted.",
            1000,
        );
    }

    #[test]
    fn dns_failure_with_unencryptable_originator_key_retires_only_that_stream() {
        init_test_logging();
        let test_name = "dns_failure_with_unencryptable_originator_key_retires_only_that_stream";
        let system = System::new(test_name);
        let (hopper, _, hopper_recording) = make_recorder();
        let peer_actors = peer_actors_builder().hopper(hopper).build();
        let stream_key = StreamKey::make_meaningful_stream_key(test_name);
        let mut subject = ProxyClient::new(ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: vec![SocketAddr::from_str("1.1.1.1:53").unwrap()],
            exit_service_rate: 0,
            exit_byte_rate: 0,
            is_decentralized: true,
            crashable: false,
            receipt_session_validation_opt: None,
        });
        subject.logger = Logger::new(test_name);
        subject.to_hopper = Some(peer_actors.hopper.from_hopper_client);
        subject.stream_contexts.insert(
            stream_key,
            StreamContext {
                return_route: make_meaningless_route(&CRYPTDE_PAIR),
                payload_destination_key: PublicKey::new(&[]),
                paying_wallet: None,
                dns_attempt_id_opt: Some(0),
                receipt_state_opt: None,
                routing_receipt_offers: vec![],
            },
        );
        let subject_addr = subject.start();
        let subject_subs = ProxyClient::make_subs_from(&subject_addr);

        subject_subs
            .dns_resolve_failed
            .try_send(DnsResolveFailure_0v1::for_attempt(stream_key, Some(0)))
            .unwrap();
        subject_subs
            .dns_resolve_failed
            .try_send(DnsResolveFailure_0v1::for_attempt(stream_key, Some(0)))
            .unwrap();

        System::current().stop();
        system.run();
        assert_eq!(hopper_recording.lock().unwrap().len(), 0);
        TestLogHandler::new().exists_log_containing(&format!(
            "ERROR: {test_name}: Could not create DNS-failure CORES package; stream and error details redacted - retiring stream"
        ));
        TestLogHandler::new().exists_log_containing(&format!(
            "ERROR: {test_name}: DNS resolution for nonexistent stream failed; identity redacted."
        ));
    }

    #[test]
    fn stale_dns_failure_does_not_remove_newer_context_or_prevent_current_failure() {
        init_test_logging();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let (hopper, hopper_awaiter, hopper_recording_arc) = make_recorder();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let return_route = make_meaningless_route(&CRYPTDE_PAIR);
        let originator_key = make_meaningless_public_key(&CRYPTDE_PAIR);
        let stream_key_inner = stream_key.clone();
        let return_route_inner = return_route.clone();
        let originator_key_inner = originator_key.clone();
        thread::spawn(move || {
            let system = System::new(
                "stale_dns_failure_does_not_remove_newer_context_or_prevent_current_failure",
            );
            let peer_actors = peer_actors_builder().hopper(hopper).build();
            let mut subject = ProxyClient::new(ProxyClientConfig {
                cryptde_pair: CRYPTDE_PAIR.clone(),
                dns_servers: vec![SocketAddr::from_str("1.1.1.1:53").unwrap()],
                exit_service_rate: 0,
                exit_byte_rate: 0,
                is_decentralized: true,
                crashable: false,
                receipt_session_validation_opt: None,
            });
            subject.stream_contexts.insert(
                stream_key_inner,
                StreamContext {
                    return_route: return_route_inner,
                    payload_destination_key: originator_key_inner,
                    paying_wallet: None,
                    dns_attempt_id_opt: Some(2),
                    receipt_state_opt: None,
                    routing_receipt_offers: vec![],
                },
            );
            let subject_addr = subject.start();
            let subject_subs = ProxyClient::make_subs_from(&subject_addr);

            send_bind_message!(subject_subs, peer_actors);

            subject_subs
                .dns_resolve_failed
                .try_send(DnsResolveFailure_0v1::for_attempt(
                    stream_key_inner,
                    Some(1),
                ))
                .unwrap();

            subject_subs
                .dns_resolve_failed
                .try_send(DnsResolveFailure_0v1::for_attempt(
                    stream_key_inner,
                    Some(2),
                ))
                .unwrap();

            system.run();
        });

        hopper_awaiter.await_message_count(1);

        let message_type: MessageType =
            DnsResolveFailure_0v1::for_attempt(stream_key, Some(2)).into();
        assert_eq!(
            &IncipientCoresPackage::new(cryptde, return_route, message_type, &originator_key)
                .unwrap(),
            hopper_recording_arc
                .lock()
                .unwrap()
                .get_record::<IncipientCoresPackage>(0)
        );
        TestLogHandler::new().await_log_containing(
            "WARN: ProxyClient: Discarding stale DNS resolution failure for a redacted stream: expected attempt Some(2), received Some(1)",
            1000,
        );
    }

    #[test]
    fn data_from_hopper_is_relayed_to_stream_handler_pool() {
        let request = ClientRequestPayload_0v1 {
            stream_key: StreamKey::make_meaningless_stream_key(),
            sequenced_packet: SequencedPacket {
                data: b"inbound data".to_vec(),
                sequence_number: 0,
                last_data: false,
            },
            target_hostname: "booga.com".to_string(),
            target_port: 0,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: PublicKey::new(&b"originator"[..]),
            dns_attempt_id_opt: None,
            receipt_session_request_opt: None,
        };
        let key1 = make_meaningless_public_key(&CRYPTDE_PAIR);
        let key2 = make_meaningless_public_key(&CRYPTDE_PAIR);
        let route = make_one_way_route_to_proxy_client(vec![&key1, &key2], &CRYPTDE_PAIR);
        let package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            Some(make_wallet("consuming")),
            route,
            request.clone().into(),
            0,
        );
        let hopper = Recorder::new();

        let system = System::new("data_from_hopper_is_relayed_to_stream_handler_pool");
        let peer_actors = peer_actors_builder().hopper(hopper).build();
        let mut process_package_parameters = Arc::new(Mutex::new(vec![]));
        let pool = Box::new(
            StreamHandlerPoolMock::new()
                .process_package_parameters(&mut process_package_parameters),
        );
        let pool_factory = StreamHandlerPoolFactoryMock::new().make_result(pool);
        let resolver = ResolverWrapperMock::new()
            .lookup_ip_success(vec![IpAddr::from_str("4.3.2.1").unwrap()]);
        let resolver_factory = ResolverWrapperFactoryMock::new().new_result(Box::new(resolver));
        let mut subject = ProxyClient::new(ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: dnss(),
            exit_service_rate: 100,
            exit_byte_rate: 200,
            is_decentralized: true,
            crashable: false,
            receipt_session_validation_opt: None,
        });
        subject.resolver_wrapper_factory = Box::new(resolver_factory);
        subject.stream_handler_pool_factory = Box::new(pool_factory);
        let subject_addr: Addr<ProxyClient> = subject.start();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(package).unwrap();

        System::current().stop_with_code(0);
        system.run();
        let parameter = process_package_parameters.lock().unwrap().remove(0);
        assert_eq!(parameter, (request, Some(make_wallet("consuming")),));
    }

    #[test]
    fn refuse_to_provide_exit_services_with_no_paying_wallet() {
        init_test_logging();
        let request = ClientRequestPayload_0v1 {
            stream_key: StreamKey::make_meaningless_stream_key(),
            sequenced_packet: SequencedPacket {
                data: b"inbound data".to_vec(),
                sequence_number: 0,
                last_data: false,
            },
            target_hostname: "booga.com".to_string(),
            target_port: 0,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: PublicKey::new(&b"originator"[..]),
            dns_attempt_id_opt: None,
            receipt_session_request_opt: None,
        };
        let package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            None,
            make_meaningless_route(&CRYPTDE_PAIR),
            request,
            0,
        );
        let hopper = Recorder::new();

        let system = System::new("refuse_to_provide_exit_services_with_no_paying_wallet");
        let peer_actors = peer_actors_builder().hopper(hopper).build();
        let mut process_package_parameters = Arc::new(Mutex::new(vec![]));
        let pool = Box::new(
            StreamHandlerPoolMock::new()
                .process_package_parameters(&mut process_package_parameters),
        );
        let pool_factory = StreamHandlerPoolFactoryMock::new().make_result(pool);
        let resolver = ResolverWrapperMock::new()
            .lookup_ip_success(vec![IpAddr::from_str("4.3.2.1").unwrap()]);
        let resolver_factory = ResolverWrapperFactoryMock::new().new_result(Box::new(resolver));
        let mut subject = ProxyClient::new(ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: dnss(),
            exit_service_rate: rate_pack_exit(100),
            exit_byte_rate: rate_pack_exit_byte(100),
            is_decentralized: true,
            crashable: false,
            receipt_session_validation_opt: None,
        });
        subject.resolver_wrapper_factory = Box::new(resolver_factory);
        subject.stream_handler_pool_factory = Box::new(pool_factory);
        let subject_addr: Addr<ProxyClient> = subject.start();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(package).unwrap();

        System::current().stop();
        system.run();
        assert_eq!(0, process_package_parameters.lock().unwrap().len());
        TestLogHandler::new().exists_log_containing("WARN: ProxyClient: Refusing to provide exit services for CORES package with 12-byte payload without paying wallet");
    }

    #[test]
    fn does_provide_zero_hop_exit_services_with_no_paying_wallet() {
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let alias_cryptde = CRYPTDE_PAIR.alias.as_ref();
        let request = ClientRequestPayload_0v1 {
            stream_key: StreamKey::make_meaningless_stream_key(),
            sequenced_packet: SequencedPacket {
                data: b"inbound data".to_vec(),
                sequence_number: 0,
                last_data: false,
            },
            target_hostname: "booga.com".to_string(),
            target_port: 0,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: alias_cryptde.public_key().clone(),
            dns_attempt_id_opt: None,
            receipt_session_request_opt: None,
        };
        let zero_hop_remaining_route = Route::one_way(
            RouteSegment::new(
                vec![main_cryptde.public_key(), main_cryptde.public_key()],
                Component::ProxyServer,
            ),
            main_cryptde,
            None,
            Some(Chain::EthRopsten.rec().contract),
        )
        .unwrap();
        let package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            None,
            zero_hop_remaining_route,
            request.clone().into(),
            0,
        );
        let hopper = Recorder::new();

        let system = System::new("unparseable_request_results_in_log_and_no_response");
        let peer_actors = peer_actors_builder().hopper(hopper).build();
        let mut process_package_parameters = Arc::new(Mutex::new(vec![]));
        let pool = Box::new(
            StreamHandlerPoolMock::new()
                .process_package_parameters(&mut process_package_parameters),
        );
        let pool_factory = StreamHandlerPoolFactoryMock::new().make_result(pool);
        let resolver = ResolverWrapperMock::new()
            .lookup_ip_success(vec![IpAddr::from_str("4.3.2.1").unwrap()]);
        let resolver_factory = ResolverWrapperFactoryMock::new().new_result(Box::new(resolver));
        let mut subject = ProxyClient::new(ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: dnss(),
            exit_service_rate: rate_pack_exit(100),
            exit_byte_rate: rate_pack_exit_byte(100),
            is_decentralized: false,
            crashable: false,
            receipt_session_validation_opt: None,
        });
        subject.resolver_wrapper_factory = Box::new(resolver_factory);
        subject.stream_handler_pool_factory = Box::new(pool_factory);
        let subject_addr: Addr<ProxyClient> = subject.start();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(package).unwrap();

        System::current().stop();
        system.run();
        let parameter = process_package_parameters.lock().unwrap().remove(0);
        assert_eq!(parameter, (request, None,));
    }

    #[test]
    fn inbound_server_data_is_translated_to_cores_packages() {
        init_test_logging();
        let test_name = "inbound_server_data_is_translated_to_cores_packages";
        let (hopper, _, hopper_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let stream_key = StreamKey::make_meaningful_stream_key(test_name);
        let data: &[u8] = b"An honest politician is one who, when he is bought, will stay bought.";
        let system = System::new(test_name);
        let route = make_meaningless_route(&CRYPTDE_PAIR);
        let mut subject = ProxyClient::new(ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: vec![SocketAddr::from_str("8.7.6.5:4321").unwrap()],
            exit_service_rate: 100,
            exit_byte_rate: 200,
            is_decentralized: true,
            crashable: false,
            receipt_session_validation_opt: None,
        });
        subject.stream_contexts.insert(
            stream_key.clone(),
            StreamContext {
                return_route: route.clone(),
                payload_destination_key: PublicKey::new(&b"abcd"[..]),
                paying_wallet: Some(make_wallet("paying")),
                dns_attempt_id_opt: None,
                receipt_state_opt: None,
                routing_receipt_offers: vec![],
            },
        );
        subject.logger = Logger::new(test_name);
        let subject_addr: Addr<ProxyClient> = subject.start();
        let peer_actors = peer_actors_builder()
            .hopper(hopper)
            .accountant(accountant)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        let before = SystemTime::now();

        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: false,
                sequence_number: 1234,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data),
            })
            .unwrap();
        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: true,
                sequence_number: 1235,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data),
            })
            .unwrap();
        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: false,
                sequence_number: 1236,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data),
            })
            .unwrap();
        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: true,
                sequence_number: 1237,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data),
            })
            .unwrap();

        System::current().stop_with_code(0);
        system.run();
        let after = SystemTime::now();
        let hopper_recording = hopper_recording_arc.lock().unwrap();
        assert_eq!(
            hopper_recording.get_record::<IncipientCoresPackage>(0),
            &IncipientCoresPackage::new(
                CRYPTDE_PAIR.main.as_ref(),
                route.clone(),
                MessageType::ClientResponse(VersionedData::new(
                    &crate::sub_lib::migrations::client_response_payload::MIGRATIONS,
                    &ClientResponsePayload_0v1 {
                        stream_key: stream_key.clone(),
                        sequenced_packet: SequencedPacket {
                            data: Vec::from(data),
                            sequence_number: 1234,
                            last_data: false,
                        },
                    }
                )),
                &PublicKey::new(&b"abcd"[..]),
            )
            .unwrap()
        );
        assert_eq!(
            hopper_recording.get_record::<IncipientCoresPackage>(1),
            &IncipientCoresPackage::new(
                CRYPTDE_PAIR.main.as_ref(),
                route.clone(),
                MessageType::ClientResponse(VersionedData::new(
                    &crate::sub_lib::migrations::client_response_payload::MIGRATIONS,
                    &ClientResponsePayload_0v1 {
                        stream_key: stream_key.clone(),
                        sequenced_packet: SequencedPacket {
                            data: Vec::from(data),
                            sequence_number: 1235,
                            last_data: true,
                        },
                    }
                )),
                &PublicKey::new(&b"abcd"[..]),
            )
            .unwrap()
        );
        assert_eq!(hopper_recording.len(), 2);

        let accountant_recording = accountant_recording_arc.lock().unwrap();
        let accountant_record =
            accountant_recording.get_record::<ReportExitServiceProvidedMessage>(0);
        check_timestamp(before, accountant_record.timestamp, after);
        assert_eq!(
            accountant_record,
            &ReportExitServiceProvidedMessage {
                timestamp: accountant_record.timestamp,
                paying_wallet: make_wallet("paying"),
                payload_size: data.len(),
                service_rate: 100,
                byte_rate: 200,
            }
        );
        let accountant_record =
            accountant_recording.get_record::<ReportExitServiceProvidedMessage>(1);
        check_timestamp(before, accountant_record.timestamp, after);
        assert_eq!(
            accountant_record,
            &ReportExitServiceProvidedMessage {
                timestamp: accountant_record.timestamp,
                paying_wallet: make_wallet("paying"),
                payload_size: data.len(),
                service_rate: 100,
                byte_rate: 200,
            }
        );
        assert_eq!(accountant_recording.len(), 2);
        let tlh = TestLogHandler::new();
        tlh.exists_log_containing(format!("ERROR: {test_name}: Received InboundServerData for an unknown stream: identity, source and contents redacted; sequence 1236, length {} - ignoring", data.len()).as_str());
        tlh.exists_log_containing(format!("ERROR: {test_name}: Received InboundServerData (last_data) for an unknown stream: identity, source and contents redacted; sequence 1237, length {} - ignoring", data.len()).as_str());
    }

    #[test]
    fn metered_exit_response_produces_a_signed_offer_without_legacy_double_booking() {
        let test_name =
            "metered_exit_response_produces_a_signed_offer_without_legacy_double_booking";
        let (hopper, _, hopper_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let stream_key = StreamKey::make_meaningful_stream_key(test_name);
        let consumer_cryptde = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let provider_cryptde_pair = CryptDEPair::new(
            Box::new(CryptDEReal::new(TEST_DEFAULT_CHAIN)),
            Box::new(CryptDEReal::new(TEST_DEFAULT_CHAIN)),
        );
        let payer_wallet = make_paying_wallet(b"metered proxy-client payer");
        let now_unix_s = ProxyClient::unix_time_now().unwrap();
        let policy = ReceiptSessionPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
            payer_wallet.address(),
            consumer_cryptde.public_key().clone(),
            1_000_000,
            now_unix_s.saturating_sub(1),
            now_unix_s + 600,
            [0x55; 32],
        );
        let request =
            ReceiptSessionRequest::new(policy.authorize(&payer_wallet).unwrap(), [0x66; 32])
                .unwrap();
        let response_data = b"measured exit response".to_vec();
        let routing_receipt_offers = vec![CryptData::new(b"encrypted forward hop receipt")];
        let system = System::new(test_name);
        let mut subject = ProxyClient::new(ProxyClientConfig {
            cryptde_pair: provider_cryptde_pair.clone(),
            dns_servers: dnss(),
            exit_service_rate: 100,
            exit_byte_rate: 200,
            is_decentralized: true,
            crashable: false,
            receipt_session_validation_opt: Some(ReceiptSessionValidation {
                chain_id: TEST_DEFAULT_CHAIN.rec().num_chain_id,
                settlement_contract: TEST_DEFAULT_CHAIN.rec().contract,
            }),
        });
        subject.stream_contexts.insert(
            stream_key,
            StreamContext {
                return_route: make_meaningless_route(&provider_cryptde_pair),
                payload_destination_key: consumer_cryptde.public_key().clone(),
                paying_wallet: Some(make_wallet("metered payer")),
                dns_attempt_id_opt: None,
                receipt_state_opt: Some(ProviderReceiptState {
                    request: request.clone(),
                    last_signed_receipt_opt: None,
                    pending_request_payload_size: 17,
                    pending_request_service_units: 2,
                }),
                routing_receipt_offers: routing_receipt_offers.clone(),
            },
        );
        let subject_addr = subject.start();
        let peer_actors = peer_actors_builder()
            .hopper(hopper)
            .accountant(accountant)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        subject_addr
            .try_send(InboundServerData {
                stream_key,
                last_data: true,
                sequence_number: 7,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: response_data.clone(),
            })
            .unwrap();

        System::current().stop_with_code(0);
        system.run();

        let hopper_recording = hopper_recording_arc.lock().unwrap();
        assert_eq!(hopper_recording.len(), 1);
        let metered_package = hopper_recording.get_record::<IncipientCoresPackage>(0);
        assert_eq!(
            metered_package.routing_receipt_offers,
            routing_receipt_offers
        );
        let decoded = decodex::<MessageType>(&consumer_cryptde, &metered_package.payload).unwrap();
        let metered = match decoded {
            MessageType::MeteredClientResponse(versioned) => {
                MeteredClientResponsePayload_0v1::try_from(versioned).unwrap()
            }
            other => panic!("Unexpected metered response payload: {:?}", other),
        };
        assert_eq!(metered.response.stream_key, stream_key);
        assert_eq!(metered.response.sequenced_packet.data, response_data);
        let offer = metered.receipt_offer;
        offer
            .signed_receipt
            .verify(provider_cryptde_pair.main.as_ref())
            .unwrap();
        let receipt = offer.signed_receipt.receipt;
        assert_eq!(receipt.route_epoch, request.route_epoch);
        assert_eq!(receipt.sequence, 7);
        assert_eq!(
            receipt.provider_public_key,
            provider_cryptde_pair.main.public_key().clone()
        );
        assert_eq!(receipt.payload_size, response_data.len() as u64 + 17);
        assert_eq!(receipt.service_units, 3);
        assert_eq!(receipt.service_rate, 100);
        assert_eq!(receipt.byte_rate, 200);
        assert_eq!(accountant_recording_arc.lock().unwrap().len(), 0);
    }

    #[test]
    fn inbound_server_data_without_paying_wallet_does_not_report_exit_service() {
        init_test_logging();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let data: &[u8] = b"An honest politician is one who, when he is bought, will stay bought.";
        let system =
            System::new("inbound_server_data_without_paying_wallet_does_not_report_exit_service");
        let mut subject = ProxyClient::new(ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: vec![SocketAddr::from_str("8.7.6.5:4321").unwrap()],
            exit_service_rate: 100,
            exit_byte_rate: 200,
            is_decentralized: true,
            crashable: false,
            receipt_session_validation_opt: None,
        });
        subject.stream_contexts.insert(
            stream_key.clone(),
            StreamContext {
                return_route: make_meaningless_route(&CRYPTDE_PAIR),
                payload_destination_key: PublicKey::new(&b"abcd"[..]),
                paying_wallet: None,
                dns_attempt_id_opt: None,
                receipt_state_opt: None,
                routing_receipt_offers: vec![],
            },
        );
        let subject_addr: Addr<ProxyClient> = subject.start();
        let peer_actors = peer_actors_builder().accountant(accountant).build();

        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: false,
                sequence_number: 1234,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data),
            })
            .unwrap();

        System::current().stop_with_code(0);
        system.run();
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        assert_eq!(accountant_recording.len(), 0);
        TestLogHandler::new().exists_log_containing(
            format!(
                "DEBUG: ProxyClient: Relayed {}-byte response without paying wallet for free",
                data.len()
            )
            .as_str(),
        );
    }

    #[test]
    fn error_creating_incipient_cores_package_is_logged_and_dropped() {
        init_test_logging();
        let (hopper, _, hopper_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let data: &[u8] = b"An honest politician is one who, when he is bought, will stay bought.";
        let system = System::new("error_creating_incipient_cores_package_is_logged_and_dropped");
        let mut subject = ProxyClient::new(ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: vec![SocketAddr::from_str("8.7.6.5:4321").unwrap()],
            exit_service_rate: 100,
            exit_byte_rate: 200,
            is_decentralized: true,
            crashable: false,
            receipt_session_validation_opt: None,
        });
        subject.stream_contexts.insert(
            stream_key.clone(),
            StreamContext {
                return_route: make_meaningless_route(&CRYPTDE_PAIR),
                payload_destination_key: PublicKey::new(&[]),
                paying_wallet: Some(make_wallet("consuming")),
                dns_attempt_id_opt: None,
                receipt_state_opt: None,
                routing_receipt_offers: vec![],
            },
        );
        let subject_addr: Addr<ProxyClient> = subject.start();
        let peer_actors = peer_actors_builder()
            .hopper(hopper)
            .accountant(accountant)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: false,
                sequence_number: 1234,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data),
            })
            .unwrap();

        System::current().stop_with_code(0);
        system.run();
        let hopper_recording = hopper_recording_arc.lock().unwrap();
        assert_eq!(hopper_recording.len(), 0);
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        assert_eq!(accountant_recording.len(), 0);
        TestLogHandler::new().exists_log_containing(format!("ERROR: ProxyClient: Could not create CORES package for {}-byte response, seq 1234; source and error details redacted - ignoring", data.len()).as_str());
    }

    #[test]
    fn new_return_route_overwrites_existing_return_route() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let (hopper, _, hopper_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let data: &[u8] = b"An honest politician is one who, when he is bought, will stay bought.";
        let system = System::new("new_return_route_overwrites_existing_return_route");
        let mut subject = ProxyClient::new(ProxyClientConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            dns_servers: vec![SocketAddr::from_str("8.7.6.5:4321").unwrap()],
            exit_service_rate: 100,
            exit_byte_rate: 200,
            is_decentralized: true,
            crashable: false,
            receipt_session_validation_opt: None,
        });
        let mut process_package_params_arc = Arc::new(Mutex::new(vec![]));
        let pool = StreamHandlerPoolMock::new()
            .process_package_parameters(&mut process_package_params_arc);
        let pool_factory = StreamHandlerPoolFactoryMock::new().make_result(Box::new(pool));
        let old_return_route = Route {
            hops: vec![CryptData::new(&[1, 2, 3, 4])],
        };
        let new_return_route = make_meaningless_route(&CRYPTDE_PAIR);
        let originator_public_key = PublicKey::new(&[4, 3, 2, 1]);
        subject.stream_contexts.insert(
            stream_key.clone(),
            StreamContext {
                return_route: old_return_route,
                payload_destination_key: originator_public_key.clone(),
                paying_wallet: Some(make_wallet("consuming")),
                dns_attempt_id_opt: None,
                receipt_state_opt: None,
                routing_receipt_offers: vec![],
            },
        );
        subject.stream_handler_pool_factory = Box::new(pool_factory);
        let subject_addr: Addr<ProxyClient> = subject.start();
        let peer_actors = peer_actors_builder()
            .hopper(hopper)
            .accountant(accountant)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        let payload = ClientRequestPayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket {
                data: vec![],
                sequence_number: 0,
                last_data: false,
            },
            target_hostname: "booga.com".to_string(),
            target_port: 0,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: originator_public_key.clone(),
            dns_attempt_id_opt: None,
            receipt_session_request_opt: None,
        };
        let before = SystemTime::now();

        subject_addr
            .try_send(ExpiredCoresPackage::new(
                SocketAddr::from_str("2.3.4.5:1235").unwrap(),
                Some(make_wallet("gnimusnoc")),
                new_return_route.clone(),
                payload.clone().into(),
                0,
            ))
            .unwrap();

        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: false,
                sequence_number: 1234,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data.clone()),
            })
            .unwrap();
        System::current().stop_with_code(0);
        system.run();
        let after = SystemTime::now();
        let mut process_package_params = process_package_params_arc.lock().unwrap();
        let (actual_payload, paying_wallet_opt) = process_package_params.remove(0);
        assert_eq!(actual_payload, payload);
        assert_eq!(paying_wallet_opt, Some(make_wallet("gnimusnoc")));
        let hopper_recording = hopper_recording_arc.lock().unwrap();
        let expected_icp = IncipientCoresPackage::new(
            cryptde,
            new_return_route,
            MessageType::ClientResponse(VersionedData::new(
                &crate::sub_lib::migrations::client_response_payload::MIGRATIONS,
                &ClientResponsePayload_0v1 {
                    stream_key,
                    sequenced_packet: SequencedPacket {
                        data: Vec::from(data.clone()),
                        sequence_number: 1234,
                        last_data: false,
                    },
                },
            )),
            &originator_public_key,
        )
        .unwrap();

        assert_eq!(
            hopper_recording.get_record::<IncipientCoresPackage>(0),
            &expected_icp.clone()
        );
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        let accountant_record =
            accountant_recording.get_record::<ReportExitServiceProvidedMessage>(0);
        check_timestamp(before, accountant_record.timestamp, after);
        assert_eq!(
            accountant_record,
            &ReportExitServiceProvidedMessage {
                timestamp: accountant_record.timestamp,
                paying_wallet: make_wallet("gnimusnoc"),
                payload_size: data.len(),
                service_rate: 100,
                byte_rate: 200,
            }
        )
    }
}
