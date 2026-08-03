// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

pub mod client_request_payload_factory;
pub mod http_protocol_pack;
pub mod protocol_pack;
pub mod receipt_session;
pub mod receipt_session_recovery;
pub mod server_impersonator_http;
pub mod server_impersonator_tls;
pub mod tls_protocol_pack;

use crate::accountant::db_access_objects::receipt_acknowledgement_outbox_dao::ReceiptAcknowledgementOutboxDao;
use crate::bootstrapper::CryptDEPair;
use crate::proxy_server::client_request_payload_factory::{
    ClientRequestPayloadFactory, ClientRequestPayloadFactoryReal,
};
use crate::proxy_server::http_protocol_pack::HttpProtocolPack;
use crate::proxy_server::protocol_pack::{from_ibcd, from_protocol, ProtocolPack};
use crate::proxy_server::receipt_session::{
    ReceiptSessionConfig, ReceiptSessionManager, ReceiptSessionManagerError,
    ReceiptSessionRecoveryStore, ReceiptSessionStatus, RoutingReceiptQuote,
};
use crate::stream_messages::NonClandestineAttributes;
use crate::stream_messages::RemovedStreamType;
use crate::sub_lib::accountant::RoutingServiceConsumed;
use crate::sub_lib::accountant::{
    ExitServiceConsumed, ReportRoutingServicesConsumedMessage, ReportServicesConsumedMessage,
};
use crate::sub_lib::bidi_hashmap::BidiHashMap;
use crate::sub_lib::cryptde::CryptDE;
use crate::sub_lib::cryptde::PublicKey;
use crate::sub_lib::dispatcher::InboundClientData;
use crate::sub_lib::dispatcher::{Endpoint, StreamShutdownMsg};
use crate::sub_lib::hopper::{ExpiredCoresPackage, IncipientCoresPackage, MessageType};
use crate::sub_lib::host::Host;
use crate::sub_lib::neighborhood::ExpectedServices;
use crate::sub_lib::neighborhood::{ExpectedService, UpdateNodeRecordMetadataMessage};
use crate::sub_lib::neighborhood::{NRMetadataChange, RouteQueryMessage};
use crate::sub_lib::neighborhood::{
    RouteQueryResponse, RouteUseFailedMessage, RouteUseSucceededMessage,
};
use crate::sub_lib::peer_actors::BindMessage;
use crate::sub_lib::proxy_client::{
    ClientResponsePayload_0v1, DnsResolveFailure_0v1, MeteredClientResponsePayload_0v1,
};
use crate::sub_lib::proxy_server::ProxyServerSubs;
use crate::sub_lib::proxy_server::StreamKeyPurge;
use crate::sub_lib::proxy_server::{
    AddRouteResultMessage, ClientRequestPayload_0v1, ProxyProtocol, RecordExitRequestForReceipt,
    RetryReceiptAcknowledgements,
};
use crate::sub_lib::route::Route;
use crate::sub_lib::sequence_buffer::MAX_SEQUENCE_REORDER_WINDOW;
use crate::sub_lib::service_receipt::ServiceReceiptOfferPayload_0v1;
use crate::sub_lib::stream_handler_pool::TransmitDataMsg;
use crate::sub_lib::stream_key::StreamKey;
use crate::sub_lib::utils::{handle_ui_crash_request, MessageScheduler, NODE_MAILBOX_CAPACITY};
use actix::Context;
use actix::Handler;
use actix::Recipient;
use actix::{Actor, MailboxError};
use actix::{Addr, AsyncContext};
use masq_lib::constants::{RECEIPT_SESSION_ERROR, TLS_PORT};
use masq_lib::logger::Logger;
use masq_lib::messages::{
    FromMessageBody, ToMessageBody, UiReceiptSessionActivateRequest,
    UiReceiptSessionActivateResponse, UiReceiptSessionProposalRequest,
    UiReceiptSessionProposalResponse, UiReceiptSessionStatusRequest,
    UiReceiptSessionStatusResponse, UiReceiptSessionStopRequest, UiReceiptSessionStopResponse,
};
use masq_lib::ui_gateway::{
    MessageBody, MessagePath, MessageTarget, NodeFromUiMessage, NodeToUiMessage,
};
use masq_lib::utils::MutabilityConflictHelper;
use regex::Regex;
use rustc_hex::ToHex;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio::prelude::Future;

pub const CRASH_KEY: &str = "PROXYSERVER";
pub const STREAM_KEY_PURGE_DELAY: Duration = Duration::from_secs(30);
pub const DNS_FAILURE_RETRIES: usize = 3;
pub const MAX_PENDING_ROUTE_PACKETS_PER_STREAM: usize = 32;
pub const MAX_PENDING_ROUTE_BYTES_PER_STREAM: usize = 1_048_576;
pub const MAX_PENDING_RECEIPT_OFFERS: usize = 4096;
pub const MAX_RECEIPT_ACKNOWLEDGEMENT_RECOVERY_BATCH: usize = 64;
pub const RECEIPT_ACKNOWLEDGEMENT_RETRY_DELAY: Duration = Duration::from_secs(30);
pub const ROUTE_ACTIVITY_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

struct ProxyServerOutSubs {
    dispatcher: Recipient<TransmitDataMsg>,
    hopper: Recipient<IncipientCoresPackage>,
    accountant: Recipient<ReportServicesConsumedMessage>,
    routing_accountant: Recipient<ReportRoutingServicesConsumedMessage>,
    route_source: Recipient<RouteQueryMessage>,
    route_use_failed: Recipient<RouteUseFailedMessage>,
    route_use_succeeded: Recipient<RouteUseSucceededMessage>,
    update_node_record_metadata: Recipient<UpdateNodeRecordMetadataMessage>,
    stream_shutdown_sub: Recipient<StreamShutdownMsg>,
    route_result_sub: Recipient<AddRouteResultMessage>,
    schedule_stream_key_purge: Recipient<MessageScheduler<StreamKeyPurge>>,
    retry_receipt_acknowledgements: Recipient<RetryReceiptAcknowledgements>,
    record_exit_request_for_receipt: Recipient<RecordExitRequestForReceipt>,
    ui_gateway: Recipient<NodeToUiMessage>,
}

struct PendingRouteRequest {
    route_request_id: u64,
    queued_payload_bytes: usize,
    packets: Vec<TransmitToHopperArgs>,
}

#[derive(Clone, Debug)]
struct ResponseSequenceReplayWindow {
    next_expected_sequence: u64,
    sequence_space_exhausted: bool,
    seen_out_of_order: HashSet<u64>,
}

impl Default for ResponseSequenceReplayWindow {
    fn default() -> Self {
        Self {
            next_expected_sequence: 0,
            sequence_space_exhausted: false,
            seen_out_of_order: HashSet::new(),
        }
    }
}

impl ResponseSequenceReplayWindow {
    fn admit(&mut self, sequence: u64) -> Result<(), &'static str> {
        if self.sequence_space_exhausted {
            return Err("the response sequence space is exhausted");
        }
        if sequence < self.next_expected_sequence || self.seen_out_of_order.contains(&sequence) {
            return Err("the response sequence is duplicate or stale");
        }
        if sequence - self.next_expected_sequence >= MAX_SEQUENCE_REORDER_WINDOW {
            return Err("the response sequence exceeds the bounded reorder window");
        }
        self.seen_out_of_order.insert(sequence);
        while self.seen_out_of_order.remove(&self.next_expected_sequence) {
            match self.next_expected_sequence.checked_add(1) {
                Some(next) => self.next_expected_sequence = next,
                None => {
                    self.sequence_space_exhausted = true;
                    break;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct StreamInfo {
    tunneled_host_opt: Option<String>,
    dns_failure_retry_opt: Option<DNSFailureRetry>,
    route_opt: Option<RouteQueryResponse>,
    protocol_opt: Option<ProxyProtocol>,
    browser_proxy_sequence_offset: bool,
    response_sequence_replay_window: ResponseSequenceReplayWindow,
    request_started_at_opt: Option<SystemTime>,
    time_to_live_opt: Option<SystemTime>,
    route_success_metadata_reported: bool,
}

/// Runtime-global, memory-only admission control for aggregate route-activity proofs. It stores
/// only a monotonic timestamp: never a stream, destination, route, peer, or traffic counter.
#[derive(Debug, Default)]
struct RouteActivityHeartbeat {
    last_emitted_at_opt: Option<Instant>,
}

impl RouteActivityHeartbeat {
    fn should_emit(&mut self, now: Instant) -> bool {
        let interval_elapsed = self.last_emitted_at_opt.map_or(true, |last_emitted_at| {
            now.checked_duration_since(last_emitted_at)
                .map_or(false, |elapsed| {
                    elapsed >= ROUTE_ACTIVITY_HEARTBEAT_INTERVAL
                })
        });
        if interval_elapsed {
            self.last_emitted_at_opt = Some(now);
        }
        interval_elapsed
    }
}

pub struct ProxyServer {
    subs: Option<ProxyServerOutSubs>,
    client_request_payload_factory: Box<dyn ClientRequestPayloadFactory>,
    stream_key_factory: Box<dyn StreamKeyFactory>,
    keys_and_addrs: BidiHashMap<StreamKey, SocketAddr>,
    stream_info: HashMap<StreamKey, StreamInfo>,
    pending_route_requests: HashMap<StreamKey, PendingRouteRequest>,
    is_decentralized: bool,
    consuming_wallet_balance: Option<i64>,
    cryptde_pair: CryptDEPair,
    crashable: bool,
    logger: Logger,
    inbound_client_data_helper_opt: Option<Box<dyn IBCDHelper>>,
    stream_key_purge_delay: Duration,
    next_return_route_id: Cell<u32>,
    next_route_request_id: Cell<u64>,
    is_running_in_integration_test: bool,
    receipt_session_manager_opt: Option<ReceiptSessionManager>,
    pending_receipt_offers: HashMap<[u8; 32], ServiceReceiptOfferPayload_0v1>,
    receipt_acknowledgement_outbox_opt:
        Option<Arc<Mutex<Box<dyn ReceiptAcknowledgementOutboxDao>>>>,
    receipt_acknowledgement_retry_scheduled: bool,
    route_activity_heartbeat: RouteActivityHeartbeat,
}

impl Actor for ProxyServer {
    type Context = Context<Self>;
}

impl Handler<BindMessage> for ProxyServer {
    type Result = ();

    fn handle(&mut self, msg: BindMessage, ctx: &mut Self::Context) -> Self::Result {
        ctx.set_mailbox_capacity(NODE_MAILBOX_CAPACITY);
        let subs = ProxyServerOutSubs {
            dispatcher: msg.peer_actors.dispatcher.from_dispatcher_client,
            hopper: msg.peer_actors.hopper.from_hopper_client,
            accountant: msg.peer_actors.accountant.report_services_consumed,
            routing_accountant: msg.peer_actors.accountant.report_routing_services_consumed,
            route_source: msg.peer_actors.neighborhood.route_query,
            route_use_failed: msg.peer_actors.neighborhood.route_use_failed,
            route_use_succeeded: msg.peer_actors.neighborhood.route_use_succeeded,
            update_node_record_metadata: msg.peer_actors.neighborhood.update_node_record_metadata,
            stream_shutdown_sub: msg.peer_actors.proxy_server.stream_shutdown_sub,
            route_result_sub: msg.peer_actors.proxy_server.route_result_sub,
            schedule_stream_key_purge: msg.peer_actors.proxy_server.schedule_stream_key_purge,
            retry_receipt_acknowledgements: msg
                .peer_actors
                .proxy_server
                .retry_receipt_acknowledgements,
            record_exit_request_for_receipt: msg
                .peer_actors
                .proxy_server
                .record_exit_request_for_receipt,
            ui_gateway: msg.peer_actors.ui_gateway.node_to_ui_message_sub,
        };
        self.subs = Some(subs);
        self.replay_persisted_receipt_acknowledgements();
    }
}

impl Handler<RetryReceiptAcknowledgements> for ProxyServer {
    type Result = ();

    fn handle(
        &mut self,
        msg: RetryReceiptAcknowledgements,
        ctx: &mut Self::Context,
    ) -> Self::Result {
        if msg.schedule_after_delay {
            if !self.receipt_acknowledgement_retry_scheduled {
                self.receipt_acknowledgement_retry_scheduled = true;
                ctx.notify_later(
                    RetryReceiptAcknowledgements {
                        schedule_after_delay: false,
                    },
                    RECEIPT_ACKNOWLEDGEMENT_RETRY_DELAY,
                );
            }
        } else {
            self.receipt_acknowledgement_retry_scheduled = false;
            self.replay_persisted_receipt_acknowledgements();
        }
    }
}

impl Handler<RecordExitRequestForReceipt> for ProxyServer {
    type Result = ();

    fn handle(
        &mut self,
        msg: RecordExitRequestForReceipt,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let now_unix_s = match Self::unix_time_now() {
            Ok(now_unix_s) => now_unix_s,
            Err(error) => {
                warning!(self.logger, "Cannot meter exit request: {}", error);
                return;
            }
        };
        if let Some(manager) = self.receipt_session_manager_opt.as_mut() {
            if let Err(error) =
                manager.record_exit_request(&msg.stream_key, msg.payload_size, now_unix_s)
            {
                warning!(
                    self.logger,
                    "Cannot correlate exit request with receipt session: {}",
                    error
                );
            }
            if let Err(error) = manager.record_routing_request(
                &msg.stream_key,
                msg.routing_payload_size,
                now_unix_s,
            ) {
                warning!(
                    self.logger,
                    "Cannot correlate routing request with receipt session: {}",
                    error
                );
            }
        }
    }
}

impl Handler<InboundClientData> for ProxyServer {
    type Result = ();

    fn handle(&mut self, msg: InboundClientData, _ctx: &mut Self::Context) -> Self::Result {
        if msg.is_connect() {
            self.tls_connect(&msg);
        } else if let Err(e) =
            // NOTE: I removed a 'false' parameter here for retire_stream_key because I think it was wrong.
            self.help(|helper, proxy| helper.handle_normal_client_data(proxy, msg))
        {
            error!(self.logger, "{}", e)
        }
    }
}

impl Handler<AddRouteResultMessage> for ProxyServer {
    type Result = ();

    fn handle(&mut self, msg: AddRouteResultMessage, _ctx: &mut Self::Context) -> Self::Result {
        self.handle_add_route_result_message(msg)
    }
}

impl Handler<ExpiredCoresPackage<DnsResolveFailure_0v1>> for ProxyServer {
    type Result = ();

    fn handle(
        &mut self,
        msg: ExpiredCoresPackage<DnsResolveFailure_0v1>,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        self.handle_dns_resolve_failure(&msg)
    }
}

impl Handler<ExpiredCoresPackage<ClientResponsePayload_0v1>> for ProxyServer {
    type Result = ();

    fn handle(
        &mut self,
        msg: ExpiredCoresPackage<ClientResponsePayload_0v1>,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        self.handle_client_response_payload(msg)
    }
}

impl Handler<ExpiredCoresPackage<MeteredClientResponsePayload_0v1>> for ProxyServer {
    type Result = ();

    fn handle(
        &mut self,
        msg: ExpiredCoresPackage<MeteredClientResponsePayload_0v1>,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let metered = msg.payload;
        self.handle_client_response_payload(ExpiredCoresPackage::new_with_routing_receipt_offers(
            msg.immediate_neighbor,
            msg.paying_wallet,
            msg.remaining_route,
            metered.response,
            msg.payload_len,
            msg.routing_receipt_offers,
        ));
        self.handle_service_receipt_offer(metered.receipt_offer);
    }
}

impl Handler<ExpiredCoresPackage<ServiceReceiptOfferPayload_0v1>> for ProxyServer {
    type Result = ();

    fn handle(
        &mut self,
        msg: ExpiredCoresPackage<ServiceReceiptOfferPayload_0v1>,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        self.handle_service_receipt_offer(msg.payload)
    }
}

impl Handler<StreamShutdownMsg> for ProxyServer {
    type Result = ();

    fn handle(&mut self, _msg: StreamShutdownMsg, _ctx: &mut Self::Context) -> Self::Result {
        self.handle_stream_shutdown_msg(_msg)
    }
}

impl Handler<NodeFromUiMessage> for ProxyServer {
    type Result = ();

    fn handle(&mut self, msg: NodeFromUiMessage, _ctx: &mut Self::Context) -> Self::Result {
        let client_id = msg.client_id;
        if let Ok((request, context_id)) = UiReceiptSessionProposalRequest::fmb(msg.body.clone()) {
            self.handle_receipt_session_proposal(request, client_id, context_id);
        } else if let Ok((request, context_id)) =
            UiReceiptSessionActivateRequest::fmb(msg.body.clone())
        {
            self.handle_receipt_session_activation(request, client_id, context_id);
        } else if let Ok((_, context_id)) = UiReceiptSessionStatusRequest::fmb(msg.body.clone()) {
            self.handle_receipt_session_status(client_id, context_id);
        } else if let Ok((_, context_id)) = UiReceiptSessionStopRequest::fmb(msg.body.clone()) {
            self.handle_receipt_session_stop(client_id, context_id);
        } else {
            handle_ui_crash_request(msg, &self.logger, self.crashable, CRASH_KEY)
        }
    }
}

impl Handler<StreamKeyPurge> for ProxyServer {
    type Result = ();

    fn handle(&mut self, msg: StreamKeyPurge, _ctx: &mut Self::Context) -> Self::Result {
        self.purge_stream_key(&msg.stream_key, "scheduled message");
    }
}

impl<M: actix::Message + 'static> Handler<MessageScheduler<M>> for ProxyServer
where
    ProxyServer: Handler<M>,
{
    type Result = ();

    fn handle(&mut self, msg: MessageScheduler<M>, ctx: &mut Self::Context) -> Self::Result {
        ctx.notify_later(msg.scheduled_msg, msg.delay);
    }
}

impl ProxyServer {
    pub fn new(
        cryptde_pair: CryptDEPair,
        is_decentralized: bool,
        consuming_wallet_balance: Option<i64>,
        crashable: bool,
        is_running_in_integration_test: bool,
    ) -> ProxyServer {
        let ps_logger = Logger::new("ProxyServer");
        ProxyServer {
            subs: None,
            client_request_payload_factory: Box::new(ClientRequestPayloadFactoryReal::new()),
            stream_key_factory: Box::new(StreamKeyFactoryReal {}),
            keys_and_addrs: BidiHashMap::new(),
            stream_info: HashMap::new(),
            pending_route_requests: HashMap::new(),
            is_decentralized,
            consuming_wallet_balance,
            cryptde_pair,
            crashable,
            logger: ps_logger,
            inbound_client_data_helper_opt: Some(Box::new(IBCDHelperReal::new())),
            stream_key_purge_delay: STREAM_KEY_PURGE_DELAY,
            next_return_route_id: Cell::new(1),
            next_route_request_id: Cell::new(1),
            is_running_in_integration_test,
            receipt_session_manager_opt: None,
            pending_receipt_offers: HashMap::new(),
            receipt_acknowledgement_outbox_opt: None,
            receipt_acknowledgement_retry_scheduled: false,
            route_activity_heartbeat: RouteActivityHeartbeat::default(),
        }
    }

    pub fn enable_receipt_sessions(&mut self, config: ReceiptSessionConfig) {
        self.receipt_session_manager_opt = Some(ReceiptSessionManager::new(config));
    }

    pub(crate) fn enable_recoverable_receipt_sessions(
        &mut self,
        config: ReceiptSessionConfig,
        recovery_store_opt: Option<Box<dyn ReceiptSessionRecoveryStore>>,
    ) {
        let now_unix_s = match Self::unix_time_now() {
            Ok(now) => now,
            Err(error) => {
                error!(
                    self.logger,
                    "Cannot initialize receipt-session recovery: {}", error
                );
                return;
            }
        };
        match ReceiptSessionManager::new_recovery_required(
            config.clone(),
            recovery_store_opt,
            now_unix_s,
        ) {
            Ok(manager) => self.receipt_session_manager_opt = Some(manager),
            Err(error) => {
                error!(
                    self.logger,
                    "Cannot restore receipt session; new activation will remain disabled: {}",
                    error
                );
                self.receipt_session_manager_opt =
                    ReceiptSessionManager::new_recovery_required(config, None, now_unix_s).ok();
            }
        }
    }

    pub fn enable_receipt_acknowledgement_outbox(
        &mut self,
        outbox: Box<dyn ReceiptAcknowledgementOutboxDao>,
    ) {
        self.receipt_acknowledgement_outbox_opt = Some(Arc::new(Mutex::new(outbox)));
    }

    fn handle_service_receipt_offer(&mut self, offer: ServiceReceiptOfferPayload_0v1) {
        let now_unix_s = match Self::unix_time_now() {
            Ok(now_unix_s) => now_unix_s,
            Err(error) => {
                error!(self.logger, "Cannot acknowledge service receipt: {}", error);
                return;
            }
        };
        let route_epoch = offer.signed_receipt.receipt.route_epoch;
        let acknowledged_payload = match self.receipt_session_manager_opt.as_mut() {
            Some(manager) => {
                match manager.acknowledge_offer(offer.signed_receipt.clone(), now_unix_s) {
                    Ok(payload) => payload,
                    Err(ReceiptSessionManagerError::ReceiptQuoteUnavailable) => {
                        if self.pending_receipt_offers.contains_key(&route_epoch)
                            || self.pending_receipt_offers.len() < MAX_PENDING_RECEIPT_OFFERS
                        {
                            self.pending_receipt_offers
                                .entry(route_epoch)
                                .or_insert(offer);
                            debug!(
                            self.logger,
                            "Deferring service receipt offer until its selected exit quote arrives"
                        );
                        } else {
                            warning!(
                            self.logger,
                            "Refusing service receipt offer because the deferred-offer limit was reached"
                        );
                        }
                        return;
                    }
                    Err(error) => {
                        warning!(self.logger, "Refusing service receipt offer: {}", error);
                        return;
                    }
                }
            }
            None => {
                warning!(
                    self.logger,
                    "Refusing service receipt offer because no receipt session is configured"
                );
                return;
            }
        };
        self.pending_receipt_offers.remove(&route_epoch);
        if let Err(error) = self.persist_receipt_acknowledgement(&acknowledged_payload) {
            error!(
                self.logger,
                "Cannot durably queue acknowledged service receipt: {}", error
            );
            return;
        }
        self.deliver_receipt_acknowledgement(acknowledged_payload);
    }

    fn handle_routing_receipt_offers(
        &mut self,
        stream_key: &StreamKey,
        response_payload_size: usize,
        encrypted_offers: Vec<crate::sub_lib::cryptde::CryptData>,
    ) {
        if encrypted_offers.is_empty() && self.receipt_session_manager_opt.is_none() {
            return;
        }
        let now_unix_s = match Self::unix_time_now() {
            Ok(now_unix_s) => now_unix_s,
            Err(error) => {
                warning!(self.logger, "Cannot process routing receipts: {}", error);
                return;
            }
        };
        let response_payload_size = match u64::try_from(response_payload_size) {
            Ok(size) => size,
            Err(_) => {
                warning!(
                    self.logger,
                    "Cannot process routing receipts: response payload size is unsupported"
                );
                return;
            }
        };
        let offers = match self.receipt_session_manager_opt.as_mut() {
            Some(manager) => {
                if let Err(error) =
                    manager.record_routing_response(stream_key, response_payload_size, now_unix_s)
                {
                    if error != ReceiptSessionManagerError::ReceiptRouteNotFound {
                        warning!(
                            self.logger,
                            "Cannot correlate routing response with receipt session: {}",
                            error
                        );
                    }
                }
                encrypted_offers
                    .iter()
                    .filter_map(|encrypted_offer| {
                        match manager.decrypt_routing_offer(encrypted_offer, now_unix_s) {
                            Ok(offer) => Some(offer),
                            Err(error) => {
                                warning!(
                                    self.logger,
                                    "Refusing encrypted routing receipt: {}",
                                    error
                                );
                                None
                            }
                        }
                    })
                    .collect::<Vec<_>>()
            }
            None => {
                if !encrypted_offers.is_empty() {
                    warning!(
                        self.logger,
                        "Refusing encrypted routing receipts because no receipt session is configured"
                    );
                }
                return;
            }
        };
        for offer in offers {
            self.handle_service_receipt_offer(offer);
        }
    }

    fn persist_receipt_acknowledgement(
        &self,
        acknowledged_payload: &crate::sub_lib::service_receipt::ServiceReceiptPayload_0v1,
    ) -> Result<(), String> {
        match self.receipt_acknowledgement_outbox_opt.as_ref() {
            Some(outbox) => outbox
                .lock()
                .map_err(|_| "receipt acknowledgement outbox lock is poisoned".to_string())?
                .enqueue(acknowledged_payload, SystemTime::now())
                .map_err(|error| format!("{:?}", error)),
            None => Ok(()),
        }
    }

    fn replay_persisted_receipt_acknowledgements(&self) {
        let outbox = match self.receipt_acknowledgement_outbox_opt.as_ref() {
            Some(outbox) => Arc::clone(outbox),
            None => return,
        };
        let pending = match outbox.lock() {
            Ok(guard) => match guard.pending() {
                Ok(pending) => pending,
                Err(error) => {
                    error!(
                        self.logger,
                        "Could not load durable receipt acknowledgements: {:?}", error
                    );
                    return;
                }
            },
            Err(_) => {
                error!(
                    self.logger,
                    "Could not load durable receipt acknowledgements: outbox lock is poisoned"
                );
                return;
            }
        };
        if pending.len() > MAX_RECEIPT_ACKNOWLEDGEMENT_RECOVERY_BATCH {
            warning!(
                self.logger,
                "Receipt acknowledgement recovery is limited to {} of {} pending records this startup",
                MAX_RECEIPT_ACKNOWLEDGEMENT_RECOVERY_BATCH,
                pending.len()
            );
            if self
                .out_subs("ProxyServer")
                .retry_receipt_acknowledgements
                .try_send(RetryReceiptAcknowledgements {
                    schedule_after_delay: true,
                })
                .is_err()
            {
                error!(
                    self.logger,
                    "Could not schedule the next receipt acknowledgement recovery batch"
                );
            }
        }
        pending
            .into_iter()
            .take(MAX_RECEIPT_ACKNOWLEDGEMENT_RECOVERY_BATCH)
            .for_each(|payload| self.deliver_receipt_acknowledgement(payload));
    }

    fn deliver_receipt_acknowledgement(
        &self,
        acknowledged_payload: crate::sub_lib::service_receipt::ServiceReceiptPayload_0v1,
    ) {
        let provider_public_key = acknowledged_payload
            .acknowledged_receipt
            .signed_receipt
            .receipt
            .provider_public_key
            .clone();
        let receipt_message: MessageType = acknowledged_payload.clone().into();
        let payload_size = match serde_cbor::to_vec(&receipt_message) {
            Ok(serialized) => serialized.len(),
            Err(error) => {
                error!(
                    self.logger,
                    "Cannot serialize acknowledged service receipt: {}", error
                );
                return;
            }
        };
        let route_query = RouteQueryMessage::service_receipt_delivery_route_request(
            provider_public_key.clone(),
            payload_size,
        );
        let route_source = self.out_subs("Neighborhood").route_source.clone();
        let hopper_sub = self.out_subs("Hopper").hopper.clone();
        let routing_accountant = self.out_subs("Accountant").routing_accountant.clone();
        let retry_receipt_acknowledgements = self
            .out_subs("ProxyServer")
            .retry_receipt_acknowledgements
            .clone();
        let main_cryptde = self.cryptde_pair.main.dup();
        let logger = self.logger.clone();
        let outbox_opt = self.receipt_acknowledgement_outbox_opt.clone();
        tokio::spawn(route_source.send(route_query).then(move |route_result| {
            let schedule_retry = || {
                if retry_receipt_acknowledgements
                    .try_send(RetryReceiptAcknowledgements {
                        schedule_after_delay: true,
                    })
                    .is_err()
                {
                    error!(
                        logger,
                        "Could not schedule receipt acknowledgement retry"
                    );
                }
            };
            match route_result {
                Ok(Some(route_response)) => {
                    let expected_services = match route_response.expected_services {
                        ExpectedServices::OneWay(services)
                        | ExpectedServices::RoundTrip(services, _) => services,
                    };
                    match IncipientCoresPackage::new(
                        main_cryptde.as_ref(),
                        route_response.route,
                        receipt_message,
                        &provider_public_key,
                    ) {
                        Ok(package) => {
                            let routing =
                                ProxyServer::report_on_routing_services(expected_services, &logger);
                            if !routing.is_empty() {
                                let report = ReportRoutingServicesConsumedMessage {
                                    timestamp: SystemTime::now(),
                                    payload_size: package.payload.len(),
                                    routing,
                                };
                                if routing_accountant.try_send(report).is_err() {
                                    error!(
                                        logger,
                                        "Could not account for receipt acknowledgement route"
                                    );
                                    schedule_retry();
                                    return Ok(());
                                }
                            }
                            if hopper_sub.try_send(package).is_err() {
                                error!(logger, "Could not queue acknowledged service receipt");
                                schedule_retry();
                            } else if let Some(outbox) = outbox_opt.as_ref() {
                                match outbox.lock() {
                                    Ok(mut guard) => {
                                        if let Err(error) = guard.delete(&acknowledged_payload) {
                                            error!(
                                                logger,
                                                "Could not clear delivered receipt acknowledgement: {:?}",
                                            error
                                        );
                                            schedule_retry();
                                        }
                                    }
                                    Err(_) => {
                                        error!(
                                            logger,
                                            "Could not clear delivered receipt acknowledgement: outbox lock is poisoned"
                                        );
                                        schedule_retry();
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            error!(
                                logger,
                                "Could not encrypt acknowledged service receipt: {}", error
                            );
                            schedule_retry();
                        }
                    }
                }
                Ok(None) => {
                    warning!(
                        logger,
                        "Could not find a receipt-capable route to return an acknowledgement"
                    );
                    schedule_retry();
                }
                Err(error) => {
                    error!(
                        logger,
                        "Neighborhood failed while routing a receipt acknowledgement: {:?}", error
                    );
                    schedule_retry();
                }
            }
            Ok(())
        }));
    }

    fn replay_pending_receipt_offer(&mut self, route_epoch: [u8; 32]) {
        if let Some(offer) = self.pending_receipt_offers.remove(&route_epoch) {
            self.handle_service_receipt_offer(offer);
        }
    }

    fn unix_time_now() -> Result<u64, String> {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .map_err(|_| "system clock is before the Unix epoch".to_string())
    }

    fn receipt_session_status_response(
        status: ReceiptSessionStatus,
    ) -> UiReceiptSessionStatusResponse {
        UiReceiptSessionStatusResponse {
            active: status.is_active(),
            protocol_version_opt: status.protocol_version_opt,
            chain_name_opt: status.chain_name_opt,
            chain_id_opt: status.chain_id_opt,
            masq_token_contract_opt: status
                .masq_token_contract_opt
                .map(|address| format!("{:#x}", address)),
            settlement_contract_opt: status
                .settlement_contract_opt
                .map(|address| format!("{:#x}", address)),
            payer_wallet_address_opt: status
                .payer_wallet_address_opt
                .map(|address| format!("{:#x}", address)),
            payer_session_public_key_opt: status
                .payer_session_public_key_opt
                .map(|key| format!("0x{}", key.as_slice().to_hex::<String>())),
            authorization_id_opt: status
                .authorization_id_opt
                .map(|id| format!("0x{}", id.to_hex::<String>())),
            max_total_charge_wei_opt: status
                .max_total_charge_wei_opt
                .map(|amount| amount.to_string()),
            spent_charge_wei_opt: status.spent_charge_wei_opt.map(|amount| amount.to_string()),
            remaining_charge_wei_opt: status
                .max_total_charge_wei_opt
                .zip(status.spent_charge_wei_opt)
                .and_then(|(maximum, spent)| maximum.checked_sub(spent))
                .map(|amount| amount.to_string()),
            valid_from_unix_s_opt: status.valid_from_unix_s_opt,
            expires_at_unix_s_opt: status.expires_at_unix_s_opt,
        }
    }

    fn send_ui_response<T: ToMessageBody>(&self, response: T, client_id: u64, context_id: u64) {
        self.out_subs("UiGateway")
            .ui_gateway
            .try_send(NodeToUiMessage {
                target: MessageTarget::ClientId(client_id),
                body: response.tmb(context_id),
            })
            .expect("UiGateway is dead");
    }

    fn send_receipt_session_error(
        &self,
        opcode: &str,
        client_id: u64,
        context_id: u64,
        message: String,
    ) {
        self.out_subs("UiGateway")
            .ui_gateway
            .try_send(NodeToUiMessage {
                target: MessageTarget::ClientId(client_id),
                body: MessageBody {
                    opcode: opcode.to_string(),
                    path: MessagePath::Conversation(context_id),
                    payload: Err((RECEIPT_SESSION_ERROR, message)),
                },
            })
            .expect("UiGateway is dead");
    }

    fn handle_receipt_session_proposal(
        &mut self,
        request: UiReceiptSessionProposalRequest,
        client_id: u64,
        context_id: u64,
    ) {
        let result = Self::unix_time_now().and_then(|now_unix_s| {
            self.receipt_session_manager_opt
                .as_mut()
                .ok_or_else(|| "receipt sessions are unavailable in this Node mode".to_string())?
                .propose(
                    &request.max_total_charge_wei,
                    request.duration_seconds,
                    now_unix_s,
                )
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(proposal) => self.send_ui_response(
                UiReceiptSessionProposalResponse {
                    proposal_id: proposal.proposal_id,
                    protocol_version: proposal.policy.protocol_version,
                    chain_name: proposal.chain_name,
                    chain_id: proposal.policy.chain_id,
                    masq_token_contract: format!("{:#x}", proposal.masq_token_contract),
                    settlement_contract: format!("{:#x}", proposal.policy.settlement_contract),
                    payer_wallet_address: format!("{:#x}", proposal.policy.payer_wallet_address),
                    payer_session_public_key: format!(
                        "0x{}",
                        proposal
                            .policy
                            .payer_session_public_key
                            .as_slice()
                            .to_hex::<String>()
                    ),
                    max_total_charge_wei: proposal.policy.max_total_charge_wei.to_string(),
                    valid_from_unix_s: proposal.policy.valid_from_unix_s,
                    expires_at_unix_s: proposal.policy.expires_at_unix_s,
                    authorization_id: format!("0x{}", proposal.authorization_id.to_hex::<String>()),
                    eip712_typed_data: proposal.eip712_typed_data,
                },
                client_id,
                context_id,
            ),
            Err(error) => self.send_receipt_session_error(
                UiReceiptSessionProposalRequest::type_opcode(),
                client_id,
                context_id,
                error,
            ),
        }
    }

    fn handle_receipt_session_activation(
        &mut self,
        request: UiReceiptSessionActivateRequest,
        client_id: u64,
        context_id: u64,
    ) {
        let result = Self::unix_time_now().and_then(|now_unix_s| {
            self.receipt_session_manager_opt
                .as_mut()
                .ok_or_else(|| "receipt sessions are unavailable in this Node mode".to_string())?
                .activate(&request.proposal_id, &request.wallet_signature, now_unix_s)
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(status) => {
                self.pending_receipt_offers.clear();
                self.send_ui_response(
                    UiReceiptSessionActivateResponse {
                        status: Self::receipt_session_status_response(status),
                    },
                    client_id,
                    context_id,
                )
            }
            Err(error) => self.send_receipt_session_error(
                UiReceiptSessionActivateRequest::type_opcode(),
                client_id,
                context_id,
                error,
            ),
        }
    }

    fn handle_receipt_session_status(&mut self, client_id: u64, context_id: u64) {
        let result = Self::unix_time_now().and_then(|now_unix_s| {
            self.receipt_session_manager_opt
                .as_mut()
                .map(|manager| manager.status(now_unix_s))
                .ok_or_else(|| "receipt sessions are unavailable in this Node mode".to_string())
        });
        match result {
            Ok(status) => self.send_ui_response(
                Self::receipt_session_status_response(status),
                client_id,
                context_id,
            ),
            Err(error) => self.send_receipt_session_error(
                UiReceiptSessionStatusRequest::type_opcode(),
                client_id,
                context_id,
                error,
            ),
        }
    }

    fn handle_receipt_session_stop(&mut self, client_id: u64, context_id: u64) {
        let result = self
            .receipt_session_manager_opt
            .as_mut()
            .map(|manager| manager.stop())
            .ok_or_else(|| "receipt sessions are unavailable in this Node mode".to_string());
        match result {
            Ok(status) => {
                self.pending_receipt_offers.clear();
                self.send_ui_response(
                    UiReceiptSessionStopResponse {
                        status: Self::receipt_session_status_response(status),
                    },
                    client_id,
                    context_id,
                )
            }
            Err(error) => self.send_receipt_session_error(
                UiReceiptSessionStopRequest::type_opcode(),
                client_id,
                context_id,
                error,
            ),
        }
    }

    pub fn make_subs_from(addr: &Addr<ProxyServer>) -> ProxyServerSubs {
        ProxyServerSubs {
            bind: recipient!(addr, BindMessage),
            from_dispatcher: recipient!(addr, InboundClientData),
            from_hopper: recipient!(addr, ExpiredCoresPackage<ClientResponsePayload_0v1>),
            dns_failure_from_hopper: recipient!(addr, ExpiredCoresPackage<DnsResolveFailure_0v1>),
            receipt_offer_from_hopper: recipient!(
                addr,
                ExpiredCoresPackage<ServiceReceiptOfferPayload_0v1>
            ),
            metered_response_from_hopper: recipient!(
                addr,
                ExpiredCoresPackage<MeteredClientResponsePayload_0v1>
            ),
            stream_shutdown_sub: recipient!(addr, StreamShutdownMsg),
            node_from_ui: recipient!(addr, NodeFromUiMessage),
            route_result_sub: recipient!(addr, AddRouteResultMessage),
            schedule_stream_key_purge: recipient!(addr, MessageScheduler<StreamKeyPurge>),
            retry_receipt_acknowledgements: recipient!(addr, RetryReceiptAcknowledgements),
            record_exit_request_for_receipt: recipient!(addr, RecordExitRequestForReceipt),
        }
    }

    fn stream_info(&self, stream_key: &StreamKey) -> Option<&StreamInfo> {
        match self.stream_info.get(stream_key) {
            None => {
                error!(
                    self.logger,
                    "Stream key {} not found in stream_info", stream_key
                );
                None
            }
            Some(info) => Some(info),
        }
    }

    fn stream_info_mut(&mut self, stream_key: &StreamKey) -> Option<&mut StreamInfo> {
        match self.stream_info.get_mut(stream_key) {
            None => {
                error!(
                    self.logger,
                    "Stream key {} not found in stream_info", stream_key
                );
                None
            }
            Some(info) => Some(info),
        }
    }

    fn remove_dns_failure_retry(
        stream_info: &mut StreamInfo,
        stream_key: &StreamKey,
    ) -> Result<DNSFailureRetry, String> {
        match stream_info.dns_failure_retry_opt.take() {
            None => Err(format!(
                "No DNSFailureRetry entry found for the stream_key: {:?}",
                stream_key
            )),
            Some(retry) => Ok(retry),
        }
    }

    fn retry_dns_resolution(
        &mut self,
        retry: &mut DNSFailureRetry,
        client_addr: SocketAddr,
    ) -> Result<(), String> {
        let next_attempt_id = retry
            .active_attempt_id
            .checked_add(1)
            .ok_or_else(|| "DNS attempt identifier overflow".to_string())?;
        let mut retry_payload = retry.unsuccessful_request.clone();
        retry_payload.dns_attempt_id_opt = Some(next_attempt_id);
        if let Some(manager) = self.receipt_session_manager_opt.as_mut() {
            let old_route_epoch_opt = retry_payload
                .receipt_session_request_opt
                .as_ref()
                .map(|request| request.route_epoch);
            retry_payload.receipt_session_request_opt = manager
                .rotate_route_for_stream(retry_payload.stream_key, Self::unix_time_now()?)
                .map_err(|error| error.to_string())?;
            if let Some(old_route_epoch) = old_route_epoch_opt {
                self.pending_receipt_offers.remove(&old_route_epoch);
            }
        }
        let args =
            TransmitToHopperArgs::new(self, retry_payload, client_addr, SystemTime::now(), false);
        let route_source = self.out_subs("Neighborhood").route_source.clone();
        let proxy_server_sub = self.out_subs("ProxyServer").route_result_sub.clone();
        let resolver_args = self
            .queue_pending_route_packet(args)?
            .ok_or_else(|| "DNS retry encountered an existing pending route request".to_string())?;
        let inbound_client_data_helper = self
            .inbound_client_data_helper_opt
            .as_ref()
            .expect("IBCDHelper uninitialized");
        inbound_client_data_helper.request_route_and_transmit(
            resolver_args,
            route_source,
            proxy_server_sub,
        );
        retry.active_attempt_id = next_attempt_id;
        Ok(())
    }

    fn retire_stream_key(&mut self, stream_key: &StreamKey) {
        self.purge_stream_key(stream_key, "DNS resolution failure");
    }

    fn send_dns_failure_response_to_the_browser(
        &self,
        client_addr: SocketAddr,
        proxy_protocol: ProxyProtocol,
        hostname: String,
    ) {
        self.subs
            .as_ref()
            .expect("Dispatcher unbound in ProxyServer")
            .dispatcher
            .try_send(TransmitDataMsg {
                endpoint: Endpoint::Socket(client_addr),
                last_data: true,
                sequence_number_opt: Some(0), // DNS resolution errors always happen on the first request
                data: from_protocol(proxy_protocol)
                    .server_impersonator()
                    .dns_resolution_failure_response(hostname),
            })
            .expect("Dispatcher is dead");
    }

    fn get_response_services(
        route_query_response: &RouteQueryResponse,
    ) -> Option<&[ExpectedService]> {
        match &route_query_response.expected_services {
            ExpectedServices::RoundTrip(_, back) => Some(back),
            _ => None,
        }
    }

    fn receipt_exit_quote(
        route_query_response: &RouteQueryResponse,
    ) -> Option<(PublicKey, u64, u64)> {
        Self::get_response_services(route_query_response).and_then(|response_services| {
            response_services.iter().find_map(|service| match service {
                ExpectedService::Exit(provider_public_key, _, rate_pack) => Some((
                    provider_public_key.clone(),
                    rate_pack.exit_service_rate,
                    rate_pack.exit_byte_rate,
                )),
                _ => None,
            })
        })
    }

    fn receipt_routing_quotes(
        route_query_response: &RouteQueryResponse,
    ) -> (Vec<RoutingReceiptQuote>, Vec<RoutingReceiptQuote>) {
        let convert = |services: &[ExpectedService]| {
            services
                .iter()
                .filter_map(|service| match service {
                    ExpectedService::Routing(provider_public_key, _, rate_pack) => {
                        Some(RoutingReceiptQuote {
                            provider_public_key: provider_public_key.clone(),
                            service_rate: rate_pack.routing_service_rate,
                            byte_rate: rate_pack.routing_byte_rate,
                        })
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        match &route_query_response.expected_services {
            ExpectedServices::RoundTrip(request, response) => (convert(request), convert(response)),
            ExpectedServices::OneWay(request) => (convert(request), vec![]),
        }
    }

    fn find_exit_node_key(response_services: &[ExpectedService]) -> Option<PublicKey> {
        response_services
            .iter()
            .find_map(|service| service.exit_node_key_opt())
    }

    fn handle_add_route_result_message(&mut self, msg: AddRouteResultMessage) {
        if !self.stream_info.contains_key(&msg.stream_key) {
            warning!(
                self.logger,
                "Discarding stale AddRouteResultMessage for stream key {} because the stream no longer exists",
                msg.stream_key
            );
            return;
        }
        if self
            .stream_info
            .get(&msg.stream_key)
            .and_then(|stream_info| stream_info.dns_failure_retry_opt.as_ref())
            .is_none()
        {
            warning!(
                self.logger,
                "Discarding AddRouteResultMessage for stream key {} because the stream has no pending DNS request",
                msg.stream_key
            );
            return;
        }
        let pending_route_request = match self.pending_route_requests.get(&msg.stream_key) {
            Some(pending) if pending.route_request_id == msg.route_request_id => self
                .pending_route_requests
                .remove(&msg.stream_key)
                .expect("validated pending route request disappeared"),
            Some(pending) => {
                warning!(
                    self.logger,
                    "Discarding stale AddRouteResultMessage for stream key {}: expected route request {}, received {}",
                    msg.stream_key,
                    pending.route_request_id,
                    msg.route_request_id
                );
                return;
            }
            None => {
                warning!(
                    self.logger,
                    "Discarding AddRouteResultMessage for stream key {} because no route request is pending",
                    msg.stream_key
                );
                return;
            }
        };
        let mut route_failed = msg.result.is_err();
        let selected_route_opt = msg.result.as_ref().ok().cloned();
        let receipt_quote_opt = msg.result.as_ref().ok().and_then(Self::receipt_exit_quote);
        let routing_quotes_opt = msg.result.as_ref().ok().map(Self::receipt_routing_quotes);
        // We can't access self.logger for logging once we obtain mutable access to a stream_info
        // element. So we create a delayed_log closure that we can call with self.logger after
        // we've finished with the mutable borrow.  We have to use #[allow(unused_assignments)]
        // because Rust can't figure out that delayed_log will always be assigned before it's used.
        type DelayedLogArgs = Box<dyn FnOnce(&Logger, String, StreamKey, usize, String)>;
        #[allow(unused_assignments)]
        let mut delayed_log: DelayedLogArgs = Box::new(|_, _, _, _, _| {});
        let logger = self.logger.clone();
        let (target_hostname, stream_key, retries_left, message) = {
            let stream_info = match self.stream_info_mut(&msg.stream_key) {
                Some(stream_info) => stream_info,
                None => {
                    warning!(
                        logger,
                        "Discarding stale AddRouteResultMessage for stream key {} because the stream no longer exists",
                        msg.stream_key
                    );
                    return;
                }
            };
            let dns_failure_retry = match stream_info.dns_failure_retry_opt.as_ref() {
                Some(dns_failure_retry) => dns_failure_retry,
                None => {
                    warning!(
                        logger,
                        "Discarding AddRouteResultMessage for stream key {} because the stream has no pending DNS request",
                        msg.stream_key
                    );
                    return;
                }
            };
            let mut message = String::new();
            match msg.result {
                Ok(route_query_response) => {
                    delayed_log = Box::new(
                        move |logger: &Logger,
                              _target_hostname: String,
                              _stream_key: StreamKey,
                              retries_left: usize,
                              _: String| {
                            debug!(
                                logger,
                                "Found a new route for DNS retry; destination and stream redacted; retries left: {}",
                                retries_left
                            );
                        },
                    );
                    stream_info.route_opt = Some(route_query_response);
                }
                Err(e) => {
                    message = e;
                    delayed_log = Box::new(
                        move |logger: &Logger,
                              _target_hostname: String,
                              _stream_key: StreamKey,
                              retries_left: usize,
                              _message: String| {
                            warning!(
                                logger,
                                "No route found for DNS retry; destination, stream and error redacted; retries left: {}",
                                retries_left
                            );
                        },
                    );
                }
            }
            (
                dns_failure_retry
                    .unsuccessful_request
                    .target_hostname
                    .clone(),
                msg.stream_key,
                dns_failure_retry.retries_left,
                message,
            )
        };
        delayed_log(
            &self.logger,
            target_hostname,
            stream_key,
            retries_left,
            message,
        );
        if let (Some((request_quotes, response_quotes)), Some(manager)) = (
            routing_quotes_opt,
            self.receipt_session_manager_opt.as_mut(),
        ) {
            match Self::unix_time_now() {
                Ok(now_unix_s) => {
                    if let Err(error) = manager.bind_routing_quotes(
                        &stream_key,
                        request_quotes,
                        response_quotes,
                        now_unix_s,
                    ) {
                        if error != ReceiptSessionManagerError::ReceiptRouteNotFound {
                            warning!(
                                self.logger,
                                "Unable to bind receipt session to selected routing quotes: {}",
                                error
                            );
                        }
                    }
                }
                Err(error) => warning!(
                    self.logger,
                    "Unable to bind receipt session to selected routing quotes: {}",
                    error
                ),
            }
        }
        let bound_route_epoch_opt =
            match (receipt_quote_opt, self.receipt_session_manager_opt.as_mut()) {
                (Some((provider_public_key, exit_service_rate, exit_byte_rate)), Some(manager)) => {
                    match Self::unix_time_now() {
                        Ok(now_unix_s) => match manager.bind_exit_quote(
                            &stream_key,
                            provider_public_key,
                            exit_service_rate,
                            exit_byte_rate,
                            now_unix_s,
                        ) {
                            Ok(()) => manager.route_epoch_for_stream(&stream_key),
                            Err(ReceiptSessionManagerError::ReceiptRouteNotFound) => None,
                            Err(error) => {
                                warning!(
                                    self.logger,
                                    "Unable to bind receipt session to selected exit quote: {}",
                                    error
                                );
                                None
                            }
                        },
                        Err(error) => {
                            warning!(
                                self.logger,
                                "Unable to bind receipt session to selected exit quote: {}",
                                error
                            );
                            None
                        }
                    }
                }
                _ => None,
            };
        if let Some(route_epoch) = bound_route_epoch_opt {
            self.replay_pending_receipt_offer(route_epoch);
        }
        match selected_route_opt {
            Some(route_query_response) => {
                for args in pending_route_request.packets {
                    if let Err(error) =
                        Self::try_transmit_to_hopper(args, route_query_response.clone())
                    {
                        warning!(
                            self.logger,
                            "Unable to transmit a packet after route selection for stream {}: {}",
                            stream_key,
                            error
                        );
                        route_failed = true;
                        if let Some(stream_info) = self.stream_info.get_mut(&stream_key) {
                            stream_info.route_opt = None;
                        }
                        break;
                    }
                }
            }
            None => {
                if let Some(args) = pending_route_request.packets.into_iter().next() {
                    if let Err(error) = Self::send_route_failure(
                        args.payload.protocol,
                        &args.payload.target_hostname,
                        args.client_addr,
                        &args.dispatcher_sub,
                    ) {
                        warning!(
                            self.logger,
                            "Unable to notify browser of route-selection failure for stream {}: {}",
                            stream_key,
                            error
                        );
                    }
                }
            }
        }
        if route_failed
            && self
                .out_subs("Neighborhood")
                .route_use_failed
                .try_send(RouteUseFailedMessage)
                .is_err()
        {
            warning!(
                self.logger,
                "Could not report route-selection failure to Neighborhood"
            );
        }
    }

    fn handle_dns_resolve_failure(&mut self, msg: &ExpiredCoresPackage<DnsResolveFailure_0v1>) {
        self.handle_routing_receipt_offers(
            &msg.payload.stream_key,
            msg.payload_len,
            msg.routing_receipt_offers.clone(),
        );
        let response = &msg.payload;

        // Validate the complete correlation state before removing it or emitting legacy
        // ReportServicesConsumed charges. A stale, raced or malicious failure package must not
        // delete an active stream, create payload-size service charges or penalize an exit that
        // was never associated with a DNS retry. Independently signed receipt offers retain
        // their own session, epoch and signature validation above.
        let (
            route_query_response,
            response_services,
            exit_public_key,
            protocol,
            uncorrelated_legacy_retry,
        ) = {
            let stream_info = match self.stream_info.get(&response.stream_key) {
                Some(info) => info,
                None => {
                    error!(
                        self.logger,
                        "Discarding DnsResolveFailure message from an unrecognized stream key {:?}",
                        &response.stream_key
                    );
                    return;
                }
            };
            let route_query_response = match stream_info.route_opt.as_ref() {
                Some(route_query_response) => route_query_response.clone(),
                None => {
                    error!(
                        self.logger,
                        "Discarding DnsResolveFailure message for stream key {} because it has no route info",
                        &response.stream_key
                    );
                    return;
                }
            };
            let response_services = match Self::get_response_services(&route_query_response) {
                Some(response_services) => response_services.to_vec(),
                None => {
                    error!(
                        self.logger,
                        "Discarding DnsResolveFailure message for stream key {} because its route has no response services",
                        &response.stream_key
                    );
                    return;
                }
            };
            if !Self::response_services_are_valid(&response_services) {
                error!(
                    self.logger,
                    "Discarding DnsResolveFailure message for stream key {} because its response-service shape is invalid",
                    &response.stream_key
                );
                return;
            }
            let exit_public_key = if !self.is_decentralized {
                self.cryptde_pair.main.public_key().clone()
            } else {
                match Self::find_exit_node_key(&response_services) {
                    Some(exit_public_key) => exit_public_key,
                    None => {
                        error!(
                            self.logger,
                            "Discarding DnsResolveFailure message for stream key {} because its response services have no exit node",
                            &response.stream_key
                        );
                        return;
                    }
                }
            };
            let retry = match stream_info.dns_failure_retry_opt.as_ref() {
                Some(retry) => retry,
                None => {
                    error!(
                        self.logger,
                        "Discarding DnsResolveFailure message for stream key {} because it has no DNS failure retry context",
                        &response.stream_key
                    );
                    return;
                }
            };
            let uncorrelated_legacy_retry = match response.dns_attempt_id_opt {
                Some(attempt_id) if attempt_id == retry.active_attempt_id => false,
                Some(attempt_id) => {
                    warning!(
                        self.logger,
                        "Discarding stale DnsResolveFailure message for stream key {}: expected attempt {}, received {}",
                        &response.stream_key,
                        retry.active_attempt_id,
                        attempt_id
                    );
                    return;
                }
                None if retry.active_attempt_id > 0 => true,
                None => false,
            };
            if self
                .pending_route_requests
                .contains_key(&response.stream_key)
            {
                warning!(
                    self.logger,
                    "Discarding premature DnsResolveFailure message for stream key {} while its route request is still pending",
                    &response.stream_key
                );
                return;
            }
            let protocol = match stream_info.protocol_opt {
                Some(protocol) => protocol,
                None => {
                    error!(
                        self.logger,
                        "Discarding DnsResolveFailure message for stream key {} because it has no proxy protocol",
                        &response.stream_key
                    );
                    return;
                }
            };
            (
                route_query_response,
                response_services,
                exit_public_key,
                protocol,
                uncorrelated_legacy_retry,
            )
        };
        let client_addr = match self.keys_and_addrs.a_to_b(&response.stream_key) {
            Some(client_addr) => client_addr,
            None => {
                error!(
                    self.logger,
                    "Discarding DnsResolveFailure message because destination and stream correlation are unrecognized"
                );
                return;
            }
        };
        if uncorrelated_legacy_retry {
            warning!(
                self.logger,
                "Ending DNS retries for stream key {} because a legacy failure cannot be correlated with active attempt {}",
                &response.stream_key,
                self.stream_info
                    .get(&response.stream_key)
                    .and_then(|info| info.dns_failure_retry_opt.as_ref())
                    .map(|retry| retry.active_attempt_id)
                    .unwrap_or_default()
            );
            self.retire_stream_key(&response.stream_key);
            self.send_dns_failure_response_to_the_browser(
                client_addr,
                protocol,
                route_query_response.host.name.clone(),
            );
            return;
        }
        let mut stream_info = match self.stream_info.remove(&response.stream_key) {
            Some(info) => info,
            None => {
                error!(
                    self.logger,
                    "Discarding DnsResolveFailure message because stream key {:?} disappeared during validation",
                    &response.stream_key
                );
                return;
            }
        };
        let mut restore_stream_info = true;

        if self
            .subs
            .as_ref()
            .expect("Neighborhood unbound in ProxyServer")
            .update_node_record_metadata
            .try_send(UpdateNodeRecordMetadataMessage {
                public_key: exit_public_key,
                metadata_change: NRMetadataChange::AddUnreachableHost {
                    hostname: route_query_response.host.name.clone(),
                },
            })
            .is_err()
        {
            warning!(
                self.logger,
                "Could not report unreachable-host metadata to Neighborhood"
            );
        }
        self.report_response_services_consumed(&response_services, 0, msg.payload_len);
        let retry_ref = match &mut stream_info.dns_failure_retry_opt {
            Some(retry_ref) => retry_ref,
            None => {
                error!(
                    self.logger,
                    "Discarding DnsResolveFailure message because the DNS failure retry context for stream key {} disappeared during validation",
                    &response.stream_key
                );
                self.stream_info.insert(response.stream_key, stream_info);
                return;
            }
        };
        debug!(
            self.logger,
            "Handling DNS failure; destination and stream redacted; retries left: {}",
            retry_ref.retries_left,
        );
        if retry_ref.retries_left > 0 {
            match self.retry_dns_resolution(retry_ref, client_addr) {
                Ok(()) => retry_ref.retries_left -= 1,
                Err(error) => {
                    warning!(
                        self.logger,
                        "Unable to rotate receipt session for DNS retry: {}",
                        error
                    );
                    retry_ref.retries_left = 0;
                    restore_stream_info = false;
                    self.retire_stream_key(&response.stream_key);
                    self.send_dns_failure_response_to_the_browser(
                        client_addr,
                        protocol,
                        route_query_response.host.name.clone(),
                    );
                }
            }
        } else {
            restore_stream_info = false;
            self.retire_stream_key(&response.stream_key);
            self.send_dns_failure_response_to_the_browser(
                client_addr,
                protocol,
                route_query_response.host.name.clone(),
            );
        }
        if restore_stream_info {
            self.stream_info.insert(response.stream_key, stream_info);
        }
    }

    fn schedule_stream_key_purge(&mut self, stream_key: StreamKey) {
        let stream_key_purge_delay = self.stream_key_purge_delay;
        // We can't access self.logger for logging once we obtain mutable access to a stream_info
        // element. So we create a delayed_log closure that we can call with self.logger after
        // we've finished with the mutable borrow.
        let mut delayed_log: Box<dyn FnOnce(&Logger)> = Box::new(|_: &Logger| {});
        if let Some(stream_info) = self.stream_info_mut(&stream_key) {
            let tunnel_state = if stream_info.tunneled_host_opt.is_some() {
                "tunneled"
            } else {
                "direct"
            };
            delayed_log = Box::new(move |logger: &Logger| {
                debug!(
                    logger,
                    "Client closed a {} stream; destination and stream identifiers redacted. It will be purged after {:?}.",
                    tunnel_state,
                    stream_key_purge_delay
                );
            });
            stream_info.time_to_live_opt = Some(SystemTime::now());
            self.subs
                .as_ref()
                .expect("ProxyServer Subs Unbound")
                .schedule_stream_key_purge
                .try_send(MessageScheduler {
                    scheduled_msg: StreamKeyPurge { stream_key },
                    delay: self.stream_key_purge_delay,
                })
                .expect("ProxyServer is dead");
        }
        delayed_log(&self.logger);
    }

    fn log_straggling_packet(
        &self,
        stream_key: &StreamKey,
        packet_len: usize,
        old_timestamp: &SystemTime,
    ) {
        let duration_since = SystemTime::now()
            .duration_since(*old_timestamp)
            .unwrap_or_else(|_| Duration::from_secs(0));
        debug!(
            self.logger,
            "Straggling packet of length {} received for a stream key {:?} after a delay of {:?}",
            packet_len,
            stream_key,
            duration_since
        );
    }

    fn handle_client_response_payload(
        &mut self,
        msg: ExpiredCoresPackage<ClientResponsePayload_0v1>,
    ) {
        self.handle_client_response_payload_at(msg, Instant::now())
    }

    fn handle_client_response_payload_at(
        &mut self,
        msg: ExpiredCoresPackage<ClientResponsePayload_0v1>,
        route_activity_at: Instant,
    ) {
        self.handle_routing_receipt_offers(
            &msg.payload.stream_key,
            msg.payload_len,
            msg.routing_receipt_offers.clone(),
        );
        let payload_data_len = msg.payload_len;
        let response = msg.payload;
        debug!(
            self.logger,
            "Relaying ClientResponsePayload (stream key {}, sequence {}, length {}) from Hopper to Dispatcher for client",
            response.stream_key, response.sequenced_packet.sequence_number, response.sequenced_packet.data.len()
        );
        let expected_services = match self.get_expected_return_services(&response.stream_key) {
            Some(expected_services) => expected_services,
            None => return,
        };
        let browser_proxy_sequence_offset = match self.stream_info(&response.stream_key) {
            Some(stream_info) => stream_info.browser_proxy_sequence_offset as u64,
            None => return,
        };
        let browser_sequence_number = match response
            .sequenced_packet
            .sequence_number
            .checked_add(browser_proxy_sequence_offset)
        {
            Some(sequence_number) => sequence_number,
            None => {
                warning!(
                    self.logger,
                    "Discarding ClientResponsePayload for stream key {} because sequence number {} cannot accommodate its CONNECT offset",
                    response.stream_key,
                    response.sequenced_packet.sequence_number
                );
                return;
            }
        };
        let response_sequence_admission = match self.stream_info_mut(&response.stream_key) {
            Some(stream_info) => stream_info
                .response_sequence_replay_window
                .admit(response.sequenced_packet.sequence_number),
            None => return,
        };
        if let Err(reason) = response_sequence_admission {
            warning!(
                self.logger,
                "Discarding ClientResponsePayload for stream key {} because {}: {}",
                response.stream_key,
                reason,
                response.sequenced_packet.sequence_number
            );
            return;
        }
        if let Some(manager) = self.receipt_session_manager_opt.as_mut() {
            if let Ok(now_unix_s) = Self::unix_time_now() {
                if let Err(error) = manager.record_exit_response(
                    &response.stream_key,
                    response.sequenced_packet.sequence_number,
                    response.sequenced_packet.data.len() as u64,
                    now_unix_s,
                ) {
                    if error != ReceiptSessionManagerError::ReceiptRouteNotFound {
                        warning!(
                            self.logger,
                            "Unable to correlate exit response with receipt session: {}",
                            error
                        );
                    }
                }
            }
        }
        let exit_public_key_opt = Self::find_exit_node_key(&expected_services);
        let correlated_route_activity =
            !response.sequenced_packet.data.is_empty() && exit_public_key_opt.is_some();
        let should_record_route_success_metadata = correlated_route_activity
            && self
                .stream_info_mut(&response.stream_key)
                .map(|info| {
                    if info.route_success_metadata_reported {
                        false
                    } else {
                        info.route_success_metadata_reported = true;
                        true
                    }
                })
                .unwrap_or(false);
        let should_emit_route_activity_heartbeat = correlated_route_activity
            && self.route_activity_heartbeat.should_emit(route_activity_at);
        let latency_ms_opt = if should_record_route_success_metadata {
            self.stream_info(&response.stream_key)
                .and_then(|info| info.request_started_at_opt)
                .and_then(|started_at| SystemTime::now().duration_since(started_at).ok())
                .map(|latency| latency.as_millis().min(u128::from(u32::MAX)) as u32)
        } else {
            None
        };
        let recovered_exit_opt = exit_public_key_opt.and_then(|key| {
            self.stream_info(&response.stream_key)
                .and_then(|info| info.route_opt.as_ref())
                .map(|route| (key, route.host.name.clone()))
        });
        if let (Some((public_key, hostname)), Some(subs)) = (recovered_exit_opt, self.subs.as_ref())
        {
            let metadata_change = match latency_ms_opt {
                Some(latency_ms) => NRMetadataChange::RecordRouteSuccess {
                    hostname,
                    latency_ms,
                },
                None => NRMetadataChange::RemoveUnreachableHost { hostname },
            };
            if subs
                .update_node_record_metadata
                .try_send(UpdateNodeRecordMetadataMessage {
                    public_key,
                    metadata_change,
                })
                .is_err()
            {
                warning!(
                    self.logger,
                    "Could not report route metadata to Neighborhood"
                );
            }
        }
        if should_emit_route_activity_heartbeat {
            if self
                .subs
                .as_ref()
                .expect("ProxyServer Subs Unbound")
                .route_use_succeeded
                .try_send(RouteUseSucceededMessage)
                .is_err()
            {
                warning!(
                    self.logger,
                    "Could not report route-use success to Neighborhood"
                );
            }
        }
        self.report_response_services_consumed(
            &expected_services,
            response.sequenced_packet.data.len(),
            payload_data_len,
        );
        let stream_key = response.stream_key;
        if let Some(info) = self.stream_info_mut(&stream_key) {
            if let Err(e) = ProxyServer::remove_dns_failure_retry(info, &stream_key) {
                trace!(
                    self.logger,
                    "No DNS retry entry found for stream key {} during a successful attempt: {}",
                    &stream_key,
                    e
                );
            }
        }
        if let Some(info) = self.stream_info(&stream_key) {
            if let Some(old_timestamp) = info.time_to_live_opt {
                self.log_straggling_packet(&stream_key, payload_data_len, &old_timestamp)
            } else {
                match self.keys_and_addrs.a_to_b(&stream_key) {
                    Some(socket_addr) => {
                        let last_data = response.sequenced_packet.last_data;
                        let sequence_number_opt = Some(browser_sequence_number);
                        let delivered_bytes = response.sequenced_packet.data.len();
                        self.subs
                            .as_ref()
                            .expect("Dispatcher unbound in ProxyServer")
                            .dispatcher
                            .try_send(TransmitDataMsg {
                                endpoint: Endpoint::Socket(socket_addr),
                                last_data,
                                sequence_number_opt,
                                data: response.sequenced_packet.data,
                            })
                            .expect("Dispatcher is dead");
                        crate::mobile_runtime::report_bytes_down(delivered_bytes);
                        if last_data {
                            self.purge_stream_key(
                                &stream_key,
                                "last data received from the exit node",
                            );
                        }
                    }
                    None => {
                        // TODO GH-608: It would be really nice to be able to send an InboundClientData with last_data: true
                        // back to the ProxyClient (and the distant server) so that the server could shut down
                        // its stream, since the browser has shut down _its_ stream and no more data will
                        // ever be accepted from the server on that stream; but we don't have enough information
                        // to do so, since our stream key has been purged and all the information it keyed
                        // is gone. Sorry, server!
                        warning!(self.logger,
                            "Discarding {}-byte packet {} from an unrecognized stream key: {:?}; can't send response back to client",
                            response.sequenced_packet.data.len(),
                            response.sequenced_packet.sequence_number,
                            response.stream_key,
                        )
                    }
                }
            }
        }
    }

    fn tls_connect(&mut self, msg: &InboundClientData) {
        let http_data = HttpProtocolPack {}.find_host(&msg.data.clone().into());
        match http_data {
            Some(ref host) if host.port == TLS_PORT => {
                let stream_key = self.find_or_generate_stream_key(msg);
                match self.stream_info_mut(&stream_key) {
                    None => return,
                    Some(stream_info) => {
                        stream_info.tunneled_host_opt = Some(host.name.clone());
                        stream_info.browser_proxy_sequence_offset = true;
                    }
                }
                self.subs
                    .as_ref()
                    .expect("Dispatcher unbound in ProxyServer")
                    .dispatcher
                    .try_send(TransmitDataMsg {
                        endpoint: Endpoint::Socket(msg.client_addr),
                        last_data: false,
                        sequence_number_opt: msg.sequence_number_opt,
                        data: b"HTTP/1.1 200 OK\r\n\r\n".to_vec(),
                    })
                    .expect("Dispatcher is dead");
            }
            _ => {
                self.subs
                    .as_ref()
                    .expect("Dispatcher unbound in ProxyServer")
                    .dispatcher
                    .try_send(TransmitDataMsg {
                        endpoint: Endpoint::Socket(msg.client_addr),
                        last_data: true,
                        sequence_number_opt: msg.sequence_number_opt,
                        data: b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n".to_vec(),
                    })
                    .expect("Dispatcher is dead");
            }
        }
    }

    fn out_subs(&self, actor_name: &str) -> &ProxyServerOutSubs {
        self.subs
            .as_ref()
            .unwrap_or_else(|| panic!("{} unbound in ProxyServer", actor_name))
    }

    fn handle_stream_shutdown_msg(&mut self, msg: StreamShutdownMsg) {
        let nca = match msg.stream_type {
            RemovedStreamType::Clandestine => {
                panic!("ProxyServer should never get ShutdownStreamMsg about clandestine stream")
            }
            RemovedStreamType::NonClandestine(nca) => nca,
        };
        let stream_key = match self.keys_and_addrs.b_to_a(&msg.peer_addr) {
            None => {
                warning!(
                    self.logger,
                    "Received instruction to shut down nonexistent stream; peer redacted - ignoring"
                );
                return;
            }
            Some(sk) => sk,
        };
        self.schedule_stream_key_purge(stream_key);
        if msg.report_to_counterpart {
            debug!(
                self.logger,
                "Reporting shutdown of {} to counterpart", &stream_key
            );
            let ibcd = InboundClientData {
                timestamp: SystemTime::now(),
                client_addr: msg.peer_addr,
                reception_port_opt: Some(nca.reception_port),
                last_data: true,
                is_clandestine: false,
                sequence_number_opt: Some(nca.sequence_number),
                data: vec![],
            };
            if let Err(e) = self.help(|helper, proxy| helper.handle_normal_client_data(proxy, ibcd))
            {
                error!(self.logger, "{}", e)
            };
        }
    }

    fn find_or_generate_stream_key(&mut self, ibcd: &InboundClientData) -> StreamKey {
        match self.keys_and_addrs.b_to_a(&ibcd.client_addr) {
            Some(stream_key) => {
                debug!(
                    self.logger,
                    "find_or_generate_stream_key() retrieved existing mapping; stream and client redacted"
                );
                stream_key
            }
            None => {
                let stream_key = self.stream_key_factory.make(
                    self.cryptde_pair.main.as_ref().public_key(),
                    ibcd.client_addr,
                );
                self.keys_and_addrs.insert(stream_key, ibcd.client_addr);
                self.stream_info.insert(
                    stream_key,
                    StreamInfo {
                        tunneled_host_opt: None,
                        dns_failure_retry_opt: None,
                        route_opt: None,
                        protocol_opt: None,
                        browser_proxy_sequence_offset: false,
                        response_sequence_replay_window: ResponseSequenceReplayWindow::default(),
                        request_started_at_opt: Some(ibcd.timestamp),
                        time_to_live_opt: None,
                        route_success_metadata_reported: false,
                    },
                );
                debug!(
                    self.logger,
                    "find_or_generate_stream_key() inserted new mapping; stream and client redacted"
                );
                stream_key
            }
        }
    }

    fn purge_stream_key(&mut self, stream_key: &StreamKey, reason: &str) {
        debug!(
            self.logger,
            "Retiring stream key {} due to {}", &stream_key, reason
        );
        let _ = self.keys_and_addrs.remove_a(stream_key);
        let _ = self.stream_info.remove(stream_key);
        let _ = self.pending_route_requests.remove(stream_key);
    }

    fn make_payload(
        &mut self,
        ibcd: InboundClientData,
        stream_key: &StreamKey,
    ) -> Result<ClientRequestPayload_0v1, String> {
        let stream_info_opt = self.stream_info.get(stream_key);
        let existing_receipt_quote_opt = stream_info_opt
            .and_then(|stream_info| stream_info.route_opt.as_ref())
            .and_then(Self::receipt_exit_quote);
        let (host_opt, tunnelled_host_opt) = match stream_info_opt {
            None => (None, None),
            Some(info) => match &info.route_opt {
                Some(route) => (Some(route.host.clone()), info.tunneled_host_opt.clone()),
                None => (None, info.tunneled_host_opt.clone()),
            },
        };
        let new_ibcd = match tunnelled_host_opt {
            Some(_) => InboundClientData {
                reception_port_opt: Some(TLS_PORT),
                ..ibcd
            },
            None => ibcd,
        };
        match self.client_request_payload_factory.make(
            &new_ibcd,
            *stream_key,
            host_opt,
            self.cryptde_pair.alias.as_ref(),
            &self.logger,
        ) {
            None => Err("Couldn't create ClientRequestPayload".to_string()),
            Some(mut payload) => {
                payload.receipt_session_request_opt =
                    match self.receipt_session_manager_opt.as_mut() {
                        Some(manager) => {
                            let now_unix_s = Self::unix_time_now()?;
                            let request_opt = manager
                                .request_for_stream(*stream_key, now_unix_s)
                                .map_err(|error| error.to_string())?;
                            if request_opt.is_some() {
                                if let Some((provider, service_rate, byte_rate)) =
                                    existing_receipt_quote_opt
                                {
                                    manager
                                        .bind_exit_quote(
                                            stream_key,
                                            provider,
                                            service_rate,
                                            byte_rate,
                                            now_unix_s,
                                        )
                                        .map_err(|error| error.to_string())?;
                                }
                            }
                            request_opt
                        }
                        None => None,
                    };
                match tunnelled_host_opt {
                    Some(hostname) => Ok(ClientRequestPayload_0v1 {
                        target_hostname: hostname,
                        ..payload
                    }),
                    None => Ok(payload),
                }
            }
        }
    }

    fn get_next_return_route_id(&self) -> u32 {
        let return_route_id = self.next_return_route_id.get();
        self.next_return_route_id
            .set(return_route_id.wrapping_add(1));
        return_route_id
    }

    fn get_next_route_request_id(&self) -> Result<u64, String> {
        let route_request_id = self.next_route_request_id.get();
        let next_route_request_id = route_request_id
            .checked_add(1)
            .ok_or_else(|| "Route request identifier overflow".to_string())?;
        self.next_route_request_id.set(next_route_request_id);
        Ok(route_request_id)
    }

    fn queue_pending_route_packet(
        &mut self,
        mut args: TransmitToHopperArgs,
    ) -> Result<Option<TransmitToHopperArgs>, String> {
        let stream_key = args.payload.stream_key;
        let payload_size = args.payload.sequenced_packet.data.len();
        if let Some(pending) = self.pending_route_requests.get_mut(&stream_key) {
            let queued_payload_bytes = pending
                .queued_payload_bytes
                .checked_add(payload_size)
                .ok_or_else(|| "Pending route payload byte count overflow".to_string())?;
            if pending.packets.len() >= MAX_PENDING_ROUTE_PACKETS_PER_STREAM
                || queued_payload_bytes > MAX_PENDING_ROUTE_BYTES_PER_STREAM
            {
                let message = format!(
                    "Pending route queue for stream {} exceeded {} packets or {} bytes",
                    stream_key,
                    MAX_PENDING_ROUTE_PACKETS_PER_STREAM,
                    MAX_PENDING_ROUTE_BYTES_PER_STREAM
                );
                if let Err(error) = Self::send_route_failure(
                    args.payload.protocol,
                    &args.payload.target_hostname,
                    args.client_addr,
                    &args.dispatcher_sub,
                ) {
                    warning!(
                        self.logger,
                        "Unable to notify browser of pending-route overflow for stream {}: {}",
                        stream_key,
                        error
                    );
                }
                self.purge_stream_key(&stream_key, "pending route queue overflow");
                return Err(message);
            }
            args.route_request_id = pending.route_request_id;
            pending.queued_payload_bytes = queued_payload_bytes;
            pending.packets.push(args);
            return Ok(None);
        }
        if payload_size > MAX_PENDING_ROUTE_BYTES_PER_STREAM {
            let message = format!(
                "Initial route payload for stream {} exceeded {} bytes",
                stream_key, MAX_PENDING_ROUTE_BYTES_PER_STREAM
            );
            if let Err(error) = Self::send_route_failure(
                args.payload.protocol,
                &args.payload.target_hostname,
                args.client_addr,
                &args.dispatcher_sub,
            ) {
                warning!(
                    self.logger,
                    "Unable to notify browser of initial-route overflow for stream {}: {}",
                    stream_key,
                    error
                );
            }
            self.purge_stream_key(&stream_key, "pending route payload overflow");
            return Err(message);
        }
        let route_request_id = self.get_next_route_request_id()?;
        args.route_request_id = route_request_id;
        let resolver_args = args.clone();
        self.pending_route_requests.insert(
            stream_key,
            PendingRouteRequest {
                route_request_id,
                queued_payload_bytes: payload_size,
                packets: vec![args],
            },
        );
        Ok(Some(resolver_args))
    }

    fn try_transmit_to_hopper(
        args: TransmitToHopperArgs,
        route_query_response: RouteQueryResponse,
    ) -> Result<(), String> {
        match route_query_response.expected_services {
            ExpectedServices::RoundTrip(over, _) => {
                ProxyServer::transmit_to_hopper(args, route_query_response.route, over)
            }
            ExpectedServices::OneWay(_) => {
                Err("Expected RoundTrip ExpectedServices but got OneWay".to_string())
            }
        }
    }

    fn response_services_are_valid(response_services: &[ExpectedService]) -> bool {
        let exit_count = response_services
            .iter()
            .filter(|service| matches!(service, ExpectedService::Exit(..)))
            .count();
        let first_billable_service = response_services
            .iter()
            .find(|service| !matches!(service, ExpectedService::Nothing));
        exit_count <= 1 && !matches!(first_billable_service, Some(ExpectedService::Routing(..)))
    }

    fn route_can_transmit_request(
        route_query_response: &RouteQueryResponse,
        is_decentralized: bool,
    ) -> bool {
        match &route_query_response.expected_services {
            ExpectedServices::RoundTrip(request_services, response_services) => {
                let request_exit_count = request_services
                    .iter()
                    .filter(|service| matches!(service, ExpectedService::Exit(..)))
                    .count();
                (!is_decentralized || request_exit_count == 1)
                    && Self::response_services_are_valid(response_services)
            }
            ExpectedServices::OneWay(_) => false,
        }
    }

    fn report_on_routing_services(
        expected_services: Vec<ExpectedService>,
        logger: &Logger,
    ) -> Vec<RoutingServiceConsumed> {
        let report_of_routing_services: Vec<RoutingServiceConsumed> = expected_services
            .into_iter()
            .filter_map(|service| match service {
                ExpectedService::Routing(_, earning_wallet, rate_pack) => {
                    Some(RoutingServiceConsumed {
                        earning_wallet,
                        service_rate: rate_pack.routing_service_rate,
                        byte_rate: rate_pack.routing_byte_rate,
                    })
                }
                _ => None,
            })
            .collect();
        if report_of_routing_services.is_empty() {
            debug!(logger, "No routing services requested.");
        }
        report_of_routing_services
    }

    fn report_on_exit_service(
        expected_services: &[ExpectedService],
        payload_size: usize,
    ) -> Result<ExitServiceConsumed, String> {
        let mut exits = expected_services
            .iter()
            .filter_map(|service| match service {
                ExpectedService::Exit(_, earning_wallet, rate_pack) => {
                    Some((earning_wallet, rate_pack))
                }
                _ => None,
            });
        let (earning_wallet, rate_pack) = exits
            .next()
            .ok_or_else(|| "Route does not demand an exit service".to_string())?;
        if exits.next().is_some() {
            return Err("Route demands more than one exit service".to_string());
        }
        Ok(ExitServiceConsumed {
            earning_wallet: earning_wallet.clone(),
            payload_size,
            service_rate: rate_pack.exit_service_rate,
            byte_rate: rate_pack.exit_byte_rate,
        })
    }

    fn transmit_to_hopper(
        args: TransmitToHopperArgs,
        route: Route,
        expected_services: Vec<ExpectedService>,
    ) -> Result<(), String> {
        let logger = args.logger;
        let destination_key_opt = if args.is_decentralized {
            expected_services.iter().find_map(|service| match service {
                ExpectedService::Exit(public_key, _, _) => Some(public_key.clone()),
                _ => None,
            })
        } else {
            // In Zero Hop Mode the exit node public key is the same as this public key
            Some(args.main_cryptde.public_key().clone())
        };
        match destination_key_opt {
            None => {
                // Route not found
                Err(ProxyServer::handle_route_failure(
                    args.payload,
                    args.client_addr,
                    &args.dispatcher_sub,
                ))
            }
            Some(payload_destination_key) => {
                // Route found
                debug!(logger, "Transmitting to Hopper; destination key redacted");
                let payload = args.payload;
                let payload_size = payload.sequenced_packet.data.len();
                let receipt_metered = payload.receipt_session_request_opt.is_some();
                let stream_key = payload.stream_key;
                let target_hostname = payload.target_hostname.clone();
                let protocol = payload.protocol;
                let pkg = match IncipientCoresPackage::new(
                    args.main_cryptde.as_ref(),
                    route,
                    payload.into(),
                    &payload_destination_key,
                ) {
                    Ok(pkg) => pkg,
                    Err(error) => {
                        let notification_failed = Self::send_route_failure(
                            protocol,
                            &target_hostname,
                            args.client_addr,
                            &args.dispatcher_sub,
                        )
                        .is_err();
                        let notification_suffix = if notification_failed {
                            "; browser notification failed; details redacted"
                        } else {
                            ""
                        };
                        return Err(format!(
                            "Could not create CORES package for stream {}: {}{}",
                            stream_key, error, notification_suffix
                        ));
                    }
                };
                if receipt_metered && payload_size > 0 {
                    let payload_size = u64::try_from(payload_size).map_err(|_| {
                        "request payload size does not fit in a receipt".to_string()
                    })?;
                    args.record_exit_request_for_receipt
                        .try_send(RecordExitRequestForReceipt {
                            stream_key,
                            payload_size,
                            routing_payload_size: u64::try_from(pkg.payload.len()).map_err(
                                |_| "routing payload size does not fit in a receipt".to_string(),
                            )?,
                        })
                        .map_err(|_| "ProxyServer receipt observer is dead".to_string())?;
                }
                if args.is_decentralized {
                    let exit =
                        ProxyServer::report_on_exit_service(&expected_services, payload_size)?;
                    let routing =
                        ProxyServer::report_on_routing_services(expected_services, &logger);
                    args.accountant_sub
                        .try_send(ReportServicesConsumedMessage {
                            timestamp: args.timestamp,
                            exit,
                            routing_payload_size: pkg.payload.len(),
                            routing,
                        })
                        .expect("Accountant is dead");
                }
                args.hopper_sub.try_send(pkg).expect("Hopper is dead");
                crate::mobile_runtime::report_bytes_up(payload_size);
                if let Some(shutdown_sub) = args.retire_stream_key_sub_opt {
                    debug!(
                        logger,
                        "Last data is on the way; directing shutdown of stream {}", stream_key
                    );
                    shutdown_sub
                        .try_send(StreamShutdownMsg {
                            peer_addr: args.client_addr,
                            stream_type: RemovedStreamType::NonClandestine(
                                NonClandestineAttributes {
                                    // No report to counterpart; these are irrelevant
                                    reception_port: 0,
                                    sequence_number: 0,
                                },
                            ),
                            report_to_counterpart: false,
                        })
                        .expect("Proxy Server is dead");
                }
                Ok(())
            }
        }
    }

    fn handle_route_failure(
        payload: ClientRequestPayload_0v1,
        source_addr: SocketAddr,
        dispatcher: &Recipient<TransmitDataMsg>,
    ) -> String {
        let target_hostname = payload.target_hostname.clone();
        let notification_failed = ProxyServer::send_route_failure(
            payload.protocol,
            &target_hostname,
            source_addr,
            dispatcher,
        )
        .is_err();
        let mut result = "Failed to find route; destination and stream redacted".to_string();
        if notification_failed {
            result.push_str("; browser notification failed; details redacted");
        }
        result
    }

    fn send_route_failure(
        protocol: ProxyProtocol,
        target_hostname: &str,
        source_addr: SocketAddr,
        dispatcher: &Recipient<TransmitDataMsg>,
    ) -> Result<(), String> {
        let data = from_protocol(protocol)
            .server_impersonator()
            .route_query_failure_response(target_hostname);
        let msg = TransmitDataMsg {
            endpoint: Endpoint::Socket(source_addr),
            last_data: true,
            sequence_number_opt: Some(0),
            data,
        };
        dispatcher
            .try_send(msg)
            .map_err(|_| "Dispatcher is dead".to_string())
    }

    fn get_expected_return_services(
        &mut self,
        stream_key: &StreamKey,
    ) -> Option<Vec<ExpectedService>> {
        match self.stream_info(stream_key) {
            None => {
                error!(self.logger, "Can't pay for return services consumed: received response with unrecognized stream key {:?}. Ignoring", stream_key);
                None
            }
            Some(stream_info) => match stream_info.route_opt.as_ref() {
                None => {
                    error!(self.logger, "Can't pay for return services consumed: received response for stream key {:?} before a return route was stored. Ignoring", stream_key);
                    None
                }
                Some(route) => match &route.expected_services {
                    ExpectedServices::RoundTrip(_, return_services)
                        if Self::response_services_are_valid(return_services) =>
                    {
                        Some(return_services.clone())
                    }
                    ExpectedServices::RoundTrip(_, _) => {
                        error!(self.logger, "Can't pay for return services consumed: received response for stream key {:?} whose return-service shape is invalid. Ignoring", stream_key);
                        None
                    }
                    ExpectedServices::OneWay(_) => {
                        error!(self.logger, "Can't pay for return services consumed: received response for stream key {:?} whose route has no return services. Ignoring", stream_key);
                        None
                    }
                },
            },
        }
    }

    fn report_response_services_consumed(
        &self,
        expected_services: &[ExpectedService],
        exit_size: usize,
        routing_size: usize,
    ) {
        if !Self::response_services_are_valid(expected_services) {
            warning!(
                self.logger,
                "Refusing to account for an invalid return-service shape"
            );
            return;
        }
        let exit_service_report = match expected_services
            .iter()
            .find(|service| !matches!(service, ExpectedService::Nothing))
        {
            None => return,
            Some(ExpectedService::Exit(_, wallet, rate_pack)) => ExitServiceConsumed {
                earning_wallet: wallet.clone(),
                payload_size: exit_size,
                service_rate: rate_pack.exit_service_rate,
                byte_rate: rate_pack.exit_byte_rate,
            },
            Some(ExpectedService::Routing(..)) => {
                warning!(
                    self.logger,
                    "Refusing to account for an invalid return-service shape"
                );
                return;
            }
            Some(ExpectedService::Nothing) => {
                // `find` explicitly excludes this variant.
                return;
            }
        };
        let routing_service_reports = expected_services
            .iter()
            .flat_map(|service| match service {
                ExpectedService::Routing(_, wallet, rate_pack) => Some(RoutingServiceConsumed {
                    earning_wallet: wallet.clone(),
                    service_rate: rate_pack.routing_service_rate,
                    byte_rate: rate_pack.routing_byte_rate,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        let report_message = ReportServicesConsumedMessage {
            timestamp: SystemTime::now(),
            exit: exit_service_report,
            routing_payload_size: routing_size,
            routing: routing_service_reports,
        };
        self.subs
            .as_ref()
            .expect("Accountant is unbound")
            .accountant
            .try_send(report_message)
            .expect("Accountant is dead");
    }
}

impl MutabilityConflictHelper<Box<dyn IBCDHelper>> for ProxyServer {
    type Result = Result<(), String>;

    fn helper_access(&mut self) -> &mut Option<Box<dyn IBCDHelper>> {
        &mut self.inbound_client_data_helper_opt
    }
}

pub trait IBCDHelper {
    fn handle_normal_client_data(
        &self,
        proxy_s: &mut ProxyServer,
        msg: InboundClientData,
    ) -> Result<(), String>;

    fn request_route_and_transmit(
        &self,
        args: TransmitToHopperArgs,
        route_source: Recipient<RouteQueryMessage>,
        proxy_server_sub: Recipient<AddRouteResultMessage>,
    );
}

trait RouteQueryResponseResolver: Send {
    fn resolve_message(
        &self,
        args: TransmitToHopperArgs,
        // add_return_route_sub: Recipient<AddReturnRouteMessage>,
        proxy_server_sub: Recipient<AddRouteResultMessage>,
        route_result_opt: Result<Option<RouteQueryResponse>, MailboxError>,
    );
}
struct RouteQueryResponseResolverReal {}

impl RouteQueryResponseResolver for RouteQueryResponseResolverReal {
    fn resolve_message(
        &self,
        args: TransmitToHopperArgs,
        proxy_server_sub: Recipient<AddRouteResultMessage>,
        route_result_opt: Result<Option<RouteQueryResponse>, MailboxError>,
    ) {
        let stream_key = args.payload.stream_key;
        let route_request_id = args.route_request_id;
        let result = match route_result_opt {
            Ok(Some(route_query_response))
                if ProxyServer::route_can_transmit_request(
                    &route_query_response,
                    args.is_decentralized,
                ) =>
            {
                // A round-trip route contains 2N+1 encrypted route entries for
                // N outward hops. Record the real selected path length instead
                // of presenting the configured minimum as observed telemetry.
                crate::mobile_runtime::report_route_hops(
                    route_query_response.route.hops.len().saturating_sub(1) / 2,
                );
                Ok(route_query_response)
            }
            Ok(Some(_)) | Ok(None) => {
                Err("Failed to find route; destination and stream redacted".to_string())
            }
            Err(_error) => {
                Err("Neighborhood refused to answer route request; details redacted".to_string())
            }
        };
        proxy_server_sub
            .try_send(AddRouteResultMessage {
                stream_key,
                route_request_id,
                result,
            })
            .expect("ProxyServer is dead");
    }
}

trait RouteQueryResponseResolverFactory {
    fn make(&self) -> Box<dyn RouteQueryResponseResolver>;
}
struct RouteQueryResponseResolverFactoryReal {}

impl RouteQueryResponseResolverFactory for RouteQueryResponseResolverFactoryReal {
    fn make(&self) -> Box<dyn RouteQueryResponseResolver> {
        Box::new(RouteQueryResponseResolverReal {})
    }
}
struct IBCDHelperReal {
    factory: Box<dyn RouteQueryResponseResolverFactory>,
}

impl IBCDHelperReal {
    fn new() -> Self {
        Self {
            factory: Box::new(RouteQueryResponseResolverFactoryReal {}),
        }
    }

    fn make_route_query(
        host: Host,
        payload_size: usize,
        receipt_session_request_opt: Option<crate::sub_lib::service_receipt::ReceiptSessionRequest>,
    ) -> RouteQueryMessage {
        let route_query = RouteQueryMessage::data_indefinite_route_request(host, payload_size);
        match receipt_session_request_opt {
            Some(request) => route_query.require_bilateral_service_receipts_v1(request),
            None => route_query,
        }
    }
}

impl IBCDHelper for IBCDHelperReal {
    fn handle_normal_client_data(
        &self,
        proxy_server: &mut ProxyServer,
        msg: InboundClientData,
    ) -> Result<(), String> {
        let client_addr = msg.client_addr;
        let last_data = msg.last_data;
        if proxy_server.consuming_wallet_balance.is_none() && proxy_server.is_decentralized {
            let protocol_pack = match from_ibcd(&msg) {
                Err(e) => return Err(e),
                Ok(pp) => pp,
            };
            let data = protocol_pack
                .server_impersonator()
                .consuming_wallet_absent();
            let msg = TransmitDataMsg {
                endpoint: Endpoint::Socket(client_addr),
                last_data: true,
                sequence_number_opt: Some(0),
                data,
            };
            proxy_server
                .out_subs("Dispatcher")
                .dispatcher
                .try_send(msg)
                .expect("Dispatcher is dead");
            return Err("Browser request rejected due to missing consuming wallet".to_string());
        }
        let stream_key = proxy_server.find_or_generate_stream_key(&msg);
        let timestamp = msg.timestamp;
        let mut payload = match proxy_server.make_payload(msg, &stream_key) {
            Ok(payload) => {
                if !proxy_server.is_running_in_integration_test {
                    if let Err(e) =
                        Hostname::new(&payload.target_hostname).validate_non_loopback_host()
                    {
                        return Err(format!("Request to wildcard IP detected - {} (Most likely because Blockchain Service URL is not set)", e));
                    }
                }
                payload
            }
            Err(e) => return Err(e),
        };

        {
            let is_decentralized = proxy_server.is_decentralized;
            let stream_info = proxy_server
                .stream_info_mut(&stream_key)
                .unwrap_or_else(|| panic!("Stream key {} disappeared!", &stream_key));
            let active_attempt_id = stream_info
                .dns_failure_retry_opt
                .as_ref()
                .map(|retry| retry.active_attempt_id)
                .unwrap_or(0);
            payload.dns_attempt_id_opt = Some(active_attempt_id);
            if stream_info.dns_failure_retry_opt.is_none() {
                let dns_failure_retry = DNSFailureRetry {
                    unsuccessful_request: payload.clone(),
                    retries_left: if is_decentralized {
                        DNS_FAILURE_RETRIES
                    } else {
                        0
                    },
                    active_attempt_id,
                };
                stream_info.dns_failure_retry_opt = Some(dns_failure_retry);
                stream_info.protocol_opt = Some(payload.protocol);
            }
        }
        let args =
            TransmitToHopperArgs::new(proxy_server, payload, client_addr, timestamp, last_data);
        let pld = &args.payload;
        let stream_info = proxy_server
            .stream_info(&pld.stream_key)
            .unwrap_or_else(|| panic!("Stream key {} disappeared!", &pld.stream_key));
        if let Some(route_query_response) = &stream_info.route_opt {
            debug!(
                proxy_server.logger,
                "Transmitting down existing stream {}: sequence {}, length {}",
                pld.stream_key,
                pld.sequenced_packet.sequence_number,
                pld.sequenced_packet.data.len()
            );
            let route_query_response = route_query_response.clone();
            ProxyServer::try_transmit_to_hopper(args, route_query_response)
        } else {
            let route_source = proxy_server.out_subs("Neighborhood").route_source.clone();
            let proxy_server_sub = proxy_server
                .out_subs("ProxyServer")
                .route_result_sub
                .clone();
            if let Some(resolver_args) = proxy_server.queue_pending_route_packet(args)? {
                self.request_route_and_transmit(resolver_args, route_source, proxy_server_sub);
            }
            Ok(())
        }
    }

    fn request_route_and_transmit(
        &self,
        args: TransmitToHopperArgs,
        neighborhood_sub: Recipient<RouteQueryMessage>,
        proxy_server_sub: Recipient<AddRouteResultMessage>,
    ) {
        let pld = &args.payload;
        let host = Host::new(&pld.target_hostname, pld.target_port);
        let logger = args.logger.clone();
        debug!(
            logger,
            "Getting route and opening new stream with key {} to transmit: sequence {}, length {}",
            pld.stream_key,
            pld.sequenced_packet.sequence_number,
            pld.sequenced_packet.data.len()
        );
        let payload_size = pld.sequenced_packet.data.len();
        let receipt_session_request_opt = args.payload.receipt_session_request_opt.clone();
        let message_resolver = self.factory.make();

        let route_query = Self::make_route_query(host, payload_size, receipt_session_request_opt);

        tokio::spawn(
            neighborhood_sub
                .send(route_query)
                .then(move |route_result| {
                    message_resolver.resolve_message(args, proxy_server_sub, route_result);
                    Ok(())
                }),
        );
    }
}

pub struct TransmitToHopperArgs {
    pub main_cryptde: Box<dyn CryptDE>,
    pub payload: ClientRequestPayload_0v1,
    pub return_route_id: u32,
    pub route_request_id: u64,
    pub client_addr: SocketAddr,
    pub timestamp: SystemTime,
    pub is_decentralized: bool,
    pub require_service_receipt_capability: bool,
    pub logger: Logger,
    pub retire_stream_key_sub_opt: Option<Recipient<StreamShutdownMsg>>,
    pub hopper_sub: Recipient<IncipientCoresPackage>,
    pub dispatcher_sub: Recipient<TransmitDataMsg>,
    pub accountant_sub: Recipient<ReportServicesConsumedMessage>,
    pub record_exit_request_for_receipt: Recipient<RecordExitRequestForReceipt>,
}

impl Clone for TransmitToHopperArgs {
    fn clone(&self) -> Self {
        Self {
            main_cryptde: self.main_cryptde.dup(),
            payload: self.payload.clone(),
            return_route_id: self.return_route_id,
            route_request_id: self.route_request_id,
            client_addr: self.client_addr,
            timestamp: self.timestamp,
            is_decentralized: self.is_decentralized,
            require_service_receipt_capability: self.require_service_receipt_capability,
            logger: self.logger.clone(),
            retire_stream_key_sub_opt: self.retire_stream_key_sub_opt.clone(),
            hopper_sub: self.hopper_sub.clone(),
            dispatcher_sub: self.dispatcher_sub.clone(),
            accountant_sub: self.accountant_sub.clone(),
            record_exit_request_for_receipt: self.record_exit_request_for_receipt.clone(),
        }
    }
}

impl TransmitToHopperArgs {
    pub fn new(
        proxy_server: &mut ProxyServer,
        payload: ClientRequestPayload_0v1,
        client_addr: SocketAddr,
        timestamp: SystemTime,
        retire_stream_key: bool,
    ) -> Self {
        let retire_stream_key_sub_opt = if retire_stream_key {
            Some(
                proxy_server
                    .out_subs("ProxyServer")
                    .stream_shutdown_sub
                    .clone(),
            )
        } else {
            None
        };
        let return_route_id = proxy_server.get_next_return_route_id();
        let require_service_receipt_capability = payload.receipt_session_request_opt.is_some();
        Self {
            main_cryptde: proxy_server.cryptde_pair.main.dup(),
            payload,
            return_route_id,
            route_request_id: 0,
            client_addr,
            timestamp,
            logger: proxy_server.logger.clone(),
            retire_stream_key_sub_opt,
            hopper_sub: proxy_server.out_subs("Hopper").hopper.clone(),
            dispatcher_sub: proxy_server.out_subs("Dispatcher").dispatcher.clone(),
            accountant_sub: proxy_server.out_subs("Accountant").accountant.clone(),
            record_exit_request_for_receipt: proxy_server
                .out_subs("ProxyServer")
                .record_exit_request_for_receipt
                .clone(),
            is_decentralized: proxy_server.is_decentralized,
            require_service_receipt_capability,
        }
    }
}

trait StreamKeyFactory: Send {
    fn make(&self, public_key: &PublicKey, client_addr: SocketAddr) -> StreamKey;
}

struct StreamKeyFactoryReal {}

impl StreamKeyFactory for StreamKeyFactoryReal {
    fn make(&self, public_key: &PublicKey, client_addr: SocketAddr) -> StreamKey {
        StreamKey::new(public_key, client_addr)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DNSFailureRetry {
    unsuccessful_request: ClientRequestPayload_0v1,
    retries_left: usize,
    active_attempt_id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Hostname {
    hostname: String,
}

impl Hostname {
    fn new(raw_url: &str) -> Self {
        let regex = Regex::new(
            r"^((http[s]?|ftp):/)?/?([^:/\s]+)((/\w+)*/)([\w\-.]+[^#?\s]+)(.*)?(#[\w\-]+)?$",
        )
        .expect("Bad Regex");
        let hostname = match regex.captures(raw_url) {
            None => raw_url.to_string(),
            Some(capture) => match capture.get(3) {
                None => raw_url.to_string(),
                Some(m) => m.as_str().to_string(),
            },
        };
        Self { hostname }
    }

    fn validate_non_loopback_host(&self) -> Result<(), String> {
        match IpAddr::from_str(&self.hostname) {
            Ok(ip_addr) => match ip_addr {
                IpAddr::V4(ipv4addr) => Self::validate_ipv4(ipv4addr),
                IpAddr::V6(ipv6addr) => Self::validate_ipv6(ipv6addr),
            },
            Err(_) => Self::validate_raw_string(&self.hostname),
        }
    }

    fn validate_ipv4(addr: Ipv4Addr) -> Result<(), String> {
        if addr.octets() == [0, 0, 0, 0] {
            Err("0.0.0.0".to_string())
        } else if addr.octets() == [127, 0, 0, 1] {
            Err("127.0.0.1".to_string())
        } else {
            Ok(())
        }
    }

    fn validate_ipv6(addr: Ipv6Addr) -> Result<(), String> {
        if addr.segments() == [0, 0, 0, 0, 0, 0, 0, 0] {
            Err("::".to_string())
        } else if addr.segments() == [0, 0, 0, 0, 0, 0, 0, 1] {
            Err("::1".to_string())
        } else {
            Ok(())
        }
    }

    fn validate_raw_string(name: &str) -> Result<(), String> {
        if name == "localhost" {
            Err("localhost".to_string())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockchain::bip32::Bip32EncryptionKeyProvider;
    use crate::bootstrapper::CryptDEPair;
    use crate::match_lazily_every_type_id;
    use crate::proxy_server::protocol_pack::ServerImpersonator;
    use crate::proxy_server::server_impersonator_http::ServerImpersonatorHttp;
    use crate::proxy_server::server_impersonator_tls::ServerImpersonatorTls;
    use crate::stream_messages::{NonClandestineAttributes, RemovedStreamType};
    use crate::sub_lib::accountant::RoutingServiceConsumed;
    use crate::sub_lib::cryptde::{decodex, CryptData};
    use crate::sub_lib::cryptde::{encodex, PlainData};
    use crate::sub_lib::cryptde_null::CryptDENull;
    use crate::sub_lib::cryptde_real::CryptDEReal;
    use crate::sub_lib::dispatcher::Component;
    use crate::sub_lib::hop::LiveHop;
    use crate::sub_lib::hopper::MessageType;
    use crate::sub_lib::host::Host;
    use crate::sub_lib::neighborhood::ExpectedServices;
    use crate::sub_lib::neighborhood::{ExpectedService, RatePack, DEFAULT_RATE_PACK};
    use crate::sub_lib::proxy_client::{ClientResponsePayload_0v1, DnsResolveFailure_0v1};
    use crate::sub_lib::proxy_server::ClientRequestPayload_0v1;
    use crate::sub_lib::proxy_server::ProxyProtocol;
    use crate::sub_lib::route::Route;
    use crate::sub_lib::route::RouteSegment;
    use crate::sub_lib::sequence_buffer::SequencedPacket;
    use crate::sub_lib::service_receipt::{
        make_accounting_commitment, ReceiptSessionPolicy, ServiceKind, ServiceReceipt,
        ServiceReceiptPayload_0v1,
    };
    use crate::sub_lib::versioned_data::VersionedData;
    use crate::sub_lib::wallet::Wallet;
    use crate::test_utils::make_meaningless_route;
    use crate::test_utils::make_paying_wallet;
    use crate::test_utils::make_wallet;
    use crate::test_utils::rate_pack;
    use crate::test_utils::recorder::make_recorder;
    use crate::test_utils::recorder::peer_actors_builder;
    use crate::test_utils::recorder::Recorder;
    use crate::test_utils::recorder_stop_conditions::StopConditions;
    use crate::test_utils::unshared_test_utils::{
        make_request_payload, prove_that_crash_request_handler_is_hooked_up, AssertionsMessage,
    };
    use crate::test_utils::zero_hop_route_response;
    use actix::System;
    use lazy_static::lazy_static;
    use masq_lib::constants::{HTTP_PORT, TLS_PORT};
    use masq_lib::test_utils::logging::init_test_logging;
    use masq_lib::test_utils::logging::TestLogHandler;
    use masq_lib::test_utils::utils::TEST_DEFAULT_CHAIN;
    use std::any::TypeId;
    use std::cell::RefCell;
    use std::net::SocketAddr;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::SystemTime;

    lazy_static! {
        static ref CRYPTDE_PAIR: CryptDEPair = CryptDEPair::null();
    }

    impl Handler<AssertionsMessage<ProxyServer>> for ProxyServer {
        type Result = ();

        fn handle(
            &mut self,
            msg: AssertionsMessage<ProxyServer>,
            _ctx: &mut Self::Context,
        ) -> Self::Result {
            (msg.assertions)(self)
        }
    }

    #[derive(Default)]
    struct ReceiptAcknowledgementOutboxState {
        pending: Vec<crate::sub_lib::service_receipt::ServiceReceiptPayload_0v1>,
        enqueue_count: usize,
        delete_count: usize,
    }

    struct ReceiptAcknowledgementOutboxDaoMock {
        state: Arc<Mutex<ReceiptAcknowledgementOutboxState>>,
    }

    impl ReceiptAcknowledgementOutboxDao for ReceiptAcknowledgementOutboxDaoMock {
        fn enqueue(
            &mut self,
            payload: &crate::sub_lib::service_receipt::ServiceReceiptPayload_0v1,
            _created_at: SystemTime,
        ) -> Result<
            (),
            crate::accountant::db_access_objects::receipt_acknowledgement_outbox_dao::ReceiptAcknowledgementOutboxDaoError,
        >{
            let mut state = self.state.lock().unwrap();
            state.enqueue_count += 1;
            if !state.pending.contains(payload) {
                state.pending.push(payload.clone());
            }
            Ok(())
        }

        fn pending(
            &self,
        ) -> Result<
            Vec<crate::sub_lib::service_receipt::ServiceReceiptPayload_0v1>,
            crate::accountant::db_access_objects::receipt_acknowledgement_outbox_dao::ReceiptAcknowledgementOutboxDaoError,
        >{
            Ok(self.state.lock().unwrap().pending.clone())
        }

        fn delete(
            &mut self,
            payload: &crate::sub_lib::service_receipt::ServiceReceiptPayload_0v1,
        ) -> Result<
            (),
            crate::accountant::db_access_objects::receipt_acknowledgement_outbox_dao::ReceiptAcknowledgementOutboxDaoError,
        >{
            let mut state = self.state.lock().unwrap();
            state.delete_count += 1;
            state.pending.retain(|candidate| candidate != payload);
            Ok(())
        }
    }

    fn make_persisted_receipt_acknowledgement(provider: &CryptDEReal) -> ServiceReceiptPayload_0v1 {
        let payer = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let route_epoch = [0x71; 32];
        let acknowledged_receipt = ServiceReceipt::new(
            route_epoch,
            1,
            ServiceKind::Exit,
            provider.public_key().clone(),
            make_accounting_commitment(&route_epoch, payer.public_key()),
            128,
            5,
            2,
        )
        .sign(provider)
        .unwrap()
        .acknowledge(&payer)
        .unwrap();
        let wallet = make_paying_wallet(b"persisted acknowledgement wallet");
        let authorization = ReceiptSessionPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
            wallet.address(),
            payer.public_key().clone(),
            10_000,
            0,
            86_400,
            [0x72; 32],
        )
        .authorize(&wallet)
        .unwrap();
        ServiceReceiptPayload_0v1 {
            authorization,
            acknowledged_receipt,
        }
    }

    #[derive(Default)]
    struct RouteQueryResponseResolverFactoryMock {
        make_params: Arc<Mutex<Vec<()>>>,
        make_results: RefCell<Vec<Box<dyn RouteQueryResponseResolver>>>,
    }
    impl RouteQueryResponseResolverFactory for RouteQueryResponseResolverFactoryMock {
        fn make(&self) -> Box<dyn RouteQueryResponseResolver> {
            self.make_params.lock().unwrap().push(());
            self.make_results.borrow_mut().remove(0)
        }
    }

    impl RouteQueryResponseResolverFactoryMock {
        fn make_params(mut self, params: &Arc<Mutex<Vec<()>>>) -> Self {
            self.make_params = params.clone();
            self
        }
        fn make_result(self, result: Box<dyn RouteQueryResponseResolver>) -> Self {
            self.make_results.borrow_mut().push(result);
            self
        }
    }

    #[derive(Default)]
    struct RouteQueryResponseResolverMock {
        resolve_message_params: Arc<
            Mutex<
                Vec<(
                    TransmitToHopperArgs,
                    Result<Option<RouteQueryResponse>, MailboxError>,
                )>,
            >,
        >,
    }

    impl RouteQueryResponseResolver for RouteQueryResponseResolverMock {
        fn resolve_message(
            &self,
            args: TransmitToHopperArgs,
            _proxy_server_sub: Recipient<AddRouteResultMessage>,
            route_result: Result<Option<RouteQueryResponse>, MailboxError>,
        ) {
            self.resolve_message_params
                .lock()
                .unwrap()
                .push((args, route_result));
        }
    }

    impl RouteQueryResponseResolverMock {
        fn resolve_message_params(
            mut self,
            param: &Arc<
                Mutex<
                    Vec<(
                        TransmitToHopperArgs,
                        Result<Option<RouteQueryResponse>, MailboxError>,
                    )>,
                >,
            >,
        ) -> Self {
            self.resolve_message_params = param.clone();
            self
        }
    }

    struct StreamInfoBuilder {
        product: StreamInfo,
    }

    impl StreamInfoBuilder {
        pub fn new() -> Self {
            Self {
                product: StreamInfo {
                    tunneled_host_opt: None,
                    dns_failure_retry_opt: None,
                    route_opt: None,
                    protocol_opt: None,
                    browser_proxy_sequence_offset: false,
                    response_sequence_replay_window: ResponseSequenceReplayWindow::default(),
                    request_started_at_opt: None,
                    time_to_live_opt: None,
                    route_success_metadata_reported: false,
                },
            }
        }

        pub fn tunneled_host(mut self, host: &str) -> Self {
            self.product.tunneled_host_opt = Some(host.to_string());
            self
        }

        pub fn dns_failure_retry(mut self, retry: DNSFailureRetry) -> Self {
            self.product.dns_failure_retry_opt = Some(retry);
            self
        }

        pub fn route(mut self, route: RouteQueryResponse) -> Self {
            self.product.route_opt = Some(route);
            self
        }

        pub fn protocol(mut self, protocol: ProxyProtocol) -> Self {
            self.product.protocol_opt = Some(protocol);
            self
        }

        pub fn time_to_live(mut self, ttl: SystemTime) -> Self {
            self.product.time_to_live_opt = Some(ttl);
            self
        }

        pub fn request_started_at(mut self, started_at: SystemTime) -> Self {
            self.product.request_started_at_opt = Some(started_at);
            self
        }

        pub fn build(self) -> StreamInfo {
            self.product
        }
    }

    #[test]
    fn constants_have_correct_values() {
        assert_eq!(CRASH_KEY, "PROXYSERVER");
        assert_eq!(STREAM_KEY_PURGE_DELAY, Duration::from_secs(30));
        assert_eq!(DNS_FAILURE_RETRIES, 3);
        assert_eq!(ROUTE_ACTIVITY_HEARTBEAT_INTERVAL, Duration::from_secs(30));
    }

    #[test]
    fn route_activity_heartbeat_is_runtime_global_and_rate_limited() {
        let started_at = Instant::now();
        let mut subject = RouteActivityHeartbeat::default();

        assert!(subject.should_emit(started_at));
        assert!(!subject.should_emit(
            started_at + ROUTE_ACTIVITY_HEARTBEAT_INTERVAL - Duration::from_millis(1)
        ));
        assert!(subject.should_emit(started_at + ROUTE_ACTIVITY_HEARTBEAT_INTERVAL));
        assert!(!subject.should_emit(started_at + ROUTE_ACTIVITY_HEARTBEAT_INTERVAL));
    }

    #[test]
    fn response_sequence_replay_window_is_bounded_and_advances_contiguously() {
        let mut subject = ResponseSequenceReplayWindow::default();
        for sequence in 1..MAX_SEQUENCE_REORDER_WINDOW {
            assert_eq!(subject.admit(sequence), Ok(()));
        }
        assert_eq!(
            subject.seen_out_of_order.len(),
            (MAX_SEQUENCE_REORDER_WINDOW - 1) as usize
        );
        assert_eq!(
            subject.admit(MAX_SEQUENCE_REORDER_WINDOW),
            Err("the response sequence exceeds the bounded reorder window")
        );
        assert_eq!(
            subject.admit(u64::MAX),
            Err("the response sequence exceeds the bounded reorder window")
        );

        assert_eq!(subject.admit(0), Ok(()));

        assert_eq!(subject.next_expected_sequence, MAX_SEQUENCE_REORDER_WINDOW);
        assert!(subject.seen_out_of_order.is_empty());
        assert_eq!(
            subject.admit(1),
            Err("the response sequence is duplicate or stale")
        );
    }

    #[test]
    fn active_receipt_session_requires_capable_route_without_changing_ordinary_route() {
        let host = Host::new("example.com", HTTP_PORT);

        let ordinary = IBCDHelperReal::make_route_query(host.clone(), 123, None);
        let provider_cryptde = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let authorization = make_persisted_receipt_acknowledgement(&provider_cryptde).authorization;
        let receipt_request =
            crate::sub_lib::service_receipt::ReceiptSessionRequest::new(authorization, [0x49; 32])
                .unwrap();
        let receipt_capable =
            IBCDHelperReal::make_route_query(host, 123, Some(receipt_request.clone()));

        assert_eq!(ordinary.minimum_protocol_capabilities, 0);
        assert_ne!(receipt_capable.minimum_protocol_capabilities, 0);
        assert_eq!(
            receipt_capable.minimum_protocol_capabilities,
            crate::sub_lib::service_receipt::SERVICE_RECEIPT_SETTLEMENT_V1_CAPABILITY
                | crate::sub_lib::service_receipt::ROUTING_RECEIPT_V1_CAPABILITY
        );
        assert_eq!(
            receipt_capable.routing_receipt_request_opt,
            Some(receipt_request)
        );
        let provider = PublicKey::new(b"receipt provider");
        let delivery =
            RouteQueryMessage::service_receipt_delivery_route_request(provider.clone(), 456);
        assert_eq!(delivery.target_key_opt, Some(provider));
        assert_eq!(delivery.target_component, Component::Hopper);
        assert_eq!(delivery.return_component_opt, Some(Component::ProxyServer));
        assert_eq!(
            delivery.minimum_protocol_capabilities,
            crate::sub_lib::service_receipt::SERVICE_RECEIPT_SETTLEMENT_V1_CAPABILITY
        );
    }

    #[test]
    fn active_receipt_session_is_attached_to_real_client_payload_and_gates_its_route() {
        let wallet = make_paying_wallet(b"attached receipt-session wallet");
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.subs = Some(make_proxy_server_out_subs());
        subject.enable_receipt_sessions(ReceiptSessionConfig {
            chain: TEST_DEFAULT_CHAIN,
            chain_id: TEST_DEFAULT_CHAIN.rec().num_chain_id,
            settlement_contract: TEST_DEFAULT_CHAIN.rec().contract,
            payer_wallet_address: wallet.address(),
        });
        let now_unix_s = ProxyServer::unix_time_now().unwrap();
        let proposal = subject
            .receipt_session_manager_opt
            .as_mut()
            .unwrap()
            .propose("1000000", 600, now_unix_s)
            .unwrap();
        let signature = wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        let signature_hex = format!(
            "0x{}{}{:02x}",
            signature.r.to_hex::<String>(),
            signature.s.to_hex::<String>(),
            signature.v
        );
        subject
            .receipt_session_manager_opt
            .as_mut()
            .unwrap()
            .activate(&proposal.proposal_id, &signature_hex, now_unix_s)
            .unwrap();
        let stream_key = StreamKey::make_meaningful_stream_key("attached receipt stream");
        let mut factory_payload = make_request_payload(32, CRYPTDE_PAIR.alias.as_ref());
        factory_payload.stream_key = stream_key;
        subject.client_request_payload_factory =
            Box::new(ClientRequestPayloadFactoryMock::new().make_result(Some(factory_payload)));
        let inbound = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: false,
            is_clandestine: false,
            data: b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n".to_vec(),
        };

        let payload = subject.make_payload(inbound, &stream_key).unwrap();

        let request = payload.receipt_session_request_opt.as_ref().unwrap();
        assert_eq!(request.authorization.policy, proposal.policy);
        request
            .verify(
                TEST_DEFAULT_CHAIN.rec().num_chain_id,
                TEST_DEFAULT_CHAIN.rec().contract,
                now_unix_s,
            )
            .unwrap();
        let args = TransmitToHopperArgs::new(
            &mut subject,
            payload,
            SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            SystemTime::now(),
            false,
        );
        assert!(args.require_service_receipt_capability);
    }

    #[test]
    fn dns_retry_rotates_the_receipt_route_epoch_without_resetting_the_wallet_budget() {
        let system = System::new(
            "dns_retry_rotates_the_receipt_route_epoch_without_resetting_the_wallet_budget",
        );
        let wallet = make_paying_wallet(b"receipt retry wallet");
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.subs = Some(make_proxy_server_out_subs());
        subject.enable_receipt_sessions(ReceiptSessionConfig {
            chain: TEST_DEFAULT_CHAIN,
            chain_id: TEST_DEFAULT_CHAIN.rec().num_chain_id,
            settlement_contract: TEST_DEFAULT_CHAIN.rec().contract,
            payer_wallet_address: wallet.address(),
        });
        let now_unix_s = ProxyServer::unix_time_now().unwrap();
        let proposal = subject
            .receipt_session_manager_opt
            .as_mut()
            .unwrap()
            .propose("1000000", 600, now_unix_s)
            .unwrap();
        let signature = wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        subject
            .receipt_session_manager_opt
            .as_mut()
            .unwrap()
            .activate(
                &proposal.proposal_id,
                &format!(
                    "0x{}{}{:02x}",
                    signature.r.to_hex::<String>(),
                    signature.s.to_hex::<String>(),
                    signature.v
                ),
                now_unix_s,
            )
            .unwrap();
        let stream_key = StreamKey::make_meaningful_stream_key("receipt retry stream");
        let old_request = subject
            .receipt_session_manager_opt
            .as_mut()
            .unwrap()
            .request_for_stream(stream_key, now_unix_s)
            .unwrap()
            .unwrap();
        let mut unsuccessful_request = make_request_payload(32, CRYPTDE_PAIR.alias.as_ref());
        unsuccessful_request.stream_key = stream_key;
        unsuccessful_request.receipt_session_request_opt = Some(old_request.clone());
        let transmitted_payloads = Arc::new(Mutex::new(vec![]));
        subject.inbound_client_data_helper_opt = Some(Box::new(
            IBCDHelperMock::default().request_route_and_transmit_params(&transmitted_payloads),
        ));

        let mut retry = DNSFailureRetry {
            unsuccessful_request,
            retries_left: 3,
            active_attempt_id: 0,
        };
        subject
            .retry_dns_resolution(&mut retry, SocketAddr::from_str("1.2.3.4:5678").unwrap())
            .unwrap();

        let recordings = transmitted_payloads.lock().unwrap();
        let new_request = recordings[0].receipt_session_request_opt.as_ref().unwrap();
        assert_ne!(new_request.route_epoch, old_request.route_epoch);
        assert_eq!(new_request.authorization, old_request.authorization);
        assert_eq!(
            subject
                .receipt_session_manager_opt
                .as_mut()
                .unwrap()
                .status(now_unix_s)
                .spent_charge_wei_opt,
            Some(0)
        );
        System::current().stop();
        system.run();
    }

    #[test]
    fn valid_provider_offer_is_acknowledged_over_a_capability_gated_route() {
        use crate::sub_lib::service_receipt::{
            make_accounting_commitment, ServiceKind, ServiceReceipt, ServiceReceiptPayload_0v1,
        };

        let system =
            System::new("valid_provider_offer_is_acknowledged_over_a_capability_gated_route");
        let wallet = make_paying_wallet(b"receipt session wallet");
        let provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let consumer_cryptde_pair = CryptDEPair::new(
            Box::new(CryptDEReal::new(TEST_DEFAULT_CHAIN)),
            Box::new(CryptDEReal::new(TEST_DEFAULT_CHAIN)),
        );
        let mut subject =
            ProxyServer::new(consumer_cryptde_pair.clone(), true, Some(0), false, false);
        let outbox_state = Arc::new(Mutex::new(ReceiptAcknowledgementOutboxState::default()));
        subject.enable_receipt_acknowledgement_outbox(Box::new(
            ReceiptAcknowledgementOutboxDaoMock {
                state: Arc::clone(&outbox_state),
            },
        ));
        subject.enable_receipt_sessions(ReceiptSessionConfig {
            chain: TEST_DEFAULT_CHAIN,
            chain_id: TEST_DEFAULT_CHAIN.rec().num_chain_id,
            settlement_contract: TEST_DEFAULT_CHAIN.rec().contract,
            payer_wallet_address: wallet.address(),
        });
        let now_unix_s = ProxyServer::unix_time_now().unwrap();
        let proposal = subject
            .receipt_session_manager_opt
            .as_mut()
            .unwrap()
            .propose("1000", 600, now_unix_s)
            .unwrap();
        let signature = wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        let signature_hex = format!(
            "0x{}{}{:02x}",
            signature.r.to_hex::<String>(),
            signature.s.to_hex::<String>(),
            signature.v
        );
        subject
            .receipt_session_manager_opt
            .as_mut()
            .unwrap()
            .activate(&proposal.proposal_id, &signature_hex, now_unix_s)
            .unwrap();
        let stream_key = StreamKey::make_meaningful_stream_key("receipt offer actor stream");
        let route_epoch = subject
            .receipt_session_manager_opt
            .as_mut()
            .unwrap()
            .request_for_stream(stream_key, now_unix_s)
            .unwrap()
            .unwrap()
            .route_epoch;
        subject
            .receipt_session_manager_opt
            .as_mut()
            .unwrap()
            .record_exit_response(&stream_key, 1, 100, now_unix_s)
            .unwrap();
        let offer = ServiceReceiptOfferPayload_0v1 {
            signed_receipt: ServiceReceipt::new(
                route_epoch,
                1,
                ServiceKind::Exit,
                provider.public_key().clone(),
                make_accounting_commitment(&route_epoch, &proposal.policy.payer_session_public_key),
                100,
                5,
                2,
            )
            .sign(&provider)
            .unwrap(),
        };
        subject.handle_service_receipt_offer(offer.clone());
        assert_eq!(
            subject.pending_receipt_offers.get(&route_epoch),
            Some(&offer)
        );
        subject
            .receipt_session_manager_opt
            .as_mut()
            .unwrap()
            .bind_exit_quote(&stream_key, provider.public_key().clone(), 5, 2, now_unix_s)
            .unwrap();
        let routing_wallet = make_wallet("receipt ack router");
        let routing_rate_pack = rate_pack(300);
        let route_response = RouteQueryResponse {
            route: make_meaningless_route(&consumer_cryptde_pair),
            expected_services: ExpectedServices::RoundTrip(
                vec![ExpectedService::Routing(
                    PublicKey::new(b"receipt ack router key"),
                    routing_wallet.clone(),
                    routing_rate_pack,
                )],
                vec![],
            ),
            host: Host::new("", 0),
        };
        let (route_recorder, _, route_recording) = make_recorder();
        let route_addr = route_recorder
            .route_query_response(Some(route_response))
            .start();
        let hopper_recorder = Recorder::new()
            .system_stop_conditions(match_lazily_every_type_id!(IncipientCoresPackage));
        let hopper_recording = hopper_recorder.get_recording();
        let hopper_addr = hopper_recorder.start();
        subject.subs = Some(ProxyServerOutSubs {
            dispatcher: recipient!(hopper_addr, TransmitDataMsg),
            hopper: recipient!(hopper_addr, IncipientCoresPackage),
            accountant: recipient!(hopper_addr, ReportServicesConsumedMessage),
            routing_accountant: recipient!(hopper_addr, ReportRoutingServicesConsumedMessage),
            route_source: recipient!(route_addr, RouteQueryMessage),
            route_use_failed: recipient!(hopper_addr, RouteUseFailedMessage),
            route_use_succeeded: recipient!(hopper_addr, RouteUseSucceededMessage),
            update_node_record_metadata: recipient!(hopper_addr, UpdateNodeRecordMetadataMessage),
            stream_shutdown_sub: recipient!(hopper_addr, StreamShutdownMsg),
            route_result_sub: recipient!(hopper_addr, AddRouteResultMessage),
            schedule_stream_key_purge: recipient!(hopper_addr, MessageScheduler<StreamKeyPurge>),
            retry_receipt_acknowledgements: recipient!(hopper_addr, RetryReceiptAcknowledgements),
            record_exit_request_for_receipt: recipient!(hopper_addr, RecordExitRequestForReceipt),
            ui_gateway: recipient!(hopper_addr, NodeToUiMessage),
        });

        subject
            .start()
            .try_send(AssertionsMessage {
                assertions: Box::new(move |proxy_server: &mut ProxyServer| {
                    proxy_server.replay_pending_receipt_offer(route_epoch)
                }),
            })
            .unwrap();
        system.run();

        {
            let recording = route_recording.lock().unwrap();
            let query = recording.get_record::<RouteQueryMessage>(0);
            assert_eq!(query.target_key_opt, Some(provider.public_key().clone()));
            assert_eq!(
                query.minimum_protocol_capabilities,
                crate::sub_lib::service_receipt::SERVICE_RECEIPT_SETTLEMENT_V1_CAPABILITY
            );
        }
        let package = hopper_recording
            .lock()
            .unwrap()
            .get_record::<IncipientCoresPackage>(1)
            .clone();
        let routing_report = hopper_recording
            .lock()
            .unwrap()
            .get_record::<ReportRoutingServicesConsumedMessage>(0)
            .clone();
        assert_eq!(routing_report.payload_size, package.payload.len());
        assert_eq!(
            routing_report.routing,
            vec![RoutingServiceConsumed {
                earning_wallet: routing_wallet,
                service_rate: routing_rate_pack.routing_service_rate,
                byte_rate: routing_rate_pack.routing_byte_rate,
            }]
        );
        let decoded = decodex::<MessageType>(&provider, &package.payload).unwrap();
        let versioned = match decoded {
            MessageType::ServiceReceipt(versioned) => versioned,
            other => panic!("Unexpected receipt acknowledgement: {:?}", other),
        };
        let payload = ServiceReceiptPayload_0v1::try_from(versioned).unwrap();
        payload
            .authorization
            .verify_for_receipt(
                &payload.acknowledged_receipt,
                &provider,
                TEST_DEFAULT_CHAIN.rec().num_chain_id,
                TEST_DEFAULT_CHAIN.rec().contract,
                now_unix_s,
            )
            .unwrap();
        let outbox = outbox_state.lock().unwrap();
        assert_eq!(outbox.enqueue_count, 1);
        assert_eq!(outbox.delete_count, 1);
        assert!(outbox.pending.is_empty());
    }

    #[test]
    fn persisted_receipt_acknowledgement_is_replayed_and_cleared_after_hopper_accepts_it() {
        let system = System::new(
            "persisted_receipt_acknowledgement_is_replayed_and_cleared_after_hopper_accepts_it",
        );
        let provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let persisted_payload = make_persisted_receipt_acknowledgement(&provider);
        let consumer_cryptde_pair = CryptDEPair::new(
            Box::new(CryptDEReal::new(TEST_DEFAULT_CHAIN)),
            Box::new(CryptDEReal::new(TEST_DEFAULT_CHAIN)),
        );
        let outbox_state = Arc::new(Mutex::new(ReceiptAcknowledgementOutboxState {
            pending: vec![persisted_payload.clone()],
            ..Default::default()
        }));
        let mut subject =
            ProxyServer::new(consumer_cryptde_pair.clone(), true, Some(0), false, false);
        subject.enable_receipt_acknowledgement_outbox(Box::new(
            ReceiptAcknowledgementOutboxDaoMock {
                state: Arc::clone(&outbox_state),
            },
        ));
        let route_response = RouteQueryResponse {
            route: make_meaningless_route(&consumer_cryptde_pair),
            expected_services: ExpectedServices::RoundTrip(vec![], vec![]),
            host: Host::new("", 0),
        };
        let (route_recorder, _, route_recording) = make_recorder();
        let route_addr = route_recorder
            .route_query_response(Some(route_response))
            .start();
        let hopper_recorder = Recorder::new()
            .system_stop_conditions(match_lazily_every_type_id!(IncipientCoresPackage));
        let hopper_recording = hopper_recorder.get_recording();
        let hopper_addr = hopper_recorder.start();
        subject.subs = Some(ProxyServerOutSubs {
            dispatcher: recipient!(hopper_addr, TransmitDataMsg),
            hopper: recipient!(hopper_addr, IncipientCoresPackage),
            accountant: recipient!(hopper_addr, ReportServicesConsumedMessage),
            routing_accountant: recipient!(hopper_addr, ReportRoutingServicesConsumedMessage),
            route_source: recipient!(route_addr, RouteQueryMessage),
            route_use_failed: recipient!(hopper_addr, RouteUseFailedMessage),
            route_use_succeeded: recipient!(hopper_addr, RouteUseSucceededMessage),
            update_node_record_metadata: recipient!(hopper_addr, UpdateNodeRecordMetadataMessage),
            stream_shutdown_sub: recipient!(hopper_addr, StreamShutdownMsg),
            route_result_sub: recipient!(hopper_addr, AddRouteResultMessage),
            schedule_stream_key_purge: recipient!(hopper_addr, MessageScheduler<StreamKeyPurge>),
            retry_receipt_acknowledgements: recipient!(hopper_addr, RetryReceiptAcknowledgements),
            record_exit_request_for_receipt: recipient!(hopper_addr, RecordExitRequestForReceipt),
            ui_gateway: recipient!(hopper_addr, NodeToUiMessage),
        });

        subject
            .start()
            .try_send(AssertionsMessage {
                assertions: Box::new(|proxy_server: &mut ProxyServer| {
                    proxy_server.replay_persisted_receipt_acknowledgements()
                }),
            })
            .unwrap();
        system.run();

        {
            let recording = route_recording.lock().unwrap();
            let query = recording.get_record::<RouteQueryMessage>(0);
            assert_eq!(query.target_key_opt, Some(provider.public_key().clone()));
        }
        let package = hopper_recording
            .lock()
            .unwrap()
            .get_record::<IncipientCoresPackage>(0)
            .clone();
        let decoded = decodex::<MessageType>(&provider, &package.payload).unwrap();
        let versioned = match decoded {
            MessageType::ServiceReceipt(versioned) => versioned,
            other => panic!("Unexpected recovered acknowledgement: {:?}", other),
        };
        assert_eq!(
            ServiceReceiptPayload_0v1::try_from(versioned).unwrap(),
            persisted_payload
        );
        let outbox = outbox_state.lock().unwrap();
        assert_eq!(outbox.enqueue_count, 0);
        assert_eq!(outbox.delete_count, 1);
        assert!(outbox.pending.is_empty());
    }

    #[test]
    fn failed_acknowledgement_route_keeps_the_outbox_record_and_schedules_a_retry() {
        let system = System::new(
            "failed_acknowledgement_route_keeps_the_outbox_record_and_schedules_a_retry",
        );
        let provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let persisted_payload = make_persisted_receipt_acknowledgement(&provider);
        let outbox_state = Arc::new(Mutex::new(ReceiptAcknowledgementOutboxState {
            pending: vec![persisted_payload],
            ..Default::default()
        }));
        let mut subject = ProxyServer::new(
            CryptDEPair::new(
                Box::new(CryptDEReal::new(TEST_DEFAULT_CHAIN)),
                Box::new(CryptDEReal::new(TEST_DEFAULT_CHAIN)),
            ),
            true,
            Some(0),
            false,
            false,
        );
        subject.enable_receipt_acknowledgement_outbox(Box::new(
            ReceiptAcknowledgementOutboxDaoMock {
                state: Arc::clone(&outbox_state),
            },
        ));
        let (route_recorder, _, _) = make_recorder();
        let route_addr = route_recorder.route_query_response(None).start();
        let retry_recorder = Recorder::new()
            .system_stop_conditions(match_lazily_every_type_id!(RetryReceiptAcknowledgements));
        let retry_recording = retry_recorder.get_recording();
        let retry_addr = retry_recorder.start();
        subject.subs = Some(ProxyServerOutSubs {
            dispatcher: recipient!(retry_addr, TransmitDataMsg),
            hopper: recipient!(retry_addr, IncipientCoresPackage),
            accountant: recipient!(retry_addr, ReportServicesConsumedMessage),
            routing_accountant: recipient!(retry_addr, ReportRoutingServicesConsumedMessage),
            route_source: recipient!(route_addr, RouteQueryMessage),
            route_use_failed: recipient!(retry_addr, RouteUseFailedMessage),
            route_use_succeeded: recipient!(retry_addr, RouteUseSucceededMessage),
            update_node_record_metadata: recipient!(retry_addr, UpdateNodeRecordMetadataMessage),
            stream_shutdown_sub: recipient!(retry_addr, StreamShutdownMsg),
            route_result_sub: recipient!(retry_addr, AddRouteResultMessage),
            schedule_stream_key_purge: recipient!(retry_addr, MessageScheduler<StreamKeyPurge>),
            retry_receipt_acknowledgements: recipient!(retry_addr, RetryReceiptAcknowledgements),
            record_exit_request_for_receipt: recipient!(retry_addr, RecordExitRequestForReceipt),
            ui_gateway: recipient!(retry_addr, NodeToUiMessage),
        });

        subject
            .start()
            .try_send(AssertionsMessage {
                assertions: Box::new(|proxy_server: &mut ProxyServer| {
                    proxy_server.replay_persisted_receipt_acknowledgements()
                }),
            })
            .unwrap();
        system.run();

        let recording = retry_recording.lock().unwrap();
        assert_eq!(
            recording.get_record::<RetryReceiptAcknowledgements>(0),
            &RetryReceiptAcknowledgements {
                schedule_after_delay: true
            }
        );
        let outbox = outbox_state.lock().unwrap();
        assert_eq!(outbox.delete_count, 0);
        assert_eq!(outbox.pending.len(), 1);
    }

    const STANDARD_CONSUMING_WALLET_BALANCE: i64 = 0;

    fn make_proxy_server_out_subs() -> ProxyServerOutSubs {
        let recorder = Recorder::new();
        let addr = recorder.start();
        ProxyServerOutSubs {
            dispatcher: recipient!(addr, TransmitDataMsg),
            hopper: recipient!(addr, IncipientCoresPackage),
            accountant: recipient!(addr, ReportServicesConsumedMessage),
            routing_accountant: recipient!(addr, ReportRoutingServicesConsumedMessage),
            route_source: recipient!(addr, RouteQueryMessage),
            route_use_failed: recipient!(addr, RouteUseFailedMessage),
            route_use_succeeded: recipient!(addr, RouteUseSucceededMessage),
            update_node_record_metadata: recipient!(addr, UpdateNodeRecordMetadataMessage),
            stream_shutdown_sub: recipient!(addr, StreamShutdownMsg),
            route_result_sub: recipient!(addr, AddRouteResultMessage),
            schedule_stream_key_purge: recipient!(addr, MessageScheduler<StreamKeyPurge>),
            retry_receipt_acknowledgements: recipient!(addr, RetryReceiptAcknowledgements),
            record_exit_request_for_receipt: recipient!(addr, RecordExitRequestForReceipt),
            ui_gateway: recipient!(addr, NodeToUiMessage),
        }
    }

    struct StreamKeyFactoryMock {
        make_parameters: Arc<Mutex<Vec<(PublicKey, SocketAddr)>>>,
        make_results: RefCell<Vec<StreamKey>>,
    }

    impl StreamKeyFactory for StreamKeyFactoryMock {
        fn make(&self, public_key: &PublicKey, client_addr: SocketAddr) -> StreamKey {
            self.make_parameters
                .lock()
                .unwrap()
                .push((public_key.clone(), client_addr));
            self.make_results.borrow_mut().remove(0)
        }
    }

    impl StreamKeyFactoryMock {
        fn new() -> StreamKeyFactoryMock {
            StreamKeyFactoryMock {
                make_parameters: Arc::new(Mutex::new(vec![])),
                make_results: RefCell::new(vec![]),
            }
        }

        fn make_parameters(
            mut self,
            params: &Arc<Mutex<Vec<(PublicKey, SocketAddr)>>>,
        ) -> StreamKeyFactoryMock {
            self.make_parameters = params.clone();
            self
        }

        fn make_result(self, stream_key: StreamKey) -> StreamKeyFactoryMock {
            self.make_results.borrow_mut().push(stream_key);
            self
        }
    }

    fn return_route(cryptde: &dyn CryptDE) -> Route {
        Route {
            hops: vec![make_cover_hop(cryptde)],
        }
    }

    fn make_cover_hop(cryptde: &dyn CryptDE) -> CryptData {
        encodex(
            cryptde,
            &cryptde.public_key(),
            &LiveHop {
                public_key: cryptde.public_key().clone(),
                payer: None,
                routing_receipt_request_opt: None,
                component: Component::ProxyServer,
            },
        )
        .unwrap()
    }

    #[derive(Default)]
    struct IBCDHelperMock {
        handle_normal_client_data_params: Arc<Mutex<Vec<InboundClientData>>>,
        handle_normal_client_data_results: RefCell<Vec<Result<(), String>>>,
        request_route_and_transmit_params: Arc<Mutex<Vec<ClientRequestPayload_0v1>>>,
    }

    impl IBCDHelper for IBCDHelperMock {
        fn handle_normal_client_data(
            &self,
            _proxy_s: &mut ProxyServer,
            msg: InboundClientData,
        ) -> Result<(), String> {
            self.handle_normal_client_data_params
                .lock()
                .unwrap()
                .push(msg);
            self.handle_normal_client_data_results
                .borrow_mut()
                .remove(0)
        }

        fn request_route_and_transmit(
            &self,
            args: TransmitToHopperArgs,
            _route_source: Recipient<RouteQueryMessage>,
            _proxy_server_sub: Recipient<AddRouteResultMessage>,
        ) {
            self.request_route_and_transmit_params
                .lock()
                .unwrap()
                .push(args.payload);
        }
    }

    impl IBCDHelperMock {
        fn handle_normal_client_data_params(
            mut self,
            params: &Arc<Mutex<Vec<InboundClientData>>>,
        ) -> Self {
            self.handle_normal_client_data_params = params.clone();
            self
        }

        fn handle_normal_client_data_result(self, result: Result<(), String>) -> Self {
            self.handle_normal_client_data_results
                .borrow_mut()
                .push(result);
            self
        }

        fn request_route_and_transmit_params(
            mut self,
            params: &Arc<Mutex<Vec<ClientRequestPayload_0v1>>>,
        ) -> Self {
            self.request_route_and_transmit_params = params.clone();
            self
        }
    }

    #[test]
    fn get_expected_services_produces_nothing_if_nothing_exists() {
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();

        let result = subject.get_expected_return_services(&stream_key);

        assert!(
            result.is_none(),
            "Expected no expected services, but got: {:?}",
            result
        );
    }

    #[test]
    fn get_expected_services_produces_rri_when_it_exists() {
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let exit_public_key = PublicKey::new(&b"exit key"[..]);
        let stream_key = StreamKey::make_meaningless_stream_key();
        let back_services = vec![ExpectedService::Exit(
            exit_public_key,
            make_wallet("booga"),
            rate_pack(1000),
        )];
        let expected_services = ExpectedServices::RoundTrip(vec![], back_services.clone());
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: make_meaningless_route(&CRYPTDE_PAIR),
                    expected_services: expected_services.clone(),
                    host: Host::new("booga.com", TLS_PORT),
                })
                .protocol(ProxyProtocol::TLS)
                .build(),
        );

        let result = subject.get_expected_return_services(&stream_key).unwrap();

        assert_eq!(result, back_services);
    }

    #[test]
    fn get_expected_services_ignores_response_if_stream_info_has_no_route() {
        init_test_logging();
        let test_name = "get_expected_services_ignores_response_if_stream_info_has_no_route";
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        let stream_key = StreamKey::from_bytes(b"constant for missing route");
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                // no route_opt: problem
                .protocol(ProxyProtocol::TLS)
                .build(),
        );

        let result = subject.get_expected_return_services(&stream_key);

        assert_eq!(result, None);
        assert!(subject.stream_info.contains_key(&stream_key));
        TestLogHandler::new().exists_log_containing(&format!(
            "ERROR: {test_name}: Can't pay for return services consumed: received response for stream key {stream_key} before a return route was stored. Ignoring"
        ));
    }

    #[test]
    fn get_expected_services_ignores_response_if_route_is_one_way() {
        init_test_logging();
        let test_name = "get_expected_services_ignores_response_if_route_is_one_way";
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        let stream_key = StreamKey::from_bytes(b"constant for one-way route");
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: make_meaningless_route(&CRYPTDE_PAIR),
                    expected_services: ExpectedServices::OneWay(vec![ExpectedService::Nothing]),
                    host: Host::new("booga.com", TLS_PORT),
                })
                .protocol(ProxyProtocol::TLS)
                .build(),
        );

        let result = subject.get_expected_return_services(&stream_key);

        assert_eq!(result, None);
        assert!(subject.stream_info.contains_key(&stream_key));
        TestLogHandler::new().exists_log_containing(&format!(
            "ERROR: {test_name}: Can't pay for return services consumed: received response for stream key {stream_key} whose route has no return services. Ignoring"
        ));
    }

    #[test]
    fn client_response_without_return_route_is_side_effect_free_and_preserves_stream_state() {
        init_test_logging();
        let test_name =
            "client_response_without_return_route_is_side_effect_free_and_preserves_stream_state";
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        let stream_key = StreamKey::make_meaningful_stream_key(test_name);
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr);
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let response = ClientResponsePayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket {
                data: b"untrusted response".to_vec(),
                sequence_number: 1,
                last_data: false,
            },
        };
        let expired_cores_package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            Some(make_wallet("irrelevant")),
            return_route(cryptde),
            response,
            777,
        );

        subject.handle_client_response_payload(expired_cores_package);

        assert!(subject.stream_info.contains_key(&stream_key));
        assert_eq!(
            subject.keys_and_addrs.a_to_b(&stream_key),
            Some(socket_addr)
        );
        TestLogHandler::new().exists_log_containing(&format!(
            "ERROR: {test_name}: Can't pay for return services consumed: received response for stream key {stream_key} before a return route was stored. Ignoring"
        ));
    }

    #[test]
    fn proxy_server_receives_http_request_with_new_stream_key_from_dispatcher_then_sends_cores_package_to_hopper(
    ) {
        init_test_logging();
        let test_name = "proxy_server_receives_http_request_with_new_stream_key_from_dispatcher_then_sends_cores_package_to_hopper";
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let alias_cryptde = CRYPTDE_PAIR.alias.as_ref();
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let (hopper_mock, hopper_awaiter, hopper_log_arc) = make_recorder();
        let (neighborhood_mock, _, neighborhood_recording_arc) = make_recorder();
        let destination_key = PublicKey::from(&b"our destination"[..]);
        let neighborhood_mock = neighborhood_mock.route_query_response(Some(RouteQueryResponse {
            route: Route { hops: vec![] },
            expected_services: ExpectedServices::RoundTrip(
                vec![make_exit_service_from_key(destination_key.clone())],
                vec![],
            ),
            host: Host::new("booga.com", HTTP_PORT),
        }));
        let (proxy_server_mock, _, proxy_server_recording_arc) = make_recorder();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr,
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let expected_http_request = PlainData::new(http_request);
        let route = Route { hops: vec![] };
        let expected_payload = ClientRequestPayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket {
                data: expected_http_request.into(),
                sequence_number: 0,
                last_data: true,
            },
            target_hostname: String::from("nowhere.com"),
            target_port: HTTP_PORT,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: alias_cryptde.public_key().clone(),
            dns_attempt_id_opt: Some(0),
            receipt_session_request_opt: None,
        };
        let expected_pkg = IncipientCoresPackage::new(
            main_cryptde,
            route.clone(),
            expected_payload.into(),
            &destination_key,
        )
        .unwrap();
        let make_parameters_arc = Arc::new(Mutex::new(vec![]));
        let make_parameters_arc_a = make_parameters_arc.clone();
        thread::spawn(move || {
            let stream_key_factory = StreamKeyFactoryMock::new()
                .make_parameters(&make_parameters_arc)
                .make_result(stream_key.clone());
            let system = System::new(test_name);
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.logger = Logger::new(test_name);
            subject.stream_key_factory = Box::new(stream_key_factory);
            let subject_addr: Addr<ProxyServer> = subject.start();
            let mut peer_actors = peer_actors_builder()
                .hopper(hopper_mock)
                .neighborhood(neighborhood_mock)
                .proxy_server(proxy_server_mock)
                .build();
            peer_actors.proxy_server = ProxyServer::make_subs_from(&subject_addr);
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();

            subject_addr
                .try_send(AssertionsMessage {
                    assertions: Box::new(move |proxy_server: &mut ProxyServer| {
                        assert!(proxy_server.stream_info.contains_key(&stream_key));
                    }),
                })
                .unwrap();
            system.run();
        });

        hopper_awaiter.await_message_count(1);
        let recording = hopper_log_arc.lock().unwrap();
        let record = recording.get_record::<IncipientCoresPackage>(0);
        assert_eq!(record, &expected_pkg);
        let mut make_parameters = make_parameters_arc_a.lock().unwrap();
        assert_eq!(
            make_parameters.remove(0),
            (main_cryptde.public_key().clone(), socket_addr)
        );
        let recording = neighborhood_recording_arc.lock().unwrap();
        let record = recording.get_record::<RouteQueryMessage>(0);
        assert_eq!(
            record,
            &RouteQueryMessage::data_indefinite_route_request(
                Host::new("nowhere.com", HTTP_PORT),
                47
            )
        );
        let recording = proxy_server_recording_arc.lock().unwrap();
        assert_eq!(recording.len(), 0);

        TestLogHandler::new().exists_log_containing(
            &format!("DEBUG: {test_name}: Found a new route for DNS retry; destination and stream redacted; retries left: 3")
        );
    }

    #[test]
    fn proxy_server_receives_connect_responds_with_ok_and_stores_stream_key_and_hostname() {
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let alias_cryptde = CRYPTDE_PAIR.alias.as_ref();
        let http_request = b"CONNECT https://realdomain.nu:443 HTTP/1.1\r\nHost: https://bunkjunk.wrong:443\r\n\r\n";
        let (hopper_mock, hopper_awaiter, hopper_recording_arc) = make_recorder();
        let (neighborhood_mock, _, neighborhood_recording_arc) = make_recorder();
        let destination_key = PublicKey::from(&b"our destination"[..]);
        let neighborhood_mock = neighborhood_mock.route_query_response(Some(RouteQueryResponse {
            route: Route { hops: vec![] },
            expected_services: ExpectedServices::RoundTrip(
                vec![make_exit_service_from_key(destination_key.clone())],
                vec![],
            ),
            host: Host::new("booga.com", HTTP_PORT),
        }));
        let route = Route { hops: vec![] };
        let (dispatcher_mock, _, dispatcher_recording_arc) = make_recorder();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let request_data = http_request.to_vec();
        let tunneled_data = make_server_com_client_hello();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(8443),
            sequence_number_opt: Some(0),
            last_data: false,
            is_clandestine: false,
            data: request_data.clone(),
        };
        let tunnelled_msg = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr,
            reception_port_opt: Some(8443),
            sequence_number_opt: Some(0),
            last_data: false,
            is_clandestine: false,
            data: tunneled_data.clone(),
        };
        let expected_tdm = TransmitDataMsg {
            endpoint: Endpoint::Socket(socket_addr),
            last_data: false,
            sequence_number_opt: Some(0),
            data: b"HTTP/1.1 200 OK\r\n\r\n".to_vec(),
        };
        let expected_payload = ClientRequestPayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket {
                data: tunneled_data,
                sequence_number: 0,
                last_data: false,
            },
            target_hostname: String::from("realdomain.nu"),
            target_port: TLS_PORT,
            protocol: ProxyProtocol::TLS,
            originator_public_key: alias_cryptde.public_key().clone(),
            dns_attempt_id_opt: Some(0),
            receipt_session_request_opt: None,
        };
        let expected_pkg = IncipientCoresPackage::new(
            main_cryptde,
            route.clone(),
            expected_payload.into(),
            &destination_key,
        )
        .unwrap();
        let make_parameters_arc = Arc::new(Mutex::new(vec![]));
        let make_parameters_arc_thread = make_parameters_arc.clone();

        thread::spawn(move || {
            let stream_key_factory = StreamKeyFactoryMock::new()
                .make_parameters(&make_parameters_arc_thread)
                .make_result(stream_key.clone());
            let system = System::new(
                "proxy_server_receives_connect_responds_with_ok_and_stores_stream_key_and_hostname",
            );
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.stream_key_factory = Box::new(stream_key_factory);
            let subject_addr: Addr<ProxyServer> = subject.start();
            let mut peer_actors = peer_actors_builder()
                .dispatcher(dispatcher_mock)
                .hopper(hopper_mock)
                .neighborhood(neighborhood_mock)
                .build();
            peer_actors.proxy_server.route_result_sub =
                recipient!(&subject_addr, AddRouteResultMessage);
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();
            subject_addr.try_send(tunnelled_msg).unwrap();
            subject_addr
                .try_send(AssertionsMessage {
                    assertions: Box::new(move |proxy_server: &mut ProxyServer| {
                        assert!(proxy_server.stream_info.contains_key(&stream_key));
                    }),
                })
                .unwrap();
            system.run();
        });

        hopper_awaiter.await_message_count(1);
        let dispatcher_recording = dispatcher_recording_arc.lock().unwrap();
        let dispatcher_record = dispatcher_recording.get_record::<TransmitDataMsg>(0);
        assert_eq!(dispatcher_record, &expected_tdm);
        let mut make_parameters = make_parameters_arc.lock().unwrap();
        assert_eq!(
            make_parameters.remove(0),
            (main_cryptde.public_key().clone(), socket_addr)
        );

        let hopper_recording = hopper_recording_arc.lock().unwrap();
        let hopper_record = hopper_recording.get_record::<IncipientCoresPackage>(0);
        assert_eq!(hopper_record, &expected_pkg);

        let neighborhood_recording = neighborhood_recording_arc.lock().unwrap();
        let neighborhood_record = neighborhood_recording.get_record::<RouteQueryMessage>(0);
        assert_eq!(
            neighborhood_record,
            &RouteQueryMessage::data_indefinite_route_request(
                Host::new("realdomain.nu", TLS_PORT),
                68
            )
        );
    }

    #[test]
    fn handle_client_response_payload_increments_sequence_number_when_browser_proxy_sequence_offset_is_true(
    ) {
        let system = System::new("handle_client_response_payload_increments_sequence_number_when_browser_proxy_sequence_offset_is_true");
        let (dispatcher_mock, _, dispatcher_log_arc) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: make_meaningless_route(&CRYPTDE_PAIR),
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![ExpectedService::Nothing],
                    ),
                    host: Host::new("booga.com", TLS_PORT),
                })
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let http_request = b"CONNECT https://realdomain.nu:443 HTTP/1.1\r\nHost: https://bunkjunk.wrong:443\r\n\r\n";
        let request_data = http_request.to_vec();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr,
            reception_port_opt: Some(TLS_PORT),
            last_data: false,
            is_clandestine: false,
            sequence_number_opt: Some(0),
            data: request_data,
        };

        let client_response_payload = ClientResponsePayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket {
                data: b"some data".to_vec(),
                sequence_number: 0,
                last_data: false,
            },
        };

        let expired_cores_package: ExpiredCoresPackage<ClientResponsePayload_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                client_response_payload.into(),
                0,
            );

        let peer_actors = peer_actors_builder().dispatcher(dispatcher_mock).build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        subject_addr.try_send(inbound_client_data).unwrap();

        subject_addr
            .try_send(expired_cores_package.clone())
            .unwrap();

        System::current().stop();
        system.run();

        let dispatcher_recording = dispatcher_log_arc.lock().unwrap();
        let record = dispatcher_recording.get_record::<TransmitDataMsg>(0);
        assert_eq!(record.sequence_number_opt.unwrap(), 0);
        let record = dispatcher_recording.get_record::<TransmitDataMsg>(1);
        assert_eq!(record.sequence_number_opt.unwrap(), 1);
    }

    #[test]
    fn connect_sequence_offset_is_isolated_to_its_stream() {
        let test_name = "connect_sequence_offset_is_isolated_to_its_stream";
        let system = System::new(test_name);
        let (dispatcher_mock, _, dispatcher_recording_arc) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let connect_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let http_addr = SocketAddr::from_str("2.3.4.5:6789").unwrap();
        let connect_stream_key = StreamKey::make_meaningful_stream_key("connect stream");
        let http_stream_key = StreamKey::make_meaningful_stream_key("http stream");
        subject
            .keys_and_addrs
            .insert(connect_stream_key, connect_addr);
        subject.keys_and_addrs.insert(http_stream_key, http_addr);
        subject
            .stream_info
            .insert(connect_stream_key, StreamInfoBuilder::new().build());
        subject.stream_info.insert(
            http_stream_key,
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: make_meaningless_route(&CRYPTDE_PAIR),
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![ExpectedService::Nothing],
                    ),
                    host: Host::new("example.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let peer_actors = peer_actors_builder().dispatcher(dispatcher_mock).build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        subject_addr
            .try_send(InboundClientData {
                timestamp: SystemTime::now(),
                client_addr: connect_addr,
                reception_port_opt: Some(TLS_PORT),
                sequence_number_opt: Some(0),
                last_data: false,
                is_clandestine: false,
                data: b"CONNECT https://example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n"
                    .to_vec(),
            })
            .unwrap();
        subject_addr
            .try_send(ExpiredCoresPackage::new(
                SocketAddr::from_str("3.4.5.6:7890").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                ClientResponsePayload_0v1 {
                    stream_key: http_stream_key,
                    sequenced_packet: SequencedPacket {
                        data: b"http response".to_vec(),
                        sequence_number: 0,
                        last_data: false,
                    },
                },
                0,
            ))
            .unwrap();

        System::current().stop();
        system.run();

        let recording = dispatcher_recording_arc.lock().unwrap();
        assert_eq!(recording.len(), 2);
        let connect_response = recording.get_record::<TransmitDataMsg>(0);
        assert_eq!(connect_response.endpoint, Endpoint::Socket(connect_addr));
        assert_eq!(connect_response.sequence_number_opt, Some(0));
        let http_response = recording.get_record::<TransmitDataMsg>(1);
        assert_eq!(http_response.endpoint, Endpoint::Socket(http_addr));
        assert_eq!(http_response.sequence_number_opt, Some(0));
    }

    #[test]
    fn overflowing_connect_sequence_is_discarded_before_side_effects_and_stream_recovers() {
        init_test_logging();
        let test_name =
            "overflowing_connect_sequence_is_discarded_before_side_effects_and_stream_recovers";
        let system = System::new(test_name);
        let (dispatcher_mock, _, dispatcher_recording_arc) = make_recorder();
        let (neighborhood_mock, _, neighborhood_recording_arc) = make_recorder();
        let (accountant_mock, _, accountant_recording_arc) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        let client_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningful_stream_key(test_name);
        subject.keys_and_addrs.insert(stream_key, client_addr);
        subject.stream_info.insert(
            stream_key,
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: make_meaningless_route(&CRYPTDE_PAIR),
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![make_exit_service_from_key(PublicKey::new(b"exit_node"))],
                    ),
                    host: Host::new("example.com", TLS_PORT),
                })
                .protocol(ProxyProtocol::TLS)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher_mock)
            .neighborhood(neighborhood_mock)
            .accountant(accountant_mock)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        subject_addr
            .try_send(InboundClientData {
                timestamp: SystemTime::now(),
                client_addr,
                reception_port_opt: Some(TLS_PORT),
                sequence_number_opt: Some(0),
                last_data: false,
                is_clandestine: false,
                data: b"CONNECT https://example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n"
                    .to_vec(),
            })
            .unwrap();
        subject_addr
            .try_send(ExpiredCoresPackage::new(
                SocketAddr::from_str("3.4.5.6:7890").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                ClientResponsePayload_0v1 {
                    stream_key,
                    sequenced_packet: SequencedPacket {
                        data: b"invalid response".to_vec(),
                        sequence_number: u64::MAX,
                        last_data: false,
                    },
                },
                111,
            ))
            .unwrap();
        subject_addr
            .try_send(ExpiredCoresPackage::new(
                SocketAddr::from_str("3.4.5.6:7890").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                ClientResponsePayload_0v1 {
                    stream_key,
                    sequenced_packet: SequencedPacket {
                        data: b"valid response".to_vec(),
                        sequence_number: 0,
                        last_data: false,
                    },
                },
                222,
            ))
            .unwrap();

        System::current().stop();
        system.run();

        let dispatcher_recording = dispatcher_recording_arc.lock().unwrap();
        assert_eq!(dispatcher_recording.len(), 2);
        let delivered_response = dispatcher_recording.get_record::<TransmitDataMsg>(1);
        assert_eq!(delivered_response.data, b"valid response".to_vec());
        assert_eq!(delivered_response.sequence_number_opt, Some(1));
        assert_eq!(accountant_recording_arc.lock().unwrap().len(), 1);
        assert_eq!(neighborhood_recording_arc.lock().unwrap().len(), 2);
        TestLogHandler::new().exists_log_containing(&format!(
            "WARN: {test_name}: Discarding ClientResponsePayload for stream key {stream_key} because sequence number {} cannot accommodate its CONNECT offset",
            u64::MAX
        ));
    }

    #[test]
    fn duplicate_client_response_is_discarded_before_accounting_and_stream_recovers() {
        init_test_logging();
        let test_name =
            "duplicate_client_response_is_discarded_before_accounting_and_stream_recovers";
        let system = System::new(test_name);
        let (dispatcher_mock, _, dispatcher_recording_arc) = make_recorder();
        let (neighborhood_mock, _, neighborhood_recording_arc) = make_recorder();
        let (accountant_mock, _, accountant_recording_arc) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        let client_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningful_stream_key(test_name);
        subject.keys_and_addrs.insert(stream_key, client_addr);
        subject.stream_info.insert(
            stream_key,
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: make_meaningless_route(&CRYPTDE_PAIR),
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![make_exit_service_from_key(PublicKey::new(b"exit_node"))],
                    ),
                    host: Host::new("example.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher_mock)
            .neighborhood(neighborhood_mock)
            .accountant(accountant_mock)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        for (sequence_number, data, payload_len) in vec![
            (0, b"first response".to_vec(), 100),
            (0, b"duplicate response".to_vec(), 999),
            (1, b"second response".to_vec(), 200),
        ] {
            subject_addr
                .try_send(ExpiredCoresPackage::new(
                    SocketAddr::from_str("3.4.5.6:7890").unwrap(),
                    Some(make_wallet("irrelevant")),
                    return_route(cryptde),
                    ClientResponsePayload_0v1 {
                        stream_key,
                        sequenced_packet: SequencedPacket {
                            data,
                            sequence_number,
                            last_data: false,
                        },
                    },
                    payload_len,
                ))
                .unwrap();
        }

        System::current().stop();
        system.run();

        let dispatcher_recording = dispatcher_recording_arc.lock().unwrap();
        assert_eq!(dispatcher_recording.len(), 2);
        assert_eq!(
            dispatcher_recording.get_record::<TransmitDataMsg>(0).data,
            b"first response".to_vec()
        );
        assert_eq!(
            dispatcher_recording.get_record::<TransmitDataMsg>(1).data,
            b"second response".to_vec()
        );
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        assert_eq!(accountant_recording.len(), 2);
        assert_eq!(
            accountant_recording
                .get_record::<ReportServicesConsumedMessage>(0)
                .routing_payload_size,
            100
        );
        assert_eq!(
            accountant_recording
                .get_record::<ReportServicesConsumedMessage>(1)
                .routing_payload_size,
            200
        );
        assert_eq!(neighborhood_recording_arc.lock().unwrap().len(), 3);
        TestLogHandler::new().exists_log_containing(&format!(
            "WARN: {test_name}: Discarding ClientResponsePayload for stream key {stream_key} because the response sequence is duplicate or stale: 0"
        ));
    }

    #[test]
    fn proxy_server_sends_route_failure_for_connect_requests_to_ports_other_than_443() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let http_request = b"CONNECT https://realdomain.nu:8443 HTTP/1.1\r\nHost: https://bunkjunk.wrong:443\r\n\r\n";

        let (hopper_mock, _hopper_awaiter, _hopper_recording_arc) = make_recorder();
        let (neighborhood_mock, _, _neighborhood_recording_arc) = make_recorder();
        let (dispatcher_mock, _dispatcher_awaiter, dispatcher_recording_arc) = make_recorder();

        let neighborhood_mock = neighborhood_mock.route_query_response(Some(
            zero_hop_route_response(&cryptde.public_key(), cryptde, false),
        ));

        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let request_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(8443),
            sequence_number_opt: Some(0),
            last_data: false,
            is_clandestine: false,
            data: request_data.clone(),
        };

        let stream_key_parameters_arc = Arc::new(Mutex::new(vec![]));
        let stream_key_parameters_arc_thread = stream_key_parameters_arc.clone();

        thread::spawn(move || {
            let stream_key_factory = StreamKeyFactoryMock::new()
                .make_parameters(&stream_key_parameters_arc_thread)
                .make_result(stream_key);
            let system = System::new(
                "proxy_server_receives_connect_responds_with_ok_and_stores_stream_key_and_hostname",
            );
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.stream_key_factory = Box::new(stream_key_factory);
            let subject_addr: Addr<ProxyServer> = subject.start();
            let peer_actors = peer_actors_builder()
                .dispatcher(dispatcher_mock)
                .hopper(hopper_mock)
                .neighborhood(neighborhood_mock)
                .build();
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();
            system.run();
        });

        thread::sleep(Duration::from_millis(500));

        let expected_transmit_data_msg = TransmitDataMsg {
            endpoint: Endpoint::Socket(socket_addr),
            last_data: true,
            sequence_number_opt: Some(0),
            data: b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n".to_vec(),
        };

        let dispatcher_recording = dispatcher_recording_arc.lock().unwrap();
        let record = dispatcher_recording.get_record::<TransmitDataMsg>(0);

        assert_eq!(record, &expected_transmit_data_msg);
    }

    #[test]
    fn proxy_server_sends_error_and_shuts_down_stream_when_connect_host_unparseable() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let http_request = "CONNECT λ:🥓:λ HTTP/1.1\r\nHost: 🥓:🥔:🥔\r\n\r\n".as_bytes();

        let (hopper_mock, _hopper_awaiter, _hopper_recording_arc) = make_recorder();
        let (neighborhood_mock, _, _neighborhood_recording_arc) = make_recorder();
        let (dispatcher_mock, _dispatcher_awaiter, dispatcher_recording_arc) = make_recorder();

        let neighborhood_mock = neighborhood_mock.route_query_response(Some(
            zero_hop_route_response(&cryptde.public_key(), cryptde, false),
        ));

        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let request_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(8443),
            sequence_number_opt: Some(0),
            last_data: false,
            is_clandestine: false,
            data: request_data.clone(),
        };

        let stream_key_parameters_arc = Arc::new(Mutex::new(vec![]));
        let stream_key_parameters_arc_thread = stream_key_parameters_arc.clone();

        thread::spawn(move || {
            let stream_key_factory = StreamKeyFactoryMock::new()
                .make_parameters(&stream_key_parameters_arc_thread)
                .make_result(stream_key);
            let system = System::new(
                "proxy_server_receives_connect_responds_with_ok_and_stores_stream_key_and_hostname",
            );
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.stream_key_factory = Box::new(stream_key_factory);
            let subject_addr: Addr<ProxyServer> = subject.start();
            let peer_actors = peer_actors_builder()
                .dispatcher(dispatcher_mock)
                .hopper(hopper_mock)
                .neighborhood(neighborhood_mock)
                .build();
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();
            system.run();
        });

        thread::sleep(Duration::from_millis(500));

        let expected_transmit_data_msg = TransmitDataMsg {
            endpoint: Endpoint::Socket(socket_addr),
            last_data: true,
            sequence_number_opt: Some(0),
            data: b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n".to_vec(),
        };

        let dispatcher_recording = dispatcher_recording_arc.lock().unwrap();
        let record = dispatcher_recording.get_record::<TransmitDataMsg>(0);

        assert_eq!(&expected_transmit_data_msg, record);
    }

    #[test]
    fn proxy_server_receives_http_request_with_no_consuming_wallet_and_sends_impersonated_response()
    {
        init_test_logging();
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let (hopper, _, hopper_log_arc) = make_recorder();
        let (neighborhood, _, neighborhood_log_arc) = make_recorder();
        let (dispatcher, _, dispatcher_log_arc) = make_recorder();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let stream_key_factory = StreamKeyFactoryMock::new(); // can't make any stream keys; shouldn't have to
        let system = System::new("proxy_server_receives_http_request_with_no_consuming_wallet_and_sends_impersonated_response");
        let mut subject = ProxyServer::new(CRYPTDE_PAIR.clone(), true, None, false, false);
        subject.stream_key_factory = Box::new(stream_key_factory);
        subject.keys_and_addrs.insert(stream_key, socket_addr);
        let subject_addr: Addr<ProxyServer> = subject.start();
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher)
            .hopper(hopper)
            .neighborhood(neighborhood)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(msg_from_dispatcher).unwrap();

        System::current().stop();
        system.run();
        let neighborhood_recording = neighborhood_log_arc.lock().unwrap();
        assert!(neighborhood_recording.is_empty());
        let hopper_recording = hopper_log_arc.lock().unwrap();
        assert!(hopper_recording.is_empty());
        let dispatcher_recording = dispatcher_log_arc.lock().unwrap();
        let record = dispatcher_recording.get_record::<TransmitDataMsg>(0);
        let server_impersonator = ServerImpersonatorHttp {};
        assert_eq!(
            record,
            &TransmitDataMsg {
                endpoint: Endpoint::Socket(socket_addr),
                last_data: true,
                sequence_number_opt: Some(0),
                data: server_impersonator.consuming_wallet_absent(),
            }
        );
        TestLogHandler::new().exists_log_containing(
            "ERROR: ProxyServer: Browser request rejected due to missing consuming wallet",
        );
    }

    #[test]
    fn proxy_server_receives_tls_request_with_no_consuming_wallet_and_sends_impersonated_response()
    {
        init_test_logging();
        let tls_request = b"Fake TLS request";
        let (hopper, _, hopper_log_arc) = make_recorder();
        let (neighborhood, _, neighborhood_log_arc) = make_recorder();
        let (dispatcher, _, dispatcher_log_arc) = make_recorder();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = tls_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(TLS_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let stream_key_factory = StreamKeyFactoryMock::new(); // can't make any stream keys; shouldn't have to
        let system = System::new("proxy_server_receives_tls_request_with_no_consuming_wallet_and_sends_impersonated_response");
        let mut subject = ProxyServer::new(CRYPTDE_PAIR.clone(), true, None, false, false);
        subject.stream_key_factory = Box::new(stream_key_factory);
        subject.keys_and_addrs.insert(stream_key, socket_addr);
        let subject_addr: Addr<ProxyServer> = subject.start();
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher)
            .hopper(hopper)
            .neighborhood(neighborhood)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(msg_from_dispatcher).unwrap();

        System::current().stop();
        system.run();
        let neighborhood_recording = neighborhood_log_arc.lock().unwrap();
        assert!(neighborhood_recording.is_empty());
        let hopper_recording = hopper_log_arc.lock().unwrap();
        assert!(hopper_recording.is_empty());
        let dispatcher_recording = dispatcher_log_arc.lock().unwrap();
        let record = dispatcher_recording.get_record::<TransmitDataMsg>(0);
        let server_impersonator = ServerImpersonatorTls {};
        assert_eq!(
            record,
            &TransmitDataMsg {
                endpoint: Endpoint::Socket(socket_addr),
                last_data: true,
                sequence_number_opt: Some(0),
                data: server_impersonator.consuming_wallet_absent(),
            }
        );
        TestLogHandler::new().exists_log_containing(
            "ERROR: ProxyServer: Browser request rejected due to missing consuming wallet",
        );
    }

    #[test]
    fn proxy_server_receives_http_request_with_no_consuming_wallet_in_zero_hop_mode_and_handles_normally(
    ) {
        init_test_logging();
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let alias_cryptde = CRYPTDE_PAIR.alias.as_ref();
        let expected_data = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n".to_vec();
        let expected_data_inner = expected_data.clone();
        let expected_route =
            zero_hop_route_response(main_cryptde.public_key(), main_cryptde, false);
        let stream_key = StreamKey::make_meaningless_stream_key();
        let (hopper, hopper_awaiter, hopper_log_arc) = make_recorder();
        let neighborhood = Recorder::new().route_query_response(Some(expected_route.clone()));
        let neighborhood_log_arc = neighborhood.get_recording();
        let (dispatcher, _, dispatcher_log_arc) = make_recorder();
        thread::spawn(move || {
            let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
            let msg_from_dispatcher = InboundClientData {
                timestamp: SystemTime::now(),
                client_addr: socket_addr.clone(),
                reception_port_opt: Some(HTTP_PORT),
                sequence_number_opt: Some(0),
                last_data: true,
                is_clandestine: false,
                data: expected_data_inner,
            };
            let stream_key_factory = StreamKeyFactoryMock::new(); // can't make any stream keys; shouldn't have to
            let system = System::new("proxy_server_receives_http_request_with_no_consuming_wallet_in_zero_hop_mode_and_handles_normally");
            let mut subject = ProxyServer::new(CRYPTDE_PAIR.clone(), false, None, false, false);
            subject.stream_key_factory = Box::new(stream_key_factory);
            subject
                .keys_and_addrs
                .insert(stream_key.clone(), socket_addr);
            subject
                .stream_info
                .insert(stream_key, StreamInfoBuilder::new().build());
            let subject_addr: Addr<ProxyServer> = subject.start();
            let mut peer_actors = peer_actors_builder()
                .dispatcher(dispatcher)
                .hopper(hopper)
                .neighborhood(neighborhood)
                .build();
            peer_actors.proxy_server.route_result_sub =
                recipient!(&subject_addr, AddRouteResultMessage);
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();

            system.run();
        });
        hopper_awaiter.await_message_count(1);
        let neighborhood_recording = neighborhood_log_arc.lock().unwrap();
        assert_eq!(
            neighborhood_recording.get_record::<RouteQueryMessage>(0),
            &RouteQueryMessage {
                target_key_opt: None,
                target_component: Component::ProxyClient,
                return_component_opt: Some(Component::ProxyServer),
                payload_size: 47,
                host: Host::new("nowhere.com", HTTP_PORT),
                minimum_protocol_capabilities: 0,
                routing_receipt_request_opt: None,
            }
        );
        let dispatcher_recording = dispatcher_log_arc.lock().unwrap();
        assert!(dispatcher_recording.is_empty());
        let hopper_recording = hopper_log_arc.lock().unwrap();
        assert_eq!(
            hopper_recording.get_record::<IncipientCoresPackage>(0),
            &IncipientCoresPackage::new(
                main_cryptde,
                expected_route.route,
                MessageType::ClientRequest(VersionedData::new(
                    &crate::sub_lib::migrations::client_request_payload::MIGRATIONS,
                    &ClientRequestPayload_0v1 {
                        stream_key,
                        sequenced_packet: SequencedPacket::new(expected_data, 0, true),
                        target_hostname: "nowhere.com".to_string(),
                        target_port: HTTP_PORT,
                        protocol: ProxyProtocol::HTTP,
                        originator_public_key: alias_cryptde.public_key().clone(),
                        dns_attempt_id_opt: Some(0),
                        receipt_session_request_opt: None,
                    }
                )),
                main_cryptde.public_key()
            )
            .unwrap()
        );
    }

    #[test]
    fn proxy_server_receives_tls_request_with_no_consuming_wallet_in_zero_hop_mode_and_handles_normally(
    ) {
        init_test_logging();
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let alias_cryptde = CRYPTDE_PAIR.alias.as_ref();
        let expected_data = b"Fake TLS request".to_vec();
        let expected_data_inner = expected_data.clone();
        let expected_route = zero_hop_route_response(main_cryptde.public_key(), main_cryptde, true);
        let expected_route_inner = expected_route.clone();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let (hopper, hopper_awaiter, hopper_log_arc) = make_recorder();
        let neighborhood = Recorder::new().route_query_response(Some(expected_route.clone()));
        let (dispatcher, _, dispatcher_log_arc) = make_recorder();
        thread::spawn(move || {
            let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
            let msg_from_dispatcher = InboundClientData {
                timestamp: SystemTime::now(),
                client_addr: socket_addr.clone(),
                reception_port_opt: Some(TLS_PORT),
                sequence_number_opt: Some(0),
                last_data: true,
                is_clandestine: false,
                data: expected_data_inner,
            };
            let stream_key_factory = StreamKeyFactoryMock::new().make_result(stream_key.clone());
            let system = System::new("proxy_server_receives_tls_request_with_no_consuming_wallet_in_zero_hop_mode_and_handles_normally");
            let mut subject = ProxyServer::new(CRYPTDE_PAIR.clone(), false, None, false, false);
            subject.stream_key_factory = Box::new(stream_key_factory);
            subject.keys_and_addrs.insert(stream_key, socket_addr);
            subject.stream_info.insert(
                stream_key,
                StreamInfoBuilder::new()
                    .route(expected_route_inner)
                    .protocol(ProxyProtocol::TLS)
                    .build(),
            );
            let subject_addr: Addr<ProxyServer> = subject.start();
            let peer_actors = peer_actors_builder()
                .dispatcher(dispatcher)
                .hopper(hopper)
                .neighborhood(neighborhood)
                .build();
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();

            system.run();
        });
        hopper_awaiter.await_message_count(1);
        let dispatcher_recording = dispatcher_log_arc.lock().unwrap();
        assert!(dispatcher_recording.is_empty());
        let hopper_recording = hopper_log_arc.lock().unwrap();
        assert_eq!(
            hopper_recording.get_record::<IncipientCoresPackage>(0),
            &IncipientCoresPackage::new(
                main_cryptde,
                expected_route.route,
                MessageType::ClientRequest(VersionedData::new(
                    &crate::sub_lib::migrations::client_request_payload::MIGRATIONS,
                    &ClientRequestPayload_0v1 {
                        stream_key,
                        sequenced_packet: SequencedPacket::new(expected_data, 0, true),
                        target_hostname: "booga.com".to_string(),
                        target_port: TLS_PORT,
                        protocol: ProxyProtocol::TLS,
                        originator_public_key: alias_cryptde.public_key().clone(),
                        dns_attempt_id_opt: Some(0),
                        receipt_session_request_opt: None,
                    }
                ),),
                main_cryptde.public_key()
            )
            .unwrap()
        );
    }

    #[test]
    fn proxy_server_receives_http_request_with_existing_stream_key_from_dispatcher_then_sends_cores_package_to_hopper(
    ) {
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let alias_cryptde = CRYPTDE_PAIR.alias.as_ref();
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let hopper_mock = Recorder::new();
        let hopper_log_arc = hopper_mock.get_recording();
        let hopper_awaiter = hopper_mock.get_awaiter();
        let destination_key = PublicKey::from(&b"our destination"[..]);
        let neighborhood_mock = Recorder::new().route_query_response(Some(RouteQueryResponse {
            route: Route { hops: vec![] },
            expected_services: ExpectedServices::RoundTrip(
                vec![make_exit_service_from_key(destination_key.clone())],
                vec![],
            ),
            host: Host::new("booga.com", HTTP_PORT),
        }));
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let expected_http_request = PlainData::new(http_request);
        let route = Route { hops: vec![] };
        let expected_payload = ClientRequestPayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket {
                data: expected_http_request.into(),
                sequence_number: 0,
                last_data: true,
            },
            target_hostname: String::from("nowhere.com"),
            target_port: HTTP_PORT,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: alias_cryptde.public_key().clone(),
            dns_attempt_id_opt: Some(0),
            receipt_session_request_opt: None,
        };
        let expected_pkg = IncipientCoresPackage::new(
            main_cryptde,
            route.clone(),
            expected_payload.into(),
            &destination_key,
        )
        .unwrap();
        thread::spawn(move || {
            let stream_key_factory = StreamKeyFactoryMock::new(); // can't make any stream keys; shouldn't have to
            let system = System::new("proxy_server_receives_http_request_from_dispatcher_then_sends_cores_package_to_hopper");
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.stream_key_factory = Box::new(stream_key_factory);
            subject.keys_and_addrs.insert(stream_key, socket_addr);
            subject
                .stream_info
                .insert(stream_key.clone(), StreamInfoBuilder::new().build());
            let subject_addr: Addr<ProxyServer> = subject.start();
            let mut peer_actors = peer_actors_builder()
                .hopper(hopper_mock)
                .neighborhood(neighborhood_mock)
                .build();
            peer_actors.proxy_server.route_result_sub =
                recipient!(&subject_addr, AddRouteResultMessage);
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();

            system.run();
        });

        hopper_awaiter.await_message_count(1);
        let recording = hopper_log_arc.lock().unwrap();
        let record = recording.get_record::<IncipientCoresPackage>(0);
        assert_eq!(record, &expected_pkg);
    }

    #[test]
    fn proxy_server_receives_http_request_from_dispatcher_then_sends_multihop_cores_package_to_hopper(
    ) {
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let alias_cryptde = CRYPTDE_PAIR.alias.as_ref();
        let consuming_wallet = make_paying_wallet(b"paying wallet");
        let earning_wallet = make_wallet("earning wallet");
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let hopper_mock = Recorder::new();
        let hopper_log_arc = hopper_mock.get_recording();
        let hopper_awaiter = hopper_mock.get_awaiter();
        let payload_destination_key = PublicKey::new(&[3]);
        let route = Route::round_trip(
            RouteSegment::new(
                vec![
                    &main_cryptde.public_key(),
                    &PublicKey::new(&[1]),
                    &PublicKey::new(&[2]),
                    &payload_destination_key,
                ],
                Component::ProxyClient,
            ),
            RouteSegment::new(
                vec![
                    &payload_destination_key,
                    &PublicKey::new(&[2]),
                    &PublicKey::new(&[1]),
                    &main_cryptde.public_key(),
                ],
                Component::ProxyServer,
            ),
            main_cryptde,
            Some(consuming_wallet),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
        )
        .unwrap();
        let (neighborhood_mock, _, neighborhood_recording_arc) = make_recorder();
        let neighborhood_mock = neighborhood_mock.route_query_response(Some(RouteQueryResponse {
            route: route.clone(),
            expected_services: ExpectedServices::RoundTrip(
                vec![
                    ExpectedService::Exit(
                        PublicKey::new(&[3]),
                        earning_wallet.clone(),
                        rate_pack(101),
                    ),
                    ExpectedService::Nothing,
                ],
                vec![
                    ExpectedService::Nothing,
                    ExpectedService::Exit(PublicKey::new(&[3]), earning_wallet, rate_pack(102)),
                ],
            ),
            host: Host::new("booga.com", HTTP_PORT),
        }));
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let expected_http_request = PlainData::new(http_request);
        let expected_payload = ClientRequestPayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket {
                data: expected_http_request.into(),
                sequence_number: 0,
                last_data: true,
            },
            target_hostname: String::from("nowhere.com"),
            target_port: HTTP_PORT,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: alias_cryptde.public_key().clone(),
            dns_attempt_id_opt: Some(0),
            receipt_session_request_opt: None,
        };
        let expected_pkg = IncipientCoresPackage::new(
            main_cryptde,
            route.clone(),
            expected_payload.into(),
            &payload_destination_key,
        )
        .unwrap();
        thread::spawn(move || {
            let stream_key_factory = StreamKeyFactoryMock::new().make_result(stream_key);
            let system = System::new("proxy_server_receives_http_request_from_dispatcher_then_sends_multihop_cores_package_to_hopper");
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.stream_key_factory = Box::new(stream_key_factory);
            let subject_addr: Addr<ProxyServer> = subject.start();
            let mut peer_actors = peer_actors_builder()
                .hopper(hopper_mock)
                .neighborhood(neighborhood_mock)
                .build();
            peer_actors.proxy_server.route_result_sub =
                recipient!(&subject_addr, AddRouteResultMessage);
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();

            system.run();
        });

        hopper_awaiter.await_message_count(1);
        let recording = hopper_log_arc.lock().unwrap();
        let record = recording.get_record::<IncipientCoresPackage>(0);
        assert_eq!(record, &expected_pkg);
        let recording = neighborhood_recording_arc.lock().unwrap();
        let record = recording.get_record::<RouteQueryMessage>(0);
        assert_eq!(
            record,
            &RouteQueryMessage::data_indefinite_route_request(
                Host::new("nowhere.com", HTTP_PORT),
                47
            )
        );
    }

    #[test]
    fn proxy_server_sends_a_message_when_dns_retry_found_a_route() {
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let (proxy_server_mock, proxy_server_awaiter, proxy_server_recording_arc) = make_recorder();
        let expected_service = ExpectedService::Exit(
            CRYPTDE_PAIR.main.as_ref().public_key().clone(),
            make_wallet("walletAddress"),
            DEFAULT_RATE_PACK,
        );
        let route_query_response = Some(RouteQueryResponse {
            route: Route { hops: vec![] },
            expected_services: ExpectedServices::RoundTrip(
                vec![expected_service.clone()],
                vec![expected_service],
            ),
            host: Host::new("booga.com", HTTP_PORT),
        });
        let (neighborhood_mock, _, _) = make_recorder();
        let neighborhood_mock =
            neighborhood_mock.route_query_response(route_query_response.clone());
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };

        thread::spawn(move || {
            let stream_key_factory = StreamKeyFactoryMock::new().make_result(stream_key);
            let system = System::new("proxy_server_sends_a_message_when_dns_retry_found_a_route");
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.stream_key_factory = Box::new(stream_key_factory);
            let subject_addr: Addr<ProxyServer> = subject.start();
            let mut peer_actors = peer_actors_builder()
                .proxy_server(proxy_server_mock)
                .neighborhood(neighborhood_mock)
                .build();
            // Get the dns_retry_result recipient so we can partially mock it...
            let dns_retry_result_recipient = peer_actors.proxy_server.route_result_sub;
            peer_actors.proxy_server = ProxyServer::make_subs_from(&subject_addr);
            peer_actors.proxy_server.route_result_sub = dns_retry_result_recipient; //Partial mocking
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();

            system.run();
        });
        let expected_route_result_message = AddRouteResultMessage {
            stream_key,
            route_request_id: 1,
            result: Ok(route_query_response.unwrap()),
        };
        proxy_server_awaiter.await_message_count(1);
        let recording = proxy_server_recording_arc.lock().unwrap();
        let message = recording.get_record::<AddRouteResultMessage>(0);
        assert_eq!(message, &expected_route_result_message);
    }

    #[test]
    fn proxy_server_sends_a_message_when_dns_retry_cannot_find_a_route() {
        let test_name = "proxy_server_sends_a_message_when_dns_retry_cannot_find_a_route";
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let (proxy_server_mock, _, proxy_server_recording_arc) = make_recorder();
        let proxy_server_mock = proxy_server_mock
            .system_stop_conditions(match_lazily_every_type_id!(AddRouteResultMessage));
        let route_query_response = None;
        let (neighborhood_mock, _, _) = make_recorder();
        let neighborhood_mock =
            neighborhood_mock.route_query_response(route_query_response.clone());
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let stream_key_factory = StreamKeyFactoryMock::new().make_result(stream_key);
        let system = System::new(test_name);
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        subject.stream_key_factory = Box::new(stream_key_factory);
        let subject_addr: Addr<ProxyServer> = subject.start();
        let mut peer_actors = peer_actors_builder()
            .proxy_server(proxy_server_mock)
            .neighborhood(neighborhood_mock)
            .build();
        // Get the dns_retry_result recipient so we can partially mock it...
        let dns_retry_result_recipient = peer_actors.proxy_server.route_result_sub;
        peer_actors.proxy_server.route_result_sub = dns_retry_result_recipient; //Partial mocking
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(msg_from_dispatcher).unwrap();

        system.run();
        let recording = proxy_server_recording_arc.lock().unwrap();
        let message = recording.get_record::<AddRouteResultMessage>(0);
        assert_eq!(message.stream_key, stream_key);
        assert_eq!(
            message.result,
            Err("Failed to find route; destination and stream redacted".to_string())
        );
    }

    #[test]
    fn proxy_server_sends_a_message_with_error_when_quad_zeros_are_detected() {
        init_test_logging();
        let test_name = "proxy_server_sends_a_message_with_error_when_quad_zeros_are_detected";
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: 0.0.0.0\r\n\r\n";
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let stream_key_factory = StreamKeyFactoryMock::new().make_result(stream_key);
        let system = System::new(test_name);
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.stream_key_factory = Box::new(stream_key_factory);
        subject.logger = Logger::new(test_name);
        let subject_addr: Addr<ProxyServer> = subject.start();
        let peer_actors = peer_actors_builder().build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(msg_from_dispatcher).unwrap();

        System::current().stop();
        system.run();

        TestLogHandler::new().exists_log_containing(&format!("ERROR: {test_name}: Request to wildcard IP detected - 0.0.0.0 (Most likely because Blockchain Service URL is not set)"));
    }

    #[test]
    fn proxy_server_uses_existing_route() {
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let alias_cryptde = CRYPTDE_PAIR.alias.as_ref();
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let destination_key = PublicKey::from(&b"our destination"[..]);
        let route = Route { hops: vec![] };
        let route_query_response = RouteQueryResponse {
            route: route.clone(),
            expected_services: ExpectedServices::RoundTrip(
                vec![make_exit_service_from_key(destination_key.clone())],
                vec![],
            ),
            host: Host::new("booga.com", HTTP_PORT),
        };
        let (hopper_mock, hopper_awaiter, hopper_recording_arc) = make_recorder();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr,
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let expected_payload = ClientRequestPayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket {
                data: PlainData::new(http_request).into(),
                sequence_number: 0,
                last_data: true,
            },
            target_hostname: String::from("nowhere.com"),
            target_port: HTTP_PORT,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: alias_cryptde.public_key().clone(),
            dns_attempt_id_opt: Some(0),
            receipt_session_request_opt: None,
        };
        let expected_pkg = IncipientCoresPackage::new(
            main_cryptde,
            route,
            expected_payload.into(),
            &destination_key,
        )
        .unwrap();

        thread::spawn(move || {
            let stream_key_factory = StreamKeyFactoryMock::new().make_result(stream_key);
            let system = System::new("proxy_server_uses_existing_route");
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.stream_key_factory = Box::new(stream_key_factory);
            subject
                .keys_and_addrs
                .insert(stream_key.clone(), socket_addr);
            subject.stream_info.insert(
                stream_key,
                StreamInfoBuilder::new().route(route_query_response).build(),
            );
            subject.next_return_route_id = Cell::new(4444);
            let subject_addr: Addr<ProxyServer> = subject.start();
            let peer_actors = peer_actors_builder().hopper(hopper_mock).build();
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();
            subject_addr.try_send(msg_from_dispatcher).unwrap();

            System::current().stop();
            system.run();
        });

        hopper_awaiter.await_message_count(1);
        let recording = hopper_recording_arc.lock().unwrap();
        let record = recording.get_record::<IncipientCoresPackage>(0);
        assert_eq!(record, &expected_pkg);
    }

    #[test]
    fn proxy_server_sends_message_to_accountant_about_all_services_consumed_on_the_route_over() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let now = SystemTime::now();
        let routing_node_1_public_key = PublicKey::new(&[1]);
        let routing_node_2_public_key = PublicKey::new(&[2]);
        let exit_node_public_key = PublicKey::new(&[3]);
        let key_bytes = b"__originating consuming wallet__";
        let keypair = Bip32EncryptionKeyProvider::from_raw_secret(key_bytes).unwrap();
        let originating_consuming_wallet = Wallet::from(keypair);
        let routing_node_1_earning_wallet = make_wallet("route 1 earning wallet");
        let routing_node_2_earning_wallet = make_wallet("route 2 earning wallet");
        let exit_node_earning_wallet = make_wallet("exit earning wallet");
        let routing_node_1_rate_pack = rate_pack(101);
        let routing_node_2_rate_pack = rate_pack(102);
        let exit_node_rate_pack = rate_pack(103);
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let (accountant_mock, _, accountant_recording_arc) = make_recorder();
        let (hopper_mock, _, hopper_recording_arc) = make_recorder();
        let (proxy_server_mock, _, proxy_server_recording_arc) = make_recorder();
        let over_route_segment = RouteSegment::new(
            vec![
                &cryptde.public_key(),
                &routing_node_1_public_key,
                &routing_node_2_public_key,
                &exit_node_public_key,
            ],
            Component::ProxyClient,
        );
        let back_route_segment = RouteSegment::new(
            vec![
                &exit_node_public_key,
                &routing_node_2_public_key,
                &routing_node_1_public_key,
                &cryptde.public_key(),
            ],
            Component::ProxyServer,
        );
        let route = Route::round_trip(
            over_route_segment,
            back_route_segment,
            cryptde,
            Some(originating_consuming_wallet),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
        )
        .unwrap();
        let route_query_response = RouteQueryResponse {
            route,
            expected_services: ExpectedServices::RoundTrip(
                vec![
                    ExpectedService::Nothing,
                    ExpectedService::Routing(
                        routing_node_1_public_key.clone(),
                        routing_node_1_earning_wallet.clone(),
                        routing_node_1_rate_pack.clone(),
                    ),
                    ExpectedService::Routing(
                        routing_node_2_public_key.clone(),
                        routing_node_2_earning_wallet.clone(),
                        routing_node_2_rate_pack.clone(),
                    ),
                    ExpectedService::Exit(
                        exit_node_public_key.clone(),
                        exit_node_earning_wallet.clone(),
                        exit_node_rate_pack.clone(),
                    ),
                ],
                vec![
                    ExpectedService::Exit(
                        exit_node_public_key.clone(),
                        exit_node_earning_wallet.clone(),
                        exit_node_rate_pack,
                    ),
                    ExpectedService::Routing(
                        routing_node_2_public_key.clone(),
                        routing_node_2_earning_wallet.clone(),
                        routing_node_2_rate_pack,
                    ),
                    ExpectedService::Routing(
                        routing_node_1_public_key.clone(),
                        routing_node_1_earning_wallet.clone(),
                        routing_node_1_rate_pack,
                    ),
                    ExpectedService::Nothing,
                ],
            ),
            host: Host::new("booga.com", HTTP_PORT),
        };
        let source_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let system =
            System::new("proxy_server_sends_message_to_accountant_for_all_services_consumed");
        let peer_actors = peer_actors_builder()
            .accountant(accountant_mock)
            .hopper(hopper_mock)
            .proxy_server(proxy_server_mock)
            .build();
        let exit_payload_size = expected_data.len();
        let payload = ClientRequestPayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket::new(expected_data, 0, false),
            target_hostname: "nowhere.com".to_string(),
            target_port: HTTP_PORT,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: PublicKey::new(b"originator_public_key"),
            dns_attempt_id_opt: None,
            receipt_session_request_opt: None,
        };
        let logger = Logger::new("test");
        let args = TransmitToHopperArgs {
            main_cryptde: cryptde.dup(),
            payload,
            return_route_id: 4444,
            route_request_id: 0,
            client_addr: source_addr,
            timestamp: now,
            is_decentralized: true,
            require_service_receipt_capability: false,
            logger,
            hopper_sub: peer_actors.hopper.from_hopper_client,
            dispatcher_sub: peer_actors.dispatcher.from_dispatcher_client,
            accountant_sub: peer_actors.accountant.report_services_consumed,
            record_exit_request_for_receipt: peer_actors
                .proxy_server
                .record_exit_request_for_receipt,
            retire_stream_key_sub_opt: None,
        };

        let result = ProxyServer::try_transmit_to_hopper(args, route_query_response);

        System::current().stop();
        system.run();
        let recording = hopper_recording_arc.lock().unwrap();
        let mut record = recording.get_record::<IncipientCoresPackage>(0).clone();
        let payload_enc_length = record.payload.len();
        let _ = record.route.shift(cryptde);
        let _ = record.route.shift(&CryptDENull::from(
            &routing_node_1_public_key,
            TEST_DEFAULT_CHAIN,
        ));
        let _ = record.route.shift(&CryptDENull::from(
            &routing_node_2_public_key,
            TEST_DEFAULT_CHAIN,
        ));
        let _ = record.route.shift(&CryptDENull::from(
            &exit_node_public_key,
            TEST_DEFAULT_CHAIN,
        ));
        let _ = record.route.shift(&CryptDENull::from(
            &routing_node_2_public_key,
            TEST_DEFAULT_CHAIN,
        ));
        let _ = record.route.shift(&CryptDENull::from(
            &routing_node_1_public_key,
            TEST_DEFAULT_CHAIN,
        ));
        let _ = record.route.shift(cryptde);
        let recording = accountant_recording_arc.lock().unwrap();
        let record = recording.get_record::<ReportServicesConsumedMessage>(0);
        assert_eq!(recording.len(), 1);
        assert_eq!(
            record,
            &ReportServicesConsumedMessage {
                timestamp: now,
                exit: ExitServiceConsumed {
                    earning_wallet: exit_node_earning_wallet,
                    payload_size: exit_payload_size,
                    service_rate: exit_node_rate_pack.exit_service_rate,
                    byte_rate: exit_node_rate_pack.exit_byte_rate
                },
                routing_payload_size: payload_enc_length,
                routing: vec![
                    RoutingServiceConsumed {
                        earning_wallet: routing_node_1_earning_wallet,
                        service_rate: routing_node_1_rate_pack.routing_service_rate,
                        byte_rate: routing_node_1_rate_pack.routing_byte_rate,
                    },
                    RoutingServiceConsumed {
                        earning_wallet: routing_node_2_earning_wallet,
                        service_rate: routing_node_2_rate_pack.routing_service_rate,
                        byte_rate: routing_node_2_rate_pack.routing_byte_rate,
                    }
                ]
            }
        );
        let recording = proxy_server_recording_arc.lock().unwrap();
        assert_eq!(recording.len(), 0); // No StreamShutdownMsg: that's the important thing
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn metered_request_is_recorded_before_it_is_queued_to_hopper() {
        let system = System::new("metered_request_is_recorded_before_it_is_queued_to_hopper");
        let (recorder, _, recording) = make_recorder();
        let addr = recorder.start();
        let payer_wallet = make_paying_wallet(b"request observer payer");
        let payer_session = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let now_unix_s = ProxyServer::unix_time_now().unwrap();
        let authorization = ReceiptSessionPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
            payer_wallet.address(),
            payer_session.public_key().clone(),
            1_000_000,
            now_unix_s.saturating_sub(1),
            now_unix_s + 600,
            [0x51; 32],
        )
        .authorize(&payer_wallet)
        .unwrap();
        let receipt_request =
            crate::sub_lib::service_receipt::ReceiptSessionRequest::new(authorization, [0x52; 32])
                .unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let request_data = b"meter this exit request".to_vec();
        let args = TransmitToHopperArgs {
            main_cryptde: CRYPTDE_PAIR.main.dup(),
            payload: ClientRequestPayload_0v1 {
                stream_key,
                sequenced_packet: SequencedPacket::new(request_data.clone(), 0, false),
                target_hostname: "nowhere.com".to_string(),
                target_port: HTTP_PORT,
                protocol: ProxyProtocol::HTTP,
                originator_public_key: payer_session.public_key().clone(),
                dns_attempt_id_opt: None,
                receipt_session_request_opt: Some(receipt_request),
            },
            return_route_id: 4545,
            route_request_id: 0,
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            timestamp: SystemTime::now(),
            is_decentralized: true,
            require_service_receipt_capability: true,
            logger: Logger::new("metered_request_is_recorded_before_it_is_queued_to_hopper"),
            hopper_sub: recipient!(addr, IncipientCoresPackage),
            dispatcher_sub: recipient!(addr, TransmitDataMsg),
            accountant_sub: recipient!(addr, ReportServicesConsumedMessage),
            record_exit_request_for_receipt: recipient!(addr, RecordExitRequestForReceipt),
            retire_stream_key_sub_opt: None,
        };
        let expected_services = vec![ExpectedService::Exit(
            provider.public_key().clone(),
            make_wallet("request observer exit"),
            rate_pack(100),
        )];

        assert_eq!(
            ProxyServer::transmit_to_hopper(
                args,
                make_meaningless_route(&CRYPTDE_PAIR),
                expected_services,
            ),
            Ok(())
        );
        System::current().stop();
        system.run();

        let recording = recording.lock().unwrap();
        let package = recording
            .get_record_opt::<IncipientCoresPackage>(2)
            .expect("metered request package");
        assert_eq!(
            recording.get_record::<RecordExitRequestForReceipt>(0),
            &RecordExitRequestForReceipt {
                stream_key,
                payload_size: request_data.len() as u64,
                routing_payload_size: package.payload.len() as u64,
            }
        );
    }

    #[test]
    fn unencryptable_exit_route_fails_browser_without_hopper_or_accounting_side_effects() {
        let test_name =
            "unencryptable_exit_route_fails_browser_without_hopper_or_accounting_side_effects";
        let system = System::new(test_name);
        let (hopper, _, hopper_recording) = make_recorder();
        let (dispatcher, _, dispatcher_recording) = make_recorder();
        let (accountant, _, accountant_recording) = make_recorder();
        let (proxy_server, _, proxy_server_recording) = make_recorder();
        let peer_actors = peer_actors_builder()
            .hopper(hopper)
            .dispatcher(dispatcher)
            .accountant(accountant)
            .proxy_server(proxy_server)
            .build();
        let stream_key = StreamKey::make_meaningful_stream_key(test_name);
        let client_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let target_hostname = "unencryptable.example";
        let args = TransmitToHopperArgs {
            main_cryptde: CRYPTDE_PAIR.main.dup(),
            payload: ClientRequestPayload_0v1 {
                stream_key,
                sequenced_packet: SequencedPacket::new(b"request".to_vec(), 0, false),
                target_hostname: target_hostname.to_string(),
                target_port: HTTP_PORT,
                protocol: ProxyProtocol::HTTP,
                originator_public_key: CRYPTDE_PAIR.main.public_key().clone(),
                dns_attempt_id_opt: Some(0),
                receipt_session_request_opt: None,
            },
            return_route_id: 4747,
            route_request_id: 0,
            client_addr,
            timestamp: SystemTime::now(),
            is_decentralized: true,
            require_service_receipt_capability: false,
            logger: Logger::new(test_name),
            hopper_sub: peer_actors.hopper.from_hopper_client,
            dispatcher_sub: peer_actors.dispatcher.from_dispatcher_client,
            accountant_sub: peer_actors.accountant.report_services_consumed,
            record_exit_request_for_receipt: peer_actors
                .proxy_server
                .record_exit_request_for_receipt,
            retire_stream_key_sub_opt: None,
        };
        let route_query_response = RouteQueryResponse {
            route: make_meaningless_route(&CRYPTDE_PAIR),
            expected_services: ExpectedServices::RoundTrip(
                vec![ExpectedService::Exit(
                    PublicKey::new(&[]),
                    make_wallet("unusable exit"),
                    rate_pack(1),
                )],
                vec![],
            ),
            host: Host::new(target_hostname, HTTP_PORT),
        };

        let result = ProxyServer::try_transmit_to_hopper(args, route_query_response);

        System::current().stop();
        system.run();
        assert_eq!(
            result,
            Err(format!(
                "Could not create CORES package for stream {}: Could not encrypt payload: EncryptionError(EmptyKey)",
                stream_key
            ))
        );
        assert_eq!(hopper_recording.lock().unwrap().len(), 0);
        assert_eq!(accountant_recording.lock().unwrap().len(), 0);
        assert_eq!(proxy_server_recording.lock().unwrap().len(), 0);
        let dispatcher_recording = dispatcher_recording.lock().unwrap();
        assert_eq!(dispatcher_recording.len(), 1);
        assert_eq!(
            dispatcher_recording.get_record::<TransmitDataMsg>(0),
            &TransmitDataMsg {
                endpoint: Endpoint::Socket(client_addr),
                last_data: true,
                sequence_number_opt: Some(0),
                data: ServerImpersonatorHttp {}.route_query_failure_response(target_hostname),
            }
        );
    }

    #[test]
    fn try_transmit_to_hopper_orders_stream_shutdown_if_directed_to_do_so() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let (proxy_server_mock, _, proxy_server_recording_arc) = make_recorder();
        let route_query_response = RouteQueryResponse {
            route: make_meaningless_route(&CRYPTDE_PAIR),
            expected_services: ExpectedServices::RoundTrip(
                vec![ExpectedService::Nothing],
                vec![ExpectedService::Nothing],
            ),
            host: Host::new("booga.com", HTTP_PORT),
        };
        let source_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let system =
            System::new("proxy_server_sends_message_to_accountant_for_routing_service_consumed");
        let peer_actors = peer_actors_builder()
            .proxy_server(proxy_server_mock)
            .build();
        let payload = ClientRequestPayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket::new(expected_data, 0, false),
            target_hostname: "nowhere.com".to_string(),
            target_port: HTTP_PORT,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: PublicKey::new(b"originator_public_key"),
            dns_attempt_id_opt: None,
            receipt_session_request_opt: None,
        };
        let logger = Logger::new("test");
        let args = TransmitToHopperArgs {
            main_cryptde: cryptde.dup(),
            payload,
            return_route_id: 3333,
            route_request_id: 0,
            client_addr: source_addr,
            timestamp: SystemTime::now(),
            is_decentralized: false,
            require_service_receipt_capability: false,
            logger,
            hopper_sub: peer_actors.hopper.from_hopper_client,
            dispatcher_sub: peer_actors.dispatcher.from_dispatcher_client,
            accountant_sub: peer_actors.accountant.report_services_consumed,
            record_exit_request_for_receipt: peer_actors
                .proxy_server
                .record_exit_request_for_receipt,
            retire_stream_key_sub_opt: Some(peer_actors.proxy_server.stream_shutdown_sub),
        };

        let result = ProxyServer::try_transmit_to_hopper(args, route_query_response);

        System::current().stop();
        system.run();
        let recording = proxy_server_recording_arc.lock().unwrap();
        let record = recording.get_record::<StreamShutdownMsg>(0);
        assert_eq!(
            record,
            &StreamShutdownMsg {
                peer_addr: source_addr,
                stream_type: RemovedStreamType::NonClandestine(NonClandestineAttributes {
                    reception_port: 0,
                    sequence_number: 0,
                }),
                report_to_counterpart: false
            }
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn proxy_server_logs_messages_when_routing_services_are_not_requested() {
        init_test_logging();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let (accountant_mock, accountant_awaiter, _) = make_recorder();
        let (neighborhood_mock, _, _) = make_recorder();
        let mut route_query_response =
            zero_hop_route_response(&cryptde.public_key(), cryptde, false);
        route_query_response.expected_services = ExpectedServices::RoundTrip(
            vec![ExpectedService::Exit(
                cryptde.public_key().clone(),
                make_wallet("exit wallet"),
                rate_pack(3),
            )],
            vec![],
        );
        let neighborhood_mock =
            neighborhood_mock.route_query_response(Some(route_query_response.clone()));
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        thread::spawn(move || {
            let stream_key_factory = StreamKeyFactoryMock::new().make_result(stream_key);
            let system =
                System::new("proxy_server_logs_messages_when_routing_services_are_not_requested");
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.stream_key_factory = Box::new(stream_key_factory);
            let subject_addr: Addr<ProxyServer> = subject.start();
            let mut peer_actors = peer_actors_builder()
                .accountant(accountant_mock)
                .neighborhood(neighborhood_mock)
                .build();
            peer_actors.proxy_server.route_result_sub =
                recipient!(&subject_addr, AddRouteResultMessage);
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();
            subject_addr.try_send(msg_from_dispatcher).unwrap();
            system.run();
        });

        TestLogHandler::new()
            .await_log_containing("DEBUG: ProxyServer: No routing services requested.", 1000);
        //report about consumed services is sent anyway, exit service is mandatory ever
        accountant_awaiter.await_message_count(1)
    }

    #[test]
    fn one_route_result_transmits_every_packet_queued_while_the_route_was_pending() {
        let system = System::new(
            "one_route_result_transmits_every_packet_queued_while_the_route_was_pending",
        );
        let (hopper, _, hopper_recording) = make_recorder();
        let (accountant, _, accountant_recording) = make_recorder();
        let dispatcher = Recorder::new();
        let observer = Recorder::new();
        let peer_actors = peer_actors_builder()
            .hopper(hopper)
            .accountant(accountant)
            .dispatcher(dispatcher)
            .proxy_server(observer)
            .build();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let client_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let make_payload = |sequence_number, data: &[u8]| ClientRequestPayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket::new(data.to_vec(), sequence_number, false),
            target_hostname: "example.com".to_string(),
            target_port: HTTP_PORT,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: CRYPTDE_PAIR.alias.as_ref().public_key().clone(),
            dns_attempt_id_opt: Some(0),
            receipt_session_request_opt: None,
        };
        let first_payload = make_payload(0, b"first");
        let make_args = |payload, return_route_id| TransmitToHopperArgs {
            main_cryptde: CRYPTDE_PAIR.main.dup(),
            payload,
            return_route_id,
            route_request_id: 0,
            client_addr,
            timestamp: SystemTime::now(),
            is_decentralized: true,
            require_service_receipt_capability: false,
            logger: Logger::new("queued route test"),
            hopper_sub: peer_actors.hopper.from_hopper_client.clone(),
            dispatcher_sub: peer_actors.dispatcher.from_dispatcher_client.clone(),
            accountant_sub: peer_actors.accountant.report_services_consumed.clone(),
            record_exit_request_for_receipt: peer_actors
                .proxy_server
                .record_exit_request_for_receipt
                .clone(),
            retire_stream_key_sub_opt: None,
        };
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.subs = Some(make_proxy_server_out_subs());
        subject.stream_info.insert(
            stream_key,
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: first_payload.clone(),
                    retries_left: DNS_FAILURE_RETRIES,
                    active_attempt_id: 0,
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );

        let resolver_args = subject
            .queue_pending_route_packet(make_args(first_payload, 1))
            .unwrap()
            .unwrap();
        assert_eq!(resolver_args.route_request_id, 1);
        assert!(subject
            .queue_pending_route_packet(make_args(make_payload(1, b"second"), 2))
            .unwrap()
            .is_none());
        let route_query_response = RouteQueryResponse {
            route: make_meaningless_route(&CRYPTDE_PAIR),
            expected_services: ExpectedServices::RoundTrip(
                vec![make_exit_service_from_key(
                    CRYPTDE_PAIR.main.as_ref().public_key().clone(),
                )],
                vec![],
            ),
            host: Host::new("example.com", HTTP_PORT),
        };

        subject.handle_add_route_result_message(AddRouteResultMessage {
            stream_key,
            route_request_id: 1,
            result: Ok(route_query_response.clone()),
        });

        assert!(subject.pending_route_requests.is_empty());
        assert_eq!(
            subject.stream_info.get(&stream_key).unwrap().route_opt,
            Some(route_query_response)
        );
        System::current().stop();
        system.run();
        assert_eq!(hopper_recording.lock().unwrap().len(), 2);
        assert_eq!(accountant_recording.lock().unwrap().len(), 2);
    }

    #[test]
    fn stale_route_result_preserves_the_current_pending_request_without_side_effects() {
        init_test_logging();
        let system = System::new(
            "stale_route_result_preserves_the_current_pending_request_without_side_effects",
        );
        let (hopper, _, hopper_recording) = make_recorder();
        let (accountant, _, accountant_recording) = make_recorder();
        let peer_actors = peer_actors_builder()
            .hopper(hopper)
            .accountant(accountant)
            .build();
        let payload = make_request_payload(16, CRYPTDE_PAIR.main.as_ref());
        let stream_key = payload.stream_key;
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.subs = Some(make_proxy_server_out_subs());
        subject.stream_info.insert(
            stream_key,
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: payload.clone(),
                    retries_left: DNS_FAILURE_RETRIES,
                    active_attempt_id: 0,
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let args = TransmitToHopperArgs {
            main_cryptde: CRYPTDE_PAIR.main.dup(),
            payload,
            return_route_id: 1,
            route_request_id: 0,
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            timestamp: SystemTime::now(),
            is_decentralized: true,
            require_service_receipt_capability: false,
            logger: Logger::new("stale route result test"),
            hopper_sub: peer_actors.hopper.from_hopper_client,
            dispatcher_sub: peer_actors.dispatcher.from_dispatcher_client,
            accountant_sub: peer_actors.accountant.report_services_consumed,
            record_exit_request_for_receipt: peer_actors
                .proxy_server
                .record_exit_request_for_receipt,
            retire_stream_key_sub_opt: None,
        };
        subject.queue_pending_route_packet(args).unwrap().unwrap();
        let route_query_response = RouteQueryResponse {
            route: make_meaningless_route(&CRYPTDE_PAIR),
            expected_services: ExpectedServices::RoundTrip(
                vec![make_exit_service_from_key(
                    CRYPTDE_PAIR.main.as_ref().public_key().clone(),
                )],
                vec![],
            ),
            host: Host::new("example.com", HTTP_PORT),
        };

        subject.handle_add_route_result_message(AddRouteResultMessage {
            stream_key,
            route_request_id: 2,
            result: Ok(route_query_response),
        });

        assert_eq!(
            subject
                .pending_route_requests
                .get(&stream_key)
                .unwrap()
                .route_request_id,
            1
        );
        assert!(subject
            .stream_info
            .get(&stream_key)
            .unwrap()
            .route_opt
            .is_none());
        System::current().stop();
        system.run();
        assert!(hopper_recording.lock().unwrap().is_empty());
        assert!(accountant_recording.lock().unwrap().is_empty());
        TestLogHandler::new()
            .exists_log_containing("Discarding stale AddRouteResultMessage for stream key");
    }

    #[test]
    fn pending_route_queue_overflow_fails_the_browser_and_purges_the_stream() {
        let system =
            System::new("pending_route_queue_overflow_fails_the_browser_and_purges_the_stream");
        let (dispatcher, _, dispatcher_recording) = make_recorder();
        let peer_actors = peer_actors_builder().dispatcher(dispatcher).build();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let client_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let make_payload = |sequence_number| ClientRequestPayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket::new(vec![0x41], sequence_number, false),
            target_hostname: "example.com".to_string(),
            target_port: HTTP_PORT,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: CRYPTDE_PAIR.alias.as_ref().public_key().clone(),
            dns_attempt_id_opt: Some(0),
            receipt_session_request_opt: None,
        };
        let initial_payload = make_payload(0);
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.stream_info.insert(
            stream_key,
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: initial_payload,
                    retries_left: DNS_FAILURE_RETRIES,
                    active_attempt_id: 0,
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        subject.keys_and_addrs.insert(stream_key, client_addr);
        for sequence_number in 0..=MAX_PENDING_ROUTE_PACKETS_PER_STREAM {
            let args = TransmitToHopperArgs {
                main_cryptde: CRYPTDE_PAIR.main.dup(),
                payload: make_payload(sequence_number as u64),
                return_route_id: sequence_number as u32,
                route_request_id: 0,
                client_addr,
                timestamp: SystemTime::now(),
                is_decentralized: true,
                require_service_receipt_capability: false,
                logger: Logger::new("route queue overflow test"),
                hopper_sub: peer_actors.hopper.from_hopper_client.clone(),
                dispatcher_sub: peer_actors.dispatcher.from_dispatcher_client.clone(),
                accountant_sub: peer_actors.accountant.report_services_consumed.clone(),
                record_exit_request_for_receipt: peer_actors
                    .proxy_server
                    .record_exit_request_for_receipt
                    .clone(),
                retire_stream_key_sub_opt: None,
            };
            let result = subject.queue_pending_route_packet(args);
            if sequence_number < MAX_PENDING_ROUTE_PACKETS_PER_STREAM {
                assert!(result.is_ok());
            } else {
                match result {
                    Err(error) => assert!(error.contains("Pending route queue")),
                    Ok(_) => panic!("route queue overflow unexpectedly succeeded"),
                }
            }
        }

        assert!(!subject.stream_info.contains_key(&stream_key));
        assert!(!subject.pending_route_requests.contains_key(&stream_key));
        assert_eq!(subject.keys_and_addrs.a_to_b(&stream_key), None);
        System::current().stop();
        system.run();
        assert_eq!(dispatcher_recording.lock().unwrap().len(), 1);
    }

    #[test]
    fn route_result_message_handler_ignores_result_for_missing_stream() {
        init_test_logging();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();

        subject.handle_add_route_result_message(AddRouteResultMessage {
            stream_key,
            route_request_id: 1,
            result: Err("Some Error".to_string()),
        });

        assert!(subject.stream_info.is_empty());
        TestLogHandler::new().exists_log_containing(&format!(
            "WARN: ProxyServer: Discarding stale AddRouteResultMessage for stream key {} because the stream no longer exists",
            stream_key
        ));
    }

    #[test]
    fn route_result_message_handler_ignores_stream_without_dns_retries() {
        init_test_logging();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new().build(), // no DNS retries
        );

        subject.handle_add_route_result_message(AddRouteResultMessage {
            stream_key,
            route_request_id: 1,
            result: Err("Some Error".to_string()),
        });

        assert!(subject
            .stream_info(&stream_key)
            .unwrap()
            .route_opt
            .is_none());
        TestLogHandler::new().exists_log_containing(&format!(
            "WARN: ProxyServer: Discarding AddRouteResultMessage for stream key {} because the stream has no pending DNS request",
            stream_key
        ));
    }

    #[test]
    fn proxy_server_rejects_routes_without_an_exit_service() {
        let expected_services = vec![ExpectedService::Routing(
            PublicKey::from(&b"routing_key_1"[..]),
            make_wallet("routing_wallet_1"),
            rate_pack(8),
        )];

        assert_eq!(
            ProxyServer::report_on_exit_service(&expected_services, 10000),
            Err("Route does not demand an exit service".to_string())
        );
    }

    #[test]
    fn proxy_server_rejects_routes_with_more_than_one_exit_service() {
        let expected_services = vec![
            ExpectedService::Exit(
                PublicKey::from(&b"exit key 1"[..]),
                make_wallet("exit wallet 1"),
                rate_pack(6),
            ),
            ExpectedService::Exit(
                PublicKey::from(&b"exit key 2"[..]),
                make_wallet("exit wallet 2"),
                rate_pack(5),
            ),
        ];

        assert_eq!(
            ProxyServer::report_on_exit_service(&expected_services, 10000),
            Err("Route demands more than one exit service".to_string())
        );
    }

    #[test]
    fn proxy_server_receives_http_request_from_dispatcher_but_neighborhood_cant_make_route() {
        init_test_logging();
        let test_name =
            "proxy_server_receives_http_request_from_dispatcher_but_neighborhood_cant_make_route";
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let (neighborhood_mock, neighborhood_awaiter, neighborhood_recording_arc) = make_recorder();
        let neighborhood_mock = neighborhood_mock.route_query_response(None);
        let dispatcher = Recorder::new();
        let dispatcher_awaiter = dispatcher.get_awaiter();
        let dispatcher_recording_arc = dispatcher.get_recording();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let expected_data = http_request.to_vec();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            data: expected_data.clone(),
            is_clandestine: false,
        };
        thread::spawn(move || {
            let system = System::new(test_name);
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.stream_key_factory =
                Box::new(StreamKeyFactoryMock::new().make_result(stream_key));
            subject.logger = Logger::new(test_name);
            let subject_addr: Addr<ProxyServer> = subject.start();
            let mut peer_actors = peer_actors_builder()
                .dispatcher(dispatcher)
                .neighborhood(neighborhood_mock)
                .build();
            peer_actors.proxy_server = ProxyServer::make_subs_from(&subject_addr);
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();

            system.run();
        });

        dispatcher_awaiter.await_message_count(1);
        neighborhood_awaiter.await_message_count(2);
        let recording = dispatcher_recording_arc.lock().unwrap();
        let record = recording.get_record::<TransmitDataMsg>(0);
        let expected_msg = TransmitDataMsg {
            endpoint: Endpoint::Socket(SocketAddr::from_str("1.2.3.4:5678").unwrap()),
            last_data: true,
            sequence_number_opt: Some(0),
            data: ServerImpersonatorHttp {}.route_query_failure_response("nowhere.com"),
        };
        assert_eq!(record, &expected_msg);
        let recording = neighborhood_recording_arc.lock().unwrap();
        let record = recording.get_record::<RouteQueryMessage>(0);
        assert_eq!(
            record,
            &RouteQueryMessage::data_indefinite_route_request(
                Host::new("nowhere.com", HTTP_PORT),
                47
            )
        );
        assert_eq!(
            recording.get_record::<RouteUseFailedMessage>(1),
            &RouteUseFailedMessage
        );
        TestLogHandler::new().exists_log_containing(&format!(
            "WARN: {test_name}: No route found for DNS retry; destination, stream and error redacted; retries left: 3"
        ));
    }

    #[test]
    fn proxy_server_rejects_a_one_way_route_from_a_request_for_a_round_trip_route() {
        let _system = System::new(
            "proxy_server_rejects_a_one_way_route_from_a_request_for_a_round_trip_route",
        );
        let peer_actors = peer_actors_builder().build();

        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let route_result = RouteQueryResponse {
            route: make_meaningless_route(&CRYPTDE_PAIR),
            expected_services: ExpectedServices::OneWay(vec![
                ExpectedService::Nothing,
                ExpectedService::Routing(
                    PublicKey::new(&[1]),
                    make_wallet("earning wallet 1"),
                    rate_pack(101),
                ),
                ExpectedService::Routing(
                    PublicKey::new(&[2]),
                    make_wallet("earning wallet 2"),
                    rate_pack(102),
                ),
                ExpectedService::Exit(
                    PublicKey::new(&[3]),
                    make_wallet("exit earning wallet"),
                    rate_pack(103),
                ),
            ]),
            host: Host::new("booga.com", HTTP_PORT),
        };
        let payload = ClientRequestPayload_0v1 {
            stream_key: StreamKey::make_meaningless_stream_key(),
            sequenced_packet: SequencedPacket {
                data: vec![],
                sequence_number: 0,
                last_data: false,
            },
            target_hostname: "booga.com".to_string(),
            target_port: 0,
            protocol: ProxyProtocol::TLS,
            originator_public_key: cryptde.public_key().clone(),
            dns_attempt_id_opt: None,
            receipt_session_request_opt: None,
        };
        let logger = Logger::new("ProxyServer");
        let source_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let args = TransmitToHopperArgs {
            main_cryptde: cryptde.dup(),
            payload,
            return_route_id: 2222,
            route_request_id: 0,
            client_addr: source_addr,
            timestamp: SystemTime::now(),
            is_decentralized: true,
            require_service_receipt_capability: false,
            logger,
            hopper_sub: peer_actors.hopper.from_hopper_client,
            dispatcher_sub: peer_actors.dispatcher.from_dispatcher_client,
            accountant_sub: peer_actors.accountant.report_services_consumed,
            record_exit_request_for_receipt: peer_actors
                .proxy_server
                .record_exit_request_for_receipt,
            retire_stream_key_sub_opt: None,
        };

        assert_eq!(
            ProxyServer::try_transmit_to_hopper(args, route_result),
            Err("Expected RoundTrip ExpectedServices but got OneWay".to_string())
        );
    }

    #[test]
    fn report_response_services_consumed_rejects_routing_first_without_panicking() {
        init_test_logging();
        let test_name = "report_response_services_consumed_rejects_routing_first_without_panicking";
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        let expected_services = vec![
            ExpectedService::Routing(
                PublicKey::from(&b"key"[..]),
                make_wallet("some wallet"),
                rate_pack(10),
            ),
            ExpectedService::Exit(
                PublicKey::from(&b"exit_key"[..]),
                make_wallet("exit"),
                rate_pack(11),
            ),
        ];

        subject.report_response_services_consumed(&expected_services, 1234, 3456);

        TestLogHandler::new().exists_log_containing(&format!(
            "WARN: {test_name}: Refusing to account for an invalid return-service shape"
        ));
    }

    #[test]
    fn route_shape_validation_rejects_ambiguous_or_unpayable_routes() {
        let exit_one = ExpectedService::Exit(
            PublicKey::from(&b"exit one"[..]),
            make_wallet("exit one"),
            rate_pack(1),
        );
        let exit_two = ExpectedService::Exit(
            PublicKey::from(&b"exit two"[..]),
            make_wallet("exit two"),
            rate_pack(2),
        );
        let routing = ExpectedService::Routing(
            PublicKey::from(&b"routing"[..]),
            make_wallet("routing"),
            rate_pack(3),
        );
        let route_response = |expected_services| RouteQueryResponse {
            route: make_meaningless_route(&CRYPTDE_PAIR),
            expected_services,
            host: Host::new("booga.com", HTTP_PORT),
        };

        assert!(!ProxyServer::route_can_transmit_request(
            &route_response(ExpectedServices::OneWay(vec![exit_one.clone()])),
            true,
        ));
        assert!(!ProxyServer::route_can_transmit_request(
            &route_response(ExpectedServices::RoundTrip(
                vec![exit_one.clone(), exit_two.clone()],
                vec![exit_one.clone()],
            )),
            true,
        ));
        assert!(!ProxyServer::route_can_transmit_request(
            &route_response(ExpectedServices::RoundTrip(
                vec![exit_one.clone()],
                vec![routing.clone(), exit_one.clone()],
            )),
            true,
        ));
        assert!(!ProxyServer::route_can_transmit_request(
            &route_response(ExpectedServices::RoundTrip(
                vec![exit_one.clone()],
                vec![exit_one.clone(), exit_two],
            )),
            true,
        ));
        assert!(ProxyServer::route_can_transmit_request(
            &route_response(ExpectedServices::RoundTrip(
                vec![routing.clone(), exit_one.clone()],
                vec![ExpectedService::Nothing, exit_one, routing],
            )),
            true,
        ));
    }

    #[test]
    fn proxy_server_receives_http_request_from_dispatcher_but_neighborhood_cant_make_route_with_no_expected_services(
    ) {
        init_test_logging();
        let test_name = "proxy_server_receives_http_request_from_dispatcher_but_neighborhood_cant_make_route_with_no_expected_services";
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let public_key = &cryptde.public_key();
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let (neighborhood_mock, _, neighborhood_recording_arc) = make_recorder();
        let route_query_response = RouteQueryResponse {
            route: Route::round_trip(
                RouteSegment::new(vec![public_key, public_key], Component::ProxyClient),
                RouteSegment::new(vec![public_key, public_key], Component::ProxyServer),
                cryptde,
                None,
                None,
            )
            .unwrap(),
            expected_services: ExpectedServices::RoundTrip(vec![], vec![]),
            host: Host::new("booga.com", HTTP_PORT),
        };
        let neighborhood_mock = neighborhood_mock.route_query_response(Some(route_query_response));
        let dispatcher = Recorder::new();
        let dispatcher_awaiter = dispatcher.get_awaiter();
        let dispatcher_recording_arc = dispatcher.get_recording();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            data: expected_data.clone(),
            is_clandestine: false,
        };
        let stream_key = StreamKey::make_meaningless_stream_key();
        thread::spawn(move || {
            let system = System::new(test_name);
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.logger = Logger::new(test_name);
            subject.stream_key_factory =
                Box::new(StreamKeyFactoryMock::new().make_result(stream_key));
            let subject_addr: Addr<ProxyServer> = subject.start();
            let mut peer_actors = peer_actors_builder()
                .dispatcher(dispatcher)
                .neighborhood(neighborhood_mock)
                .build();
            peer_actors.proxy_server = ProxyServer::make_subs_from(&subject_addr);
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();

            system.run();
        });

        dispatcher_awaiter.await_message_count(1);
        let recording = dispatcher_recording_arc.lock().unwrap();
        let record = recording.get_record::<TransmitDataMsg>(0);
        let expected_msg = TransmitDataMsg {
            endpoint: Endpoint::Socket(SocketAddr::from_str("1.2.3.4:5678").unwrap()),
            last_data: true,
            sequence_number_opt: Some(0),
            data: ServerImpersonatorHttp {}.route_query_failure_response("nowhere.com"),
        };
        assert_eq!(record, &expected_msg);
        let recording = neighborhood_recording_arc.lock().unwrap();
        let record = recording.get_record::<RouteQueryMessage>(0);
        assert_eq!(
            record,
            &RouteQueryMessage::data_indefinite_route_request(
                Host::new("nowhere.com", HTTP_PORT),
                47
            )
        );
        TestLogHandler::new().exists_log_containing(&format!(
            "WARN: {test_name}: No route found for DNS retry; destination, stream and error redacted; retries left: 3"
        ));
    }

    #[test]
    fn proxy_server_receives_tls_client_hello_from_dispatcher_then_sends_cores_package_to_hopper() {
        let tls_request = make_server_com_client_hello();
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let alias_cryptde = CRYPTDE_PAIR.alias.as_ref();
        let hopper_mock = Recorder::new();
        let hopper_log_arc = hopper_mock.get_recording();
        let hopper_awaiter = hopper_mock.get_awaiter();
        let destination_key = PublicKey::from(&b"our destination"[..]);
        let neighborhood_mock = Recorder::new().route_query_response(Some(RouteQueryResponse {
            route: Route { hops: vec![] },
            expected_services: ExpectedServices::RoundTrip(
                vec![make_exit_service_from_key(destination_key.clone())],
                vec![],
            ),
            host: Host::new("booga.com", TLS_PORT),
        }));
        let stream_key = StreamKey::make_meaningless_stream_key();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let expected_data = tls_request.clone();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(TLS_PORT),
            sequence_number_opt: Some(0),
            last_data: false,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let expected_tls_request = PlainData::new(tls_request.as_slice());
        let route = Route { hops: vec![] };
        let expected_payload = ClientRequestPayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket {
                data: expected_tls_request.into(),
                sequence_number: 0,
                last_data: false,
            },
            target_hostname: String::from("server.com"),
            target_port: TLS_PORT,
            protocol: ProxyProtocol::TLS,
            originator_public_key: alias_cryptde.public_key().clone(),
            dns_attempt_id_opt: Some(0),
            receipt_session_request_opt: None,
        };
        let expected_pkg = IncipientCoresPackage::new(
            main_cryptde,
            route.clone(),
            expected_payload.into(),
            &destination_key,
        )
        .unwrap();
        thread::spawn(move || {
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.stream_key_factory =
                Box::new(StreamKeyFactoryMock::new().make_result(stream_key.clone()));
            let system = System::new("proxy_server_receives_tls_client_hello_from_dispatcher_then_sends_cores_package_to_hopper");
            let subject_addr: Addr<ProxyServer> = subject.start();
            let mut peer_actors = peer_actors_builder()
                .hopper(hopper_mock)
                .neighborhood(neighborhood_mock)
                .build();
            peer_actors.proxy_server.route_result_sub =
                recipient!(&subject_addr, AddRouteResultMessage);
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();

            system.run();
        });

        hopper_awaiter.await_message_count(1);
        let recording = hopper_log_arc.lock().unwrap();
        let record = recording.get_record::<IncipientCoresPackage>(0);
        assert_eq!(record, &expected_pkg);
    }

    #[test]
    fn proxy_server_receives_tls_handshake_packet_other_than_client_hello_from_dispatcher_then_sends_cores_package_to_hopper(
    ) {
        let tls_request = &[
            0x16, // content_type: Handshake
            0x00, 0x00, 0x00, 0x00, // version, length: don't care
            0x10, // handshake_type: ClientKeyExchange (not important--just not ClientHello)
            0x00, 0x00, 0x00, // length: 0
        ];
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let alias_cryptde = CRYPTDE_PAIR.alias.as_ref();
        let hopper_mock = Recorder::new();
        let hopper_log_arc = hopper_mock.get_recording();
        let hopper_awaiter = hopper_mock.get_awaiter();
        let destination_key = PublicKey::from(&b"our destination"[..]);
        let neighborhood_mock = Recorder::new().route_query_response(Some(RouteQueryResponse {
            route: Route { hops: vec![] },
            expected_services: ExpectedServices::RoundTrip(
                vec![make_exit_service_from_key(destination_key.clone())],
                vec![],
            ),
            host: Host::new("booga.com", TLS_PORT),
        }));
        let stream_key = StreamKey::make_meaningless_stream_key();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let expected_data = tls_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(TLS_PORT),
            sequence_number_opt: Some(0),
            last_data: false,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let expected_tls_request = PlainData::new(tls_request);
        let route = Route { hops: vec![] };
        let expected_payload = ClientRequestPayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket {
                data: expected_tls_request.into(),
                sequence_number: 0,
                last_data: false,
            },
            target_hostname: "booga.com".to_string(),
            target_port: TLS_PORT,
            protocol: ProxyProtocol::TLS,
            originator_public_key: alias_cryptde.public_key().clone(),
            dns_attempt_id_opt: Some(0),
            receipt_session_request_opt: None,
        };
        let expected_pkg = IncipientCoresPackage::new(
            main_cryptde,
            route.clone(),
            expected_payload.into(),
            &destination_key,
        )
        .unwrap();
        thread::spawn(move || {
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.stream_key_factory =
                Box::new(StreamKeyFactoryMock::new().make_result(stream_key.clone()));
            subject
                .keys_and_addrs
                .insert(stream_key.clone(), socket_addr);
            subject.stream_info.insert(
                stream_key.clone(),
                StreamInfoBuilder::new()
                    .route(RouteQueryResponse {
                        route: Route { hops: vec![] },
                        expected_services: ExpectedServices::RoundTrip(
                            vec![make_exit_service_from_key(destination_key.clone())],
                            vec![],
                        ),
                        host: Host::new("booga.com", TLS_PORT),
                    })
                    .build(),
            );
            let system = System::new("proxy_server_receives_tls_client_hello_from_dispatcher_then_sends_cores_package_to_hopper");
            let subject_addr: Addr<ProxyServer> = subject.start();
            let peer_actors = peer_actors_builder()
                .hopper(hopper_mock)
                .neighborhood(neighborhood_mock)
                .build();
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();

            system.run();
        });
        hopper_awaiter.await_message_count(1);
        let recording = hopper_log_arc.lock().unwrap();
        let record = recording.get_record::<IncipientCoresPackage>(0);
        assert_eq!(record, &expected_pkg);
    }

    #[test]
    fn proxy_server_receives_tls_packet_other_than_handshake_from_dispatcher_then_sends_cores_package_to_hopper(
    ) {
        let test_name = "proxy_server_receives_tls_packet_other_than_handshake_from_dispatcher_then_sends_cores_package_to_hopper";
        let tls_request = &[
            0xFF, // content_type: don't care, just not Handshake
            0x00, 0x00, 0x00, 0x00, // version, length: don't care
        ];
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let alias_cryptde = CRYPTDE_PAIR.alias.as_ref();
        let hopper_mock = Recorder::new();
        let hopper_log_arc = hopper_mock.get_recording();
        let hopper_awaiter = hopper_mock.get_awaiter();
        let destination_key = PublicKey::from(&b"our destination"[..]);
        let neighborhood_mock = Recorder::new().route_query_response(Some(RouteQueryResponse {
            route: Route { hops: vec![] },
            expected_services: ExpectedServices::RoundTrip(
                vec![make_exit_service_from_key(destination_key.clone())],
                vec![],
            ),
            host: Host::new("booga.com", TLS_PORT),
        }));
        let client_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningful_stream_key(test_name);
        let expected_data = tls_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr,
            reception_port_opt: Some(TLS_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let expected_tls_request = PlainData::new(tls_request);
        let route = Route { hops: vec![] };
        let expected_payload = ClientRequestPayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket {
                data: expected_tls_request.into(),
                sequence_number: 0,
                last_data: true,
            },
            target_hostname: "booga.com".to_string(),
            target_port: TLS_PORT,
            protocol: ProxyProtocol::TLS,
            originator_public_key: alias_cryptde.public_key().clone(),
            dns_attempt_id_opt: Some(0),
            receipt_session_request_opt: None,
        };
        let expected_pkg = IncipientCoresPackage::new(
            main_cryptde,
            route.clone(),
            expected_payload.into(),
            &destination_key,
        )
        .unwrap();
        thread::spawn(move || {
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.keys_and_addrs.insert(stream_key, client_addr);
            subject.stream_info.insert(
                stream_key,
                StreamInfoBuilder::new()
                    .route(RouteQueryResponse {
                        route: Route { hops: vec![] },
                        expected_services: ExpectedServices::RoundTrip(
                            vec![make_exit_service_from_key(destination_key.clone())],
                            vec![],
                        ),
                        host: Host::new("booga.com", TLS_PORT),
                    })
                    .protocol(ProxyProtocol::TLS)
                    .build(),
            );
            let system = System::new(test_name);
            let subject_addr: Addr<ProxyServer> = subject.start();
            let peer_actors = peer_actors_builder()
                .hopper(hopper_mock)
                .neighborhood(neighborhood_mock)
                .build();
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();

            system.run();
        });

        hopper_awaiter.await_message_count(1);
        let recording = hopper_log_arc.lock().unwrap();
        let record = recording.get_record::<IncipientCoresPackage>(0);
        assert_eq!(record, &expected_pkg);
    }

    #[test]
    fn proxy_server_receives_tls_client_hello_from_dispatcher_but_neighborhood_cant_make_route() {
        init_test_logging();
        let tls_request = [
            0x16, // content_type: Handshake
            0x00, 0x00, 0x00, 0x00, // version, length: don't care
            0x01, // handshake_type: ClientHello
            0x00, 0x00, 0x00, 0x00, 0x00, // length, version: don't care
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, // random: don't care
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, // random: don't care
            0x00, // session_id_length
            0x00, 0x00, // cipher_suites_length
            0x00, // compression_methods_length
            0x00, 0x13, // extensions_length
            0x00, 0x00, // extension_type: server_name
            0x00, 0x0F, // extension_length
            0x00, 0x0D, // server_name_list_length
            0x00, // server_name_type
            0x00, 0x0A, // server_name_length
            b's', b'e', b'r', b'v', b'e', b'r', b'.', b'c', b'o', b'm', // server_name
        ]
        .to_vec();
        let test_name = "proxy_server_receives_tls_client_hello_from_dispatcher_but_neighborhood_cant_make_route";
        let dispatcher = Recorder::new();
        let dispatcher_awaiter = dispatcher.get_awaiter();
        let dispatcher_recording_arc = dispatcher.get_recording();
        let neighborhood = Recorder::new().route_query_response(None);
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(TLS_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            data: tls_request,
            is_clandestine: false,
        };
        thread::spawn(move || {
            let system = System::new(test_name);
            let mut subject = ProxyServer::new(
                CRYPTDE_PAIR.clone(),
                true,
                Some(STANDARD_CONSUMING_WALLET_BALANCE),
                false,
                false,
            );
            subject.logger = Logger::new(test_name);
            subject.stream_key_factory =
                Box::new(StreamKeyFactoryMock::new().make_result(stream_key));
            let subject_addr: Addr<ProxyServer> = subject.start();
            let mut peer_actors = peer_actors_builder()
                .dispatcher(dispatcher)
                .neighborhood(neighborhood)
                .build();
            peer_actors.proxy_server.route_result_sub =
                recipient!(&subject_addr, AddRouteResultMessage);
            subject_addr.try_send(BindMessage { peer_actors }).unwrap();

            subject_addr.try_send(msg_from_dispatcher).unwrap();

            system.run();
        });
        dispatcher_awaiter.await_message_count(1);
        let recording = dispatcher_recording_arc.lock().unwrap();
        let record = recording.get_record::<TransmitDataMsg>(0);
        let expected_msg = TransmitDataMsg {
            endpoint: Endpoint::Socket(SocketAddr::from_str("1.2.3.4:5678").unwrap()),
            last_data: true,
            sequence_number_opt: Some(0),
            data: ServerImpersonatorTls {}.route_query_failure_response("ignored"),
        };
        assert_eq!(record, &expected_msg);
    }

    #[test]
    fn proxy_server_receives_terminal_response_from_hopper() {
        init_test_logging();
        let test_name = "proxy_server_receives_terminal_response_from_hopper";
        let system = System::new(test_name);
        let (dispatcher, _, dispatcher_recording_arc) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningful_stream_key(test_name);
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![ExpectedService::Nothing],
                    ),
                    host: Host::new("booga.com", TLS_PORT),
                })
                .protocol(ProxyProtocol::TLS)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let remaining_route = return_route(cryptde);
        let client_response_payload = ClientResponsePayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket {
                data: b"16 bytes of data".to_vec(),
                sequence_number: 0,
                last_data: true,
            },
        };
        let first_expired_cores_package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            Some(make_wallet("consuming")),
            remaining_route,
            client_response_payload,
            0,
        );
        let second_expired_cores_package = first_expired_cores_package.clone();
        let peer_actors = peer_actors_builder().dispatcher(dispatcher).build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(first_expired_cores_package).unwrap(); // This will purge the stream key records
        subject_addr.try_send(second_expired_cores_package).unwrap(); // This will be discarded

        System::current().stop();
        system.run();
        let dispatcher_recording = dispatcher_recording_arc.lock().unwrap();
        let transmit_data_msg = dispatcher_recording.get_record::<TransmitDataMsg>(0);
        assert_eq!(transmit_data_msg.endpoint, Endpoint::Socket(socket_addr));
        assert_eq!(transmit_data_msg.last_data, true);
        assert_eq!(transmit_data_msg.data, b"16 bytes of data".to_vec());
        let tlh = TestLogHandler::new();
        tlh.exists_log_containing(&format!(
            "DEBUG: {test_name}: Retiring stream key {:?} due to last data received from the exit node",
            stream_key
        ));
        tlh.exists_log_containing(&format!(
            "ERROR: {test_name}: Can't pay for return services consumed: received response with unrecognized stream key {:?}. Ignoring",
            stream_key
        ));
    }

    #[test]
    fn log_straggling_packet_tolerates_timestamp_from_the_future() {
        let subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();
        let timestamp = SystemTime::now()
            .checked_add(Duration::from_secs(10))
            .unwrap();
        subject.log_straggling_packet(&stream_key, 10, &timestamp);
    }

    #[test]
    fn successful_client_response_clears_exit_host_penalty() {
        let system = System::new("successful_client_response_clears_exit_host_penalty");
        let (neighborhood_mock, _, neighborhood_recording_arc) = make_recorder();
        let (dispatcher_mock, _, _) = make_recorder();
        let (accountant_mock, _, _) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();
        let client_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let exit_public_key = PublicKey::from(&b"exit_key"[..]);
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), client_addr);
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: make_meaningless_route(&CRYPTDE_PAIR),
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![ExpectedService::Exit(
                            exit_public_key.clone(),
                            make_wallet("exit wallet"),
                            rate_pack(10),
                        )],
                    ),
                    host: Host::new("recovered.example", TLS_PORT),
                })
                .protocol(ProxyProtocol::TLS)
                .request_started_at(
                    SystemTime::now()
                        .checked_sub(Duration::from_millis(250))
                        .unwrap(),
                )
                .build(),
        );
        let subject_addr = subject.start();
        let empty_response = ClientResponsePayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket::new(vec![], 0, false),
        };
        let empty_package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            Some(make_wallet("irrelevant")),
            return_route(cryptde),
            empty_response,
            0,
        );
        let response = ClientResponsePayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket::new(vec![1, 2, 3], 1, false),
        };
        let package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            Some(make_wallet("irrelevant")),
            return_route(cryptde),
            response,
            0,
        );
        let peer_actors = peer_actors_builder()
            .neighborhood(neighborhood_mock)
            .dispatcher(dispatcher_mock)
            .accountant(accountant_mock)
            .build();

        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        subject_addr.try_send(empty_package).unwrap();
        subject_addr.try_send(package.clone()).unwrap();
        subject_addr.try_send(package).unwrap();

        System::current().stop();
        assert_eq!(system.run(), 0);
        let recording = neighborhood_recording_arc.lock().unwrap();
        assert_eq!(
            recording.get_record::<UpdateNodeRecordMetadataMessage>(0),
            &UpdateNodeRecordMetadataMessage {
                public_key: exit_public_key.clone(),
                metadata_change: NRMetadataChange::RemoveUnreachableHost {
                    hostname: "recovered.example".to_string(),
                },
            }
        );
        let route_success_metadata = recording.get_record::<UpdateNodeRecordMetadataMessage>(1);
        assert_eq!(route_success_metadata.public_key, exit_public_key);
        match &route_success_metadata.metadata_change {
            NRMetadataChange::RecordRouteSuccess {
                hostname,
                latency_ms,
            } => {
                assert_eq!(hostname, "recovered.example");
                assert!((250..=2_000).contains(latency_ms));
            }
            other => panic!("Expected route-success metadata, got {:?}", other),
        }
        assert_eq!(
            recording.get_record::<RouteUseSucceededMessage>(2),
            &RouteUseSucceededMessage
        );
        assert_eq!(recording.len(), 3);
    }

    #[test]
    fn long_lived_stream_refreshes_route_activity_without_changing_response_accounting() {
        let system = System::new(
            "long_lived_stream_refreshes_route_activity_without_changing_response_accounting",
        );
        let (dispatcher_mock, _, _) = make_recorder();
        let activity_and_accounting_recorder =
            Recorder::new().system_stop_conditions(match_lazily_every_type_id!(
                RouteUseSucceededMessage,
                ReportServicesConsumedMessage,
                ReportServicesConsumedMessage,
                RouteUseSucceededMessage,
                ReportServicesConsumedMessage
            ));
        let activity_and_accounting_recording_arc =
            activity_and_accounting_recorder.get_recording();
        let activity_and_accounting_addr = activity_and_accounting_recorder.start();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();
        subject.keys_and_addrs.insert(
            stream_key.clone(),
            SocketAddr::from_str("1.2.3.4:5678").unwrap(),
        );
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: make_meaningless_route(&CRYPTDE_PAIR),
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![ExpectedService::Exit(
                            PublicKey::from(&b"heartbeat_exit"[..]),
                            make_wallet("heartbeat exit wallet"),
                            rate_pack(10),
                        )],
                    ),
                    host: Host::new("heartbeat.example", TLS_PORT),
                })
                .protocol(ProxyProtocol::TLS)
                .build(),
        );
        let response_package = |sequence_number| {
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                ClientResponsePayload_0v1 {
                    stream_key: stream_key.clone(),
                    sequenced_packet: SequencedPacket::new(
                        vec![sequence_number as u8 + 1],
                        sequence_number,
                        false,
                    ),
                },
                0,
            )
        };
        let first_response = response_package(0);
        let suppressed_response = response_package(1);
        let heartbeat_response = response_package(2);
        let mut peer_actors = peer_actors_builder().dispatcher(dispatcher_mock).build();
        peer_actors.neighborhood.route_use_succeeded =
            recipient!(activity_and_accounting_addr, RouteUseSucceededMessage);
        peer_actors.accountant.report_services_consumed =
            recipient!(activity_and_accounting_addr, ReportServicesConsumedMessage);
        let subject_addr = subject.start();
        let started_at = Instant::now();

        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        subject_addr
            .try_send(AssertionsMessage {
                assertions: Box::new(move |proxy_server: &mut ProxyServer| {
                    proxy_server.handle_client_response_payload_at(first_response, started_at);
                    proxy_server.handle_client_response_payload_at(
                        suppressed_response,
                        started_at + ROUTE_ACTIVITY_HEARTBEAT_INTERVAL - Duration::from_millis(1),
                    );
                    proxy_server.handle_client_response_payload_at(
                        heartbeat_response,
                        started_at + ROUTE_ACTIVITY_HEARTBEAT_INTERVAL,
                    );
                }),
            })
            .unwrap();

        assert_eq!(system.run(), 0);
        let recording = activity_and_accounting_recording_arc.lock().unwrap();
        let heartbeat_indices = (0..recording.len())
            .filter(|index| {
                recording
                    .get_record_opt::<RouteUseSucceededMessage>(*index)
                    .is_some()
            })
            .collect::<Vec<_>>();
        let accounting_indices = (0..recording.len())
            .filter(|index| {
                recording
                    .get_record_opt::<ReportServicesConsumedMessage>(*index)
                    .is_some()
            })
            .collect::<Vec<_>>();
        assert_eq!(heartbeat_indices, vec![0, 3]);
        assert_eq!(accounting_indices, vec![1, 2, 4]);
        assert_eq!(recording.len(), 5);
    }

    #[test]
    fn handle_client_response_payload_purges_stream_keys_for_terminal_response() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.subs = Some(make_proxy_server_out_subs());

        let stream_key = StreamKey::make_meaningless_stream_key();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(vec![], vec![]),
                    host: Host::new("booga.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .tunneled_host("hostname")
                .build(),
        );
        let client_response_payload = ClientResponsePayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket::new(vec![], 1, true),
        };
        let (dispatcher_mock, _, _) = make_recorder();
        let peer_actors = peer_actors_builder().dispatcher(dispatcher_mock).build();
        subject.subs.as_mut().unwrap().dispatcher = peer_actors.dispatcher.from_dispatcher_client;
        let expired_cores_package: ExpiredCoresPackage<ClientResponsePayload_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                client_response_payload.into(),
                0,
            );

        subject.handle_client_response_payload(expired_cores_package);

        assert!(subject.keys_and_addrs.is_empty());
        assert!(subject.stream_info.get(&stream_key).is_none());
    }

    #[test]
    fn proxy_server_schedules_stream_key_purge_once_shutdown_order_is_received_for_stream() {
        let common_msg = StreamShutdownMsg {
            peer_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            stream_type: RemovedStreamType::NonClandestine(NonClandestineAttributes {
                reception_port: 0,
                sequence_number: 0,
            }),
            report_to_counterpart: true,
        };
        assert_stream_is_purged_with_a_delay(StreamShutdownMsg {
            report_to_counterpart: true,
            ..common_msg.clone()
        });
        assert_stream_is_purged_with_a_delay(StreamShutdownMsg {
            report_to_counterpart: false,
            ..common_msg
        });
    }

    fn assert_stream_is_purged_with_a_delay(msg: StreamShutdownMsg) {
        /*
        +------------------------------------------------------------------+
        | (0ms)                                                            |
        | Stream shutdown is ordered                                       |
        +------------------------------------------------------------------+
                      |
                      v
        +------------------------------------------------------------------+
        | (400ms) (stream_key_purge_delay_in_millis - offset_in_millis)    |
        | Pre-purge assertion message finds records                        |
        +------------------------------------------------------------------+
                      |
                      v
        +------------------------------------------------------------------+
        | (500ms) (stream_key_purge_delay_in_millis)                       |
        | Stream is purged                                                 |
        +------------------------------------------------------------------+
                      |
                      v
        +------------------------------------------------------------------+
        | (600ms) (stream_key_purge_delay_in_millis + offset_in_millis)    |
        | Post-purge assertion message finds no records                    |
        +------------------------------------------------------------------+
        */

        init_test_logging();
        let test_name =
            "proxy_server_schedules_stream_key_purge_once_shutdown_order_is_received_for_stream";
        let stream_key_purge_delay_in_millis = 500;
        let offset_in_millis = 100;
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.stream_key_purge_delay = Duration::from_millis(stream_key_purge_delay_in_millis);
        subject.logger = Logger::new(&test_name);
        subject.subs = Some(make_proxy_server_out_subs());
        let stream_key = StreamKey::make_meaningful_stream_key(&test_name);
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), msg.peer_addr.clone());
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(vec![], vec![]),
                    host: Host::new("booga.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .tunneled_host("hostname")
                .build(),
        );
        let proxy_server_addr = subject.start();
        let schedule_stream_key_purge_sub = proxy_server_addr.clone().recipient();
        let mut peer_actors = peer_actors_builder().build();
        peer_actors.proxy_server.schedule_stream_key_purge = schedule_stream_key_purge_sub;
        let system = System::new(test_name);
        let bind_msg = BindMessage { peer_actors };
        proxy_server_addr.try_send(bind_msg).unwrap();
        let time_before_sending_package = SystemTime::now();

        proxy_server_addr.try_send(msg).unwrap();

        let time_after_sending_package = time_before_sending_package
            .checked_add(Duration::from_secs(1))
            .unwrap();
        let pre_purge_assertions = AssertionsMessage {
            assertions: Box::new(move |proxy_server: &mut ProxyServer| {
                let stream_info = proxy_server.stream_info.get(&stream_key).unwrap();
                let purge_timestamp = stream_info.time_to_live_opt.unwrap();
                assert!(
                    time_before_sending_package <= purge_timestamp
                        && purge_timestamp <= time_after_sending_package
                );
                assert!(!proxy_server.stream_info.get(&stream_key).is_none());
                assert!(!proxy_server.keys_and_addrs.is_empty());
                TestLogHandler::new().exists_log_containing(&format!(
                    "DEBUG: {test_name}: Client closed a tunneled stream; destination and stream identifiers redacted. It will be purged after {stream_key_purge_delay_in_millis}ms."
                ));
            }),
        };
        proxy_server_addr
            .try_send(MessageScheduler {
                scheduled_msg: pre_purge_assertions,
                delay: Duration::from_millis(stream_key_purge_delay_in_millis - offset_in_millis), // 400ms
            })
            .unwrap();
        let post_purge_assertions = AssertionsMessage {
            assertions: Box::new(move |proxy_server: &mut ProxyServer| {
                assert!(proxy_server.keys_and_addrs.is_empty());
                assert!(proxy_server.stream_info.get(&stream_key).is_none());
                TestLogHandler::new().exists_log_containing(&format!(
                    "DEBUG: {test_name}: Retiring stream key {:?}",
                    stream_key
                ));
                System::current().stop();
            }),
        };
        proxy_server_addr
            .try_send(MessageScheduler {
                scheduled_msg: post_purge_assertions,
                delay: Duration::from_millis(stream_key_purge_delay_in_millis + offset_in_millis), // 600ms
            })
            .unwrap();
        system.run();
    }

    #[test]
    fn straggling_packets_are_charged_and_dropped_as_the_browser_stopped_awaiting_them_anyway() {
        init_test_logging();
        let test_name = "straggling_packets_are_charged_and_dropped_as_the_browser_stopped_awaiting_them_anyway";
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        subject.subs = Some(make_proxy_server_out_subs());
        let stream_key = StreamKey::make_meaningful_stream_key(test_name);
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        let exit_key = PublicKey::new(&b"blah"[..]);
        let exit_wallet = make_wallet("abc");
        let exit_rates = RatePack {
            routing_byte_rate: 0,
            routing_service_rate: 0,
            exit_byte_rate: 100,
            exit_service_rate: 60000,
        };
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![ExpectedService::Exit(
                            exit_key,
                            exit_wallet.clone(),
                            exit_rates.clone(),
                        )],
                    ),
                    host: Host::new("booga.com", HTTP_PORT),
                })
                .tunneled_host("hostname")
                .protocol(ProxyProtocol::HTTP)
                .time_to_live(SystemTime::now())
                .build(),
        );
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let (dispatcher, _, dispatcher_recording_arc) = make_recorder();
        let proxy_server_addr = subject.start();
        let peer_actors = peer_actors_builder()
            .accountant(accountant)
            .dispatcher(dispatcher)
            .build();
        let system = System::new(test_name);
        let response_data = vec![0; 30];
        let client_response_payload = ClientResponsePayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket::new(response_data.clone(), 1, true),
        };
        let expired_cores_package: ExpiredCoresPackage<ClientResponsePayload_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                client_response_payload.into(),
                5432,
            );
        let bind_msg = BindMessage { peer_actors };
        proxy_server_addr.try_send(bind_msg).unwrap();

        proxy_server_addr.try_send(expired_cores_package).unwrap();

        System::current().stop();
        system.run();
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        let msg = accountant_recording.get_record::<ReportServicesConsumedMessage>(0);
        assert_eq!(
            &msg.exit,
            &ExitServiceConsumed {
                earning_wallet: exit_wallet,
                payload_size: response_data.len(),
                service_rate: exit_rates.exit_service_rate,
                byte_rate: exit_rates.exit_byte_rate,
            }
        );
        assert_eq!(msg.routing_payload_size, 5432);
        let dispatcher_recording = dispatcher_recording_arc.lock().unwrap();
        let len = dispatcher_recording.len();
        assert_eq!(len, 0);
        TestLogHandler::new().exists_log_containing(&format!(
            "DEBUG: {test_name}: Straggling packet of length 5432 received for a \
            stream key {:?} after a delay of",
            stream_key
        ));
    }

    #[test]
    fn proxy_server_receives_nonterminal_response_from_hopper() {
        let system = System::new("proxy_server_receives_nonterminal_response_from_hopper");
        let (dispatcher_mock, _, dispatcher_log_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let irrelevant_public_key = PublicKey::from(&b"irrelevant"[..]);
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        let incoming_route_d_wallet = make_wallet("D Earning");
        let incoming_route_e_wallet = make_wallet("E Earning");
        let incoming_route_f_wallet = make_wallet("F Earning");
        let rate_pack_d = rate_pack(101);
        let rate_pack_e = rate_pack(102);
        let rate_pack_f = rate_pack(103);
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![
                            ExpectedService::Exit(
                                irrelevant_public_key.clone(),
                                incoming_route_d_wallet.clone(),
                                rate_pack_d,
                            ),
                            ExpectedService::Routing(
                                irrelevant_public_key.clone(),
                                incoming_route_e_wallet.clone(),
                                rate_pack_e,
                            ),
                            ExpectedService::Routing(
                                irrelevant_public_key.clone(),
                                incoming_route_f_wallet.clone(),
                                rate_pack_f,
                            ),
                            ExpectedService::Nothing,
                        ],
                    ),
                    host: Host::new("booga.com", HTTP_PORT),
                })
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let first_client_response_payload = ClientResponsePayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket {
                data: b"some data".to_vec(),
                sequence_number: 0,
                last_data: false,
            },
        };
        let first_exit_size = first_client_response_payload.sequenced_packet.data.len();
        let first_expired_cores_package: ExpiredCoresPackage<ClientResponsePayload_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                first_client_response_payload.into(),
                0,
            );
        let routing_size = first_expired_cores_package.payload_len;
        let second_client_response_payload = ClientResponsePayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket {
                data: b"other data".to_vec(),
                sequence_number: 1,
                last_data: false,
            },
        };
        let second_exit_size = second_client_response_payload.sequenced_packet.data.len();
        let second_expired_cores_package: ExpiredCoresPackage<ClientResponsePayload_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.5:1235").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                second_client_response_payload.into(),
                0,
            );
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher_mock)
            .accountant(accountant)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        let before = SystemTime::now();

        subject_addr
            .try_send(first_expired_cores_package.clone())
            .unwrap();
        subject_addr
            .try_send(second_expired_cores_package.clone())
            .unwrap();

        System::current().stop();
        system.run();
        let after = SystemTime::now();
        let dispatcher_recording = dispatcher_log_arc.lock().unwrap();
        let record = dispatcher_recording.get_record::<TransmitDataMsg>(0);
        assert_eq!(record.endpoint, Endpoint::Socket(socket_addr));
        assert_eq!(record.last_data, false);
        assert_eq!(record.data, b"some data".to_vec());
        let record = dispatcher_recording.get_record::<TransmitDataMsg>(1);
        assert_eq!(record.endpoint, Endpoint::Socket(socket_addr));
        assert_eq!(record.last_data, false);
        assert_eq!(record.data, b"other data".to_vec());
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        let first_report = accountant_recording.get_record::<ReportServicesConsumedMessage>(0);
        let first_report_timestamp = first_report.timestamp;
        assert_eq!(
            first_report,
            &ReportServicesConsumedMessage {
                timestamp: first_report_timestamp,
                exit: ExitServiceConsumed {
                    earning_wallet: incoming_route_d_wallet.clone(),
                    payload_size: first_exit_size,
                    service_rate: rate_pack_d.exit_service_rate,
                    byte_rate: rate_pack_d.exit_byte_rate
                },
                routing_payload_size: routing_size,
                routing: vec![
                    RoutingServiceConsumed {
                        earning_wallet: incoming_route_e_wallet.clone(),
                        service_rate: rate_pack_e.routing_service_rate,
                        byte_rate: rate_pack_e.routing_byte_rate
                    },
                    RoutingServiceConsumed {
                        earning_wallet: incoming_route_f_wallet.clone(),
                        service_rate: rate_pack_f.routing_service_rate,
                        byte_rate: rate_pack_f.routing_byte_rate
                    }
                ]
            }
        );
        assert!(before <= first_report_timestamp && first_report_timestamp <= after);
        let second_report = accountant_recording.get_record::<ReportServicesConsumedMessage>(1);
        let second_report_timestamp = second_report.timestamp;
        let routing_size = second_expired_cores_package.payload_len;
        assert_eq!(
            second_report,
            &ReportServicesConsumedMessage {
                timestamp: second_report_timestamp,
                exit: ExitServiceConsumed {
                    earning_wallet: incoming_route_d_wallet,
                    payload_size: second_exit_size,
                    service_rate: rate_pack_d.exit_service_rate,
                    byte_rate: rate_pack_d.exit_byte_rate
                },
                routing_payload_size: routing_size,
                routing: vec![
                    RoutingServiceConsumed {
                        earning_wallet: incoming_route_e_wallet,
                        service_rate: rate_pack_e.routing_service_rate,
                        byte_rate: rate_pack_e.routing_byte_rate
                    },
                    RoutingServiceConsumed {
                        earning_wallet: incoming_route_f_wallet,
                        service_rate: rate_pack_f.routing_service_rate,
                        byte_rate: rate_pack_f.routing_byte_rate
                    }
                ]
            }
        );
        assert!(before <= second_report_timestamp && second_report_timestamp <= after);
        assert_eq!(accountant_recording.len(), 2);
    }

    #[test]
    fn dns_retry_entry_is_removed_after_a_successful_client_response() {
        init_test_logging();
        let test_name = "dns_retry_entry_is_removed_after_a_successful_client_response";
        let system = System::new(test_name);
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let stream_key_clone = stream_key.clone();
        let irrelevant_public_key = PublicKey::from(&b"irrelevant"[..]);
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        subject.logger = Logger::new(test_name);
        let mut dns_fail_client_payload = make_request_payload(111, cryptde);
        dns_fail_client_payload.stream_key = stream_key;
        let incoming_route_d_wallet = make_wallet("D Earning");
        let incoming_route_e_wallet = make_wallet("E Earning");
        let incoming_route_f_wallet = make_wallet("F Earning");
        let rate_pack_d = rate_pack(101);
        let rate_pack_e = rate_pack(102);
        let rate_pack_f = rate_pack(103);
        subject.stream_info.insert(
            stream_key_clone.clone(),
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: dns_fail_client_payload,
                    retries_left: 3,
                    active_attempt_id: 0,
                })
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![
                            ExpectedService::Exit(
                                irrelevant_public_key.clone(),
                                incoming_route_d_wallet.clone(),
                                rate_pack_d,
                            ),
                            ExpectedService::Routing(
                                irrelevant_public_key.clone(),
                                incoming_route_e_wallet.clone(),
                                rate_pack_e,
                            ),
                            ExpectedService::Routing(
                                irrelevant_public_key.clone(),
                                incoming_route_f_wallet.clone(),
                                rate_pack_f,
                            ),
                            ExpectedService::Nothing,
                        ],
                    ),
                    host: Host::new("booga.com", TLS_PORT),
                })
                .protocol(ProxyProtocol::TLS)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let first_client_response_payload = ClientResponsePayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket {
                data: b"some data".to_vec(),
                sequence_number: 0,
                last_data: false,
            },
        };
        let first_expired_cores_package: ExpiredCoresPackage<ClientResponsePayload_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                first_client_response_payload.into(),
                0,
            );
        let peer_actors = peer_actors_builder().build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(first_expired_cores_package).unwrap();

        subject_addr
            .try_send(AssertionsMessage {
                assertions: Box::new(move |proxy_server: &mut ProxyServer| {
                    let retry_opt = &proxy_server
                        .stream_info(&stream_key_clone)
                        .unwrap()
                        .dns_failure_retry_opt;
                    assert!(retry_opt.is_none());
                }),
            })
            .unwrap();
        System::current().stop();
        system.run();
    }

    #[test]
    fn proxy_server_records_services_consumed_even_after_browser_stream_is_gone() {
        let system =
            System::new("proxy_server_records_services_consumed_even_after_browser_stream_is_gone");
        let (dispatcher_mock, _, dispatcher_log_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();
        let irrelevant_public_key = PublicKey::from(&b"irrelevant"[..]);
        // subject.keys_and_addrs contains no browser stream
        let incoming_route_d_wallet = make_wallet("D Earning");
        let incoming_route_e_wallet = make_wallet("E Earning");
        let rate_pack_d = rate_pack(101);
        let rate_pack_e = rate_pack(102);
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![
                            ExpectedService::Exit(
                                irrelevant_public_key.clone(),
                                incoming_route_d_wallet.clone(),
                                rate_pack_d,
                            ),
                            ExpectedService::Routing(
                                irrelevant_public_key.clone(),
                                incoming_route_e_wallet.clone(),
                                rate_pack_e,
                            ),
                        ],
                    ),
                    host: Host::new("booga.com", TLS_PORT),
                })
                .protocol(ProxyProtocol::TLS)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let client_response_payload = ClientResponsePayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket {
                data: b"some data".to_vec(),
                sequence_number: 0,
                last_data: false,
            },
        };
        let exit_size = client_response_payload.sequenced_packet.data.len();
        let expired_cores_package: ExpiredCoresPackage<ClientResponsePayload_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                client_response_payload.into(),
                0,
            );
        let routing_size = expired_cores_package.payload_len;
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher_mock)
            .accountant(accountant)
            .build();
        let before = SystemTime::now();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr
            .try_send(expired_cores_package.clone())
            .unwrap();

        System::current().stop();
        system.run();
        let after = SystemTime::now();
        let dispatcher_recording = dispatcher_log_arc.lock().unwrap();
        assert_eq!(dispatcher_recording.len(), 0);
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        let services_consumed_report =
            accountant_recording.get_record::<ReportServicesConsumedMessage>(0);
        let returned_timestamp = services_consumed_report.timestamp;
        assert_eq!(
            services_consumed_report,
            &ReportServicesConsumedMessage {
                timestamp: returned_timestamp,
                exit: ExitServiceConsumed {
                    earning_wallet: incoming_route_d_wallet,
                    payload_size: exit_size,
                    service_rate: rate_pack_d.exit_service_rate,
                    byte_rate: rate_pack_d.exit_byte_rate
                },
                routing_payload_size: routing_size,
                routing: vec![RoutingServiceConsumed {
                    earning_wallet: incoming_route_e_wallet,
                    service_rate: rate_pack_e.routing_service_rate,
                    byte_rate: rate_pack_e.routing_byte_rate
                }]
            }
        );
        assert!(before <= returned_timestamp && returned_timestamp <= after);
        assert_eq!(accountant_recording.len(), 1);
    }

    #[test]
    fn handle_dns_resolve_failure_sends_message_to_dispatcher() {
        let system = System::new("proxy_server_receives_response_from_routing_services");
        let (dispatcher_mock, _, dispatcher_log_arc) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );

        let stream_key = StreamKey::make_meaningless_stream_key();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let client_payload = make_request_payload(111, cryptde);
        let exit_public_key = PublicKey::from(&b"exit_key"[..]);
        let exit_wallet = make_wallet("exit wallet");
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: client_payload,
                    retries_left: 0,
                    active_attempt_id: 0,
                })
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![ExpectedService::Exit(
                            exit_public_key.clone(),
                            exit_wallet,
                            rate_pack(10),
                        )],
                    ),
                    host: Host::new("server.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let dns_resolve_failure = DnsResolveFailure_0v1::new(stream_key);
        let expired_cores_package: ExpiredCoresPackage<DnsResolveFailure_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                dns_resolve_failure.into(),
                0,
            );

        let peer_actors = peer_actors_builder().dispatcher(dispatcher_mock).build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        subject_addr.try_send(expired_cores_package).unwrap();

        System::current().stop();
        system.run();

        let dispatcher_recording = dispatcher_log_arc.lock().unwrap();
        let record = dispatcher_recording.get_record::<TransmitDataMsg>(0);
        assert_eq!(
            TransmitDataMsg {
                endpoint: Endpoint::Socket(socket_addr),
                last_data: true,
                sequence_number_opt: Some(0),
                data: ServerImpersonatorHttp {}
                    .dns_resolution_failure_response("server.com".to_string()),
            },
            *record
        );
    }

    #[test]
    fn handle_dns_resolve_failure_reports_services_consumed() {
        let system = System::new("proxy_server_records_accounting");
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let irrelevant_public_key = PublicKey::from(&b"irrelevant"[..]);
        let client_payload = make_request_payload(111, cryptde);
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        let incoming_route_d_wallet = make_wallet("D Earning");
        let incoming_route_e_wallet = make_wallet("E Earning");
        let incoming_route_f_wallet = make_wallet("F Earning");
        let rate_pack_d = rate_pack(101);
        let rate_pack_e = rate_pack(102);
        let rate_pack_f = rate_pack(103);
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: client_payload,
                    retries_left: 0,
                    active_attempt_id: 0,
                })
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![
                            ExpectedService::Exit(
                                irrelevant_public_key.clone(),
                                incoming_route_d_wallet.clone(),
                                rate_pack_d,
                            ),
                            ExpectedService::Routing(
                                irrelevant_public_key.clone(),
                                incoming_route_e_wallet.clone(),
                                rate_pack_e,
                            ),
                            ExpectedService::Routing(
                                irrelevant_public_key.clone(),
                                incoming_route_f_wallet.clone(),
                                rate_pack_f,
                            ),
                            ExpectedService::Nothing,
                        ],
                    ),
                    host: Host::new("booga.com", TLS_PORT),
                })
                .protocol(ProxyProtocol::TLS)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let dns_resolve_failure_payload = DnsResolveFailure_0v1::new(stream_key);
        let expired_cores_package: ExpiredCoresPackage<DnsResolveFailure_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                dns_resolve_failure_payload.into(),
                0,
            );
        let routing_size = expired_cores_package.payload_len;
        let peer_actors = peer_actors_builder().accountant(accountant).build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        let before = SystemTime::now();

        subject_addr
            .try_send(expired_cores_package.clone())
            .unwrap();

        System::current().stop();
        system.run();
        let after = SystemTime::now();
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        let services_consumed_message =
            accountant_recording.get_record::<ReportServicesConsumedMessage>(0);
        let returned_timestamp = services_consumed_message.timestamp;
        assert_eq!(
            services_consumed_message,
            &ReportServicesConsumedMessage {
                timestamp: returned_timestamp,
                exit: ExitServiceConsumed {
                    earning_wallet: incoming_route_d_wallet,
                    payload_size: 0,
                    service_rate: rate_pack_d.exit_service_rate,
                    byte_rate: rate_pack_d.exit_byte_rate
                },
                routing_payload_size: routing_size,
                routing: vec![
                    RoutingServiceConsumed {
                        earning_wallet: incoming_route_e_wallet,
                        service_rate: rate_pack_e.routing_service_rate,
                        byte_rate: rate_pack_e.routing_byte_rate
                    },
                    RoutingServiceConsumed {
                        earning_wallet: incoming_route_f_wallet,
                        service_rate: rate_pack_f.routing_service_rate,
                        byte_rate: rate_pack_f.routing_byte_rate
                    }
                ]
            }
        );
        assert!(before <= returned_timestamp && returned_timestamp <= after);
        assert_eq!(accountant_recording.len(), 1);
    }

    #[test]
    fn handle_dns_resolve_failure_sends_message_to_neighborhood() {
        init_test_logging();
        let test_name = "handle_dns_resolve_failure_sends_message_to_neighborhood";
        let system = System::new(test_name);
        let (neighborhood_mock, _, neighborhood_log_arc) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let client_payload = make_request_payload(111, cryptde);
        subject.logger = Logger::new(test_name);
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        let exit_public_key = PublicKey::from(&b"exit_key"[..]);
        let exit_wallet = make_wallet("exit wallet");
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: client_payload,
                    retries_left: 0,
                    active_attempt_id: 0,
                })
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![ExpectedService::Exit(
                            exit_public_key.clone(),
                            exit_wallet.clone(),
                            rate_pack(10),
                        )],
                    ),
                    host: Host::new("server.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let dns_resolve_failure = DnsResolveFailure_0v1::new(stream_key);
        let expired_cores_package: ExpiredCoresPackage<DnsResolveFailure_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                dns_resolve_failure.into(),
                0,
            );
        let peer_actors = peer_actors_builder()
            .neighborhood(neighborhood_mock)
            .build();

        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        subject_addr.try_send(expired_cores_package).unwrap();

        System::current().stop();
        system.run();
        let neighborhood_recording = neighborhood_log_arc.lock().unwrap();
        let record = neighborhood_recording.get_record::<UpdateNodeRecordMetadataMessage>(0);
        assert_eq!(
            record,
            &UpdateNodeRecordMetadataMessage {
                public_key: exit_public_key.clone(),
                metadata_change: NRMetadataChange::AddUnreachableHost {
                    hostname: "server.com".to_string()
                }
            }
        );
        TestLogHandler::new().exists_no_log_containing(&format!(
            "ERROR: {test_name}: Exit node {exit_public_key} complained of DNS failure, but was given no hostname to resolve."
        ));
    }

    #[test]
    fn handle_dns_resolve_failure_logs_when_stream_key_is_found_in_stream_info_but_not_keys_and_addrs(
    ) {
        init_test_logging();
        let system = System::new("test");
        let (neighborhood_mock, _, _) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let client_payload = make_request_payload(111, cryptde);
        let exit_public_key = PublicKey::from(&b"exit_key"[..]);
        let exit_wallet = make_wallet("exit wallet");
        subject.stream_info.insert(
            stream_key,
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: client_payload,
                    retries_left: 0,
                    active_attempt_id: 0,
                })
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![ExpectedService::Exit(
                            exit_public_key.clone(),
                            exit_wallet.clone(),
                            rate_pack(10),
                        )],
                    ),
                    host: Host::new("server.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let dns_resolve_failure = DnsResolveFailure_0v1::new(stream_key);
        let expired_cores_package: ExpiredCoresPackage<DnsResolveFailure_0v1> =
            ExpiredCoresPackage::new(
                socket_addr,
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                dns_resolve_failure.into(),
                0,
            );
        let already_used_expired_cores_package = expired_cores_package.clone();
        let peer_actors = peer_actors_builder()
            .neighborhood(neighborhood_mock)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(expired_cores_package).unwrap();
        subject_addr
            .try_send(already_used_expired_cores_package)
            .unwrap();

        System::current().stop();
        system.run();
        TestLogHandler::new().exists_log_containing(
            "Discarding DnsResolveFailure message because destination and stream correlation are unrecognized",
        );
    }

    #[test]
    fn handle_dns_resolve_failure_logs_when_stream_key_and_server_name_are_both_missing() {
        init_test_logging();
        let system = System::new("test");
        let (neighborhood_mock, _, _) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let client_payload = make_request_payload(111, cryptde);
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        let exit_public_key = PublicKey::from(&b"exit_key"[..]);
        let exit_wallet = make_wallet("exit wallet");
        subject.stream_info.insert(
            stream_key,
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: client_payload,
                    retries_left: 0,
                    active_attempt_id: 0,
                })
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![ExpectedService::Exit(
                            exit_public_key.clone(),
                            exit_wallet.clone(),
                            rate_pack(10),
                        )],
                    ),
                    host: Host::new("booga.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let dns_resolve_failure = DnsResolveFailure_0v1::new(stream_key);
        let expired_cores_package: ExpiredCoresPackage<DnsResolveFailure_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                dns_resolve_failure.into(),
                0,
            );
        let already_used_expired_cores_package = expired_cores_package.clone();
        let peer_actors = peer_actors_builder()
            .neighborhood(neighborhood_mock)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(expired_cores_package).unwrap();
        subject_addr
            .try_send(already_used_expired_cores_package)
            .unwrap();

        System::current().stop();
        system.run();
        TestLogHandler::new().exists_log_containing(&format!(
            "Discarding DnsResolveFailure message from an unrecognized stream key {:?}",
            stream_key
        ));
    }

    #[test]
    fn handle_dns_resolve_failure_without_route_preserves_stream_state() {
        init_test_logging();
        let test_name = "handle_dns_resolve_failure_without_route_preserves_stream_state";
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        let stream_key = StreamKey::make_meaningful_stream_key(test_name);
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr);
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: make_request_payload(111, cryptde),
                    retries_left: DNS_FAILURE_RETRIES,
                    active_attempt_id: 0,
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let expired_cores_package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            Some(make_wallet("irrelevant")),
            return_route(cryptde),
            DnsResolveFailure_0v1::new(stream_key).into(),
            123,
        );

        subject.handle_dns_resolve_failure(&expired_cores_package);

        assert!(subject.stream_info.contains_key(&stream_key));
        assert_eq!(
            subject.keys_and_addrs.a_to_b(&stream_key),
            Some(socket_addr)
        );
        TestLogHandler::new().exists_log_containing(&format!(
            "ERROR: {test_name}: Discarding DnsResolveFailure message for stream key {stream_key} because it has no route info"
        ));
    }

    #[test]
    fn handle_dns_resolve_failure_without_retry_context_is_side_effect_free_and_preserves_stream_state(
    ) {
        init_test_logging();
        let test_name = "handle_dns_resolve_failure_without_retry_context_is_side_effect_free_and_preserves_stream_state";
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        let stream_key = StreamKey::make_meaningful_stream_key(test_name);
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr);
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![make_exit_service_from_key(PublicKey::new(b"exit_node"))],
                    ),
                    host: Host::new("booga.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let expired_cores_package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            Some(make_wallet("irrelevant")),
            return_route(cryptde),
            DnsResolveFailure_0v1::new(stream_key).into(),
            999,
        );

        subject.handle_dns_resolve_failure(&expired_cores_package);

        assert!(subject.stream_info.contains_key(&stream_key));
        assert_eq!(
            subject.keys_and_addrs.a_to_b(&stream_key),
            Some(socket_addr)
        );
        TestLogHandler::new().exists_log_containing(&format!(
            "ERROR: {test_name}: Discarding DnsResolveFailure message for stream key {stream_key} because it has no DNS failure retry context"
        ));
    }

    #[test]
    fn handle_dns_resolve_failure_purges_stream_keys() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let (neighborhood_mock, _, _) = make_recorder();
        let (dispatcher_mock, _, _) = make_recorder();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.subs = Some(make_proxy_server_out_subs());
        let peer_actors = peer_actors_builder()
            .neighborhood(neighborhood_mock)
            .dispatcher(dispatcher_mock)
            .build();
        subject.subs.as_mut().unwrap().update_node_record_metadata =
            peer_actors.neighborhood.update_node_record_metadata;
        subject.subs.as_mut().unwrap().dispatcher = peer_actors.dispatcher.from_dispatcher_client;
        let stream_key = StreamKey::make_meaningless_stream_key();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let client_payload = make_request_payload(111, cryptde);
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        subject.stream_info.insert(
            stream_key,
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: client_payload,
                    retries_left: 0,
                    active_attempt_id: 0,
                })
                .tunneled_host("tunneled host")
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![
                            make_exit_service_from_key(PublicKey::new(b"exit_node")),
                            ExpectedService::Nothing,
                        ],
                    ),
                    host: Host::new("booga.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let dns_resolve_failure = DnsResolveFailure_0v1::new(stream_key);
        let expired_cores_package: ExpiredCoresPackage<DnsResolveFailure_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                dns_resolve_failure.into(),
                0,
            );

        subject.handle_dns_resolve_failure(&expired_cores_package);

        assert!(subject.keys_and_addrs.is_empty());
        assert!(subject.stream_info.get(&stream_key).is_none());
    }

    #[test]
    fn handle_dns_resolve_failure_zero_hop() {
        let system = System::new("handle_dns_resolve_failure_zero_hop");
        let (dispatcher_mock, _, dispatcher_recording_arc) = make_recorder();
        let (neighborhood_mock, _, neighborhood_recording_arc) = make_recorder();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let this_node_public_key = cryptde.public_key();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            false, //meaning ZeroHop
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let client_payload = make_request_payload(111, cryptde);
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: client_payload,
                    retries_left: 0,
                    active_attempt_id: 0,
                })
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![ExpectedService::Nothing, ExpectedService::Nothing],
                    ),
                    host: Host::new("server.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let dns_resolve_failure = DnsResolveFailure_0v1::new(stream_key);
        let expired_cores_package: ExpiredCoresPackage<DnsResolveFailure_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                dns_resolve_failure.into(),
                0,
            );
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher_mock)
            .neighborhood(neighborhood_mock)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(expired_cores_package).unwrap();

        System::current().stop();
        system.run();
        let neighborhood_recording = neighborhood_recording_arc.lock().unwrap();
        let msg = neighborhood_recording.get_record::<UpdateNodeRecordMetadataMessage>(0);
        assert_eq!(
            msg,
            &UpdateNodeRecordMetadataMessage {
                public_key: this_node_public_key.clone(),
                metadata_change: NRMetadataChange::AddUnreachableHost {
                    hostname: "server.com".to_string()
                }
            }
        );
        let dispatcher_recording = dispatcher_recording_arc.lock().unwrap();
        let record = dispatcher_recording.get_record::<TransmitDataMsg>(0);
        assert_eq!(
            TransmitDataMsg {
                endpoint: Endpoint::Socket(socket_addr),
                last_data: true,
                sequence_number_opt: Some(0),
                data: ServerImpersonatorHttp {}
                    .dns_resolution_failure_response("server.com".to_string()),
            },
            *record
        );
    }

    #[test]
    fn handle_dns_resolve_failure_sent_request_retry() {
        let test_name = "handle_dns_resolve_failure_sent_request_retry";
        let system = System::new(test_name);
        let resolve_message_params_arc = Arc::new(Mutex::new(vec![]));
        let (neighborhood_mock, _, _) = make_recorder();
        let exit_public_key = PublicKey::from(&b"exit_key"[..]);
        let exit_wallet = make_wallet("exit wallet");
        let expected_services = vec![ExpectedService::Exit(
            exit_public_key.clone(),
            exit_wallet,
            rate_pack(10),
        )];
        let route_query_response_expected = RouteQueryResponse {
            route: make_meaningless_route(&CRYPTDE_PAIR),
            expected_services: ExpectedServices::RoundTrip(
                expected_services.clone(),
                expected_services.clone(),
            ),
            host: Host::new("booga.com", HTTP_PORT),
        };
        let neighborhood_mock = neighborhood_mock
            .system_stop_conditions(match_lazily_every_type_id!(RouteQueryMessage))
            .route_query_response(Some(route_query_response_expected.clone()));
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let client_payload = make_request_payload(111, cryptde);
        let stream_key = client_payload.stream_key;
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: client_payload.clone(),
                    retries_left: 3,
                    active_attempt_id: 0,
                })
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        expected_services.clone(),
                    ),
                    host: Host::new("server.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let message_resolver = RouteQueryResponseResolverMock::default()
            .resolve_message_params(&resolve_message_params_arc);
        let message_resolver_factory = RouteQueryResponseResolverFactoryMock::default()
            .make_result(Box::new(message_resolver));
        subject.inbound_client_data_helper_opt = Some(Box::new(IBCDHelperReal {
            factory: Box::new(message_resolver_factory),
        }));
        let subject_addr: Addr<ProxyServer> = subject.start();
        let dns_resolve_failure = DnsResolveFailure_0v1::new(stream_key);
        let expired_cores_package: ExpiredCoresPackage<DnsResolveFailure_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                dns_resolve_failure.into(),
                0,
            );
        let peer_actors = peer_actors_builder()
            .neighborhood(neighborhood_mock)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(expired_cores_package).unwrap();

        subject_addr
            .try_send(AssertionsMessage {
                assertions: Box::new(move |proxy_server: &mut ProxyServer| {
                    let retry = proxy_server
                        .stream_info(&stream_key)
                        .unwrap()
                        .dns_failure_retry_opt
                        .as_ref()
                        .unwrap();
                    assert_eq!(retry.retries_left, 2);
                    assert_eq!(retry.active_attempt_id, 1);
                }),
            })
            .unwrap();
        let before = SystemTime::now();
        system.run();
        let after = SystemTime::now();
        let mut resolve_message_params = resolve_message_params_arc.lock().unwrap();
        let (transmit_to_hopper_args, route_query_message_response) =
            resolve_message_params.remove(0);
        let args = transmit_to_hopper_args;
        assert!(resolve_message_params.is_empty());
        let mut expected_retry_payload = client_payload;
        expected_retry_payload.dns_attempt_id_opt = Some(1);
        assert_eq!(args.payload, expected_retry_payload);
        assert_eq!(args.client_addr, socket_addr);
        assert!(before <= args.timestamp && args.timestamp <= after);
        assert!(args.retire_stream_key_sub_opt.is_none());
        assert_eq!(args.is_decentralized, true);
        assert_eq!(
            route_query_message_response.unwrap().unwrap(),
            route_query_response_expected
        );
    }

    #[test]
    fn handle_dns_resolve_failure_logs_error_when_there_is_no_dns_failure_retry_entry_for_the_stream_key(
    ) {
        init_test_logging();
        let test_name = "handle_dns_resolve_failure_logs_error_when_there_is_no_dns_failure_retry_entry_for_the_stream_key";
        let system = System::new(test_name);
        let exit_public_key = PublicKey::from(&b"exit_key"[..]);
        let exit_wallet = make_wallet("exit wallet");
        let expected_services = vec![ExpectedService::Exit(
            exit_public_key.clone(),
            exit_wallet,
            rate_pack(10),
        )];
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        subject.logger = Logger::new(test_name);
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        expected_services.clone(),
                    ),
                    host: Host::new("server.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();
        let dns_resolve_failure = DnsResolveFailure_0v1::new(stream_key);
        let expired_cores_package: ExpiredCoresPackage<DnsResolveFailure_0v1> =
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                dns_resolve_failure.into(),
                0,
            );
        let peer_actors = peer_actors_builder().build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(expired_cores_package).unwrap();

        System::current().stop();
        system.run();
        TestLogHandler::new().exists_log_containing(&format!(
            "ERROR: {test_name}: Discarding DnsResolveFailure message for stream key {stream_key} because it has no DNS failure retry context"
        ));
    }

    #[test]
    fn handle_dns_resolve_failure_retries_three_times_and_ignores_a_duplicate_attempt() {
        init_test_logging();
        let test_name =
            "handle_dns_resolve_failure_retries_three_times_and_ignores_a_duplicate_attempt";
        let make_params_arc = Arc::new(Mutex::new(vec![]));
        let system = System::new(test_name);
        let (neighborhood_mock, _, _) = make_recorder();
        let exit_public_key = PublicKey::from(&b"exit_key"[..]);
        let exit_wallet = make_wallet("exit wallet");
        let expected_services = vec![ExpectedService::Exit(
            exit_public_key.clone(),
            exit_wallet,
            rate_pack(10),
        )];
        let route_query_response_expected = RouteQueryResponse {
            route: make_meaningless_route(&CRYPTDE_PAIR),
            expected_services: ExpectedServices::RoundTrip(
                expected_services.clone(),
                expected_services.clone(),
            ),
            host: Host::new("booga.com", HTTP_PORT),
        };
        let neighborhood_mock = neighborhood_mock
            .system_stop_conditions(match_lazily_every_type_id!(
                RouteQueryMessage,
                RouteQueryMessage,
                RouteQueryMessage
            ))
            .route_query_response(Some(route_query_response_expected.clone()));
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let client_payload = make_request_payload(111, cryptde);
        let stream_key = client_payload.stream_key;
        let stream_key_clone = stream_key.clone();
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        subject.stream_info.insert(
            stream_key_clone.clone(),
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: client_payload,
                    retries_left: 3,
                    active_attempt_id: 0,
                })
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        expected_services.clone(),
                    ),
                    host: Host::new("server.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let message_resolver_factory = RouteQueryResponseResolverFactoryMock::default()
            .make_params(&make_params_arc)
            .make_result(Box::new(RouteQueryResponseResolverMock::default()))
            .make_result(Box::new(RouteQueryResponseResolverMock::default()))
            .make_result(Box::new(RouteQueryResponseResolverMock::default()))
            .make_result(Box::new(RouteQueryResponseResolverMock::default()));
        subject.inbound_client_data_helper_opt = Some(Box::new(IBCDHelperReal {
            factory: Box::new(message_resolver_factory),
        }));
        let subject_addr: Addr<ProxyServer> = subject.start();
        let make_failure_package = |attempt_id| -> ExpiredCoresPackage<DnsResolveFailure_0v1> {
            ExpiredCoresPackage::new(
                SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                Some(make_wallet("irrelevant")),
                return_route(cryptde),
                DnsResolveFailure_0v1::for_attempt(stream_key, Some(attempt_id)).into(),
                0,
            )
        };
        let peer_actors = peer_actors_builder()
            .neighborhood(neighborhood_mock)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(make_failure_package(0)).unwrap();
        subject_addr
            .try_send(AddRouteResultMessage {
                stream_key,
                route_request_id: 1,
                result: Ok(route_query_response_expected.clone()),
            })
            .unwrap();
        subject_addr.try_send(make_failure_package(0)).unwrap();
        subject_addr.try_send(make_failure_package(1)).unwrap();
        subject_addr
            .try_send(AddRouteResultMessage {
                stream_key,
                route_request_id: 2,
                result: Ok(route_query_response_expected.clone()),
            })
            .unwrap();
        subject_addr.try_send(make_failure_package(2)).unwrap();
        subject_addr
            .try_send(AddRouteResultMessage {
                stream_key,
                route_request_id: 3,
                result: Ok(route_query_response_expected),
            })
            .unwrap();
        subject_addr.try_send(make_failure_package(3)).unwrap();

        subject_addr
            .try_send(AssertionsMessage {
                assertions: Box::new(move |proxy_server: &mut ProxyServer| {
                    assert_eq!(proxy_server.keys_and_addrs.a_to_b(&stream_key), None);
                    assert_eq!(proxy_server.stream_info.get(&stream_key).is_none(), true);
                }),
            })
            .unwrap();
        system.run();
        let make_params = make_params_arc.lock().unwrap();
        assert_eq!(make_params.len(), 3);
        TestLogHandler::new().exists_log_containing(&format!(
            "DEBUG: {test_name}: Retiring stream key {stream_key_clone} due to DNS resolution failure"
        ));
        TestLogHandler::new().exists_log_containing(&format!(
            "WARN: {test_name}: Discarding stale DnsResolveFailure message for stream key {stream_key_clone}: expected attempt 1, received 0"
        ));
    }

    #[test]
    fn dns_failure_for_a_route_request_still_in_flight_is_side_effect_free() {
        init_test_logging();
        let test_name = "dns_failure_for_a_route_request_still_in_flight_is_side_effect_free";
        let system = System::new(test_name);
        let (dispatcher, _, dispatcher_recording) = make_recorder();
        let (accountant, _, accountant_recording) = make_recorder();
        let (neighborhood, _, neighborhood_recording) = make_recorder();
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher)
            .accountant(accountant)
            .neighborhood(neighborhood)
            .build();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut payload = make_request_payload(32, cryptde);
        payload.dns_attempt_id_opt = Some(1);
        let stream_key = payload.stream_key;
        let client_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let exit_public_key = PublicKey::from(&b"pending exit"[..]);
        let expected_services = vec![ExpectedService::Exit(
            exit_public_key,
            make_wallet("pending exit wallet"),
            rate_pack(10),
        )];
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.logger = Logger::new(test_name);
        subject.keys_and_addrs.insert(stream_key, client_addr);
        subject.stream_info.insert(
            stream_key,
            StreamInfoBuilder::new()
                .dns_failure_retry(DNSFailureRetry {
                    unsuccessful_request: payload.clone(),
                    retries_left: 2,
                    active_attempt_id: 1,
                })
                .route(RouteQueryResponse {
                    route: make_meaningless_route(&CRYPTDE_PAIR),
                    expected_services: ExpectedServices::RoundTrip(
                        expected_services.clone(),
                        expected_services,
                    ),
                    host: Host::new("example.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let args = TransmitToHopperArgs {
            main_cryptde: cryptde.dup(),
            payload,
            return_route_id: 1,
            route_request_id: 0,
            client_addr,
            timestamp: SystemTime::now(),
            is_decentralized: true,
            require_service_receipt_capability: false,
            logger: Logger::new(test_name),
            hopper_sub: peer_actors.hopper.from_hopper_client.clone(),
            dispatcher_sub: peer_actors.dispatcher.from_dispatcher_client.clone(),
            accountant_sub: peer_actors.accountant.report_services_consumed.clone(),
            record_exit_request_for_receipt: peer_actors
                .proxy_server
                .record_exit_request_for_receipt
                .clone(),
            retire_stream_key_sub_opt: None,
        };
        assert_eq!(
            subject
                .queue_pending_route_packet(args)
                .unwrap()
                .unwrap()
                .route_request_id,
            1
        );
        let subject_addr = subject.start();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        let failure_package: ExpiredCoresPackage<DnsResolveFailure_0v1> = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            Some(make_wallet("irrelevant")),
            return_route(cryptde),
            DnsResolveFailure_0v1::for_attempt(stream_key, Some(1)).into(),
            0,
        );
        subject_addr.try_send(failure_package).unwrap();
        subject_addr
            .try_send(AssertionsMessage {
                assertions: Box::new(move |proxy_server: &mut ProxyServer| {
                    assert_eq!(
                        proxy_server
                            .pending_route_requests
                            .get(&stream_key)
                            .unwrap()
                            .route_request_id,
                        1
                    );
                    assert_eq!(
                        proxy_server
                            .stream_info
                            .get(&stream_key)
                            .unwrap()
                            .dns_failure_retry_opt
                            .as_ref()
                            .unwrap()
                            .retries_left,
                        2
                    );
                    System::current().stop();
                }),
            })
            .unwrap();

        system.run();
        assert!(dispatcher_recording.lock().unwrap().is_empty());
        assert!(accountant_recording.lock().unwrap().is_empty());
        assert!(neighborhood_recording.lock().unwrap().is_empty());
        TestLogHandler::new()
            .exists_log_containing("Discarding premature DnsResolveFailure message for stream key");
    }

    #[test]
    #[should_panic(expected = "Dispatcher unbound in ProxyServer")]
    fn panics_if_dispatcher_is_unbound() {
        let system = System::new("panics_if_dispatcher_is_unbound");
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject
            .keys_and_addrs
            .insert(stream_key.clone(), socket_addr.clone());
        let remaining_route = return_route(cryptde);
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(
                        vec![],
                        vec![ExpectedService::Nothing],
                    ),
                    host: Host::new("booga.com", HTTP_PORT),
                })
                .protocol(ProxyProtocol::HTTP)
                .build(),
        );
        let subject_addr: Addr<ProxyServer> = subject.start();

        let client_response_payload = ClientResponsePayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket {
                data: b"data".to_vec(),
                sequence_number: 0,
                last_data: true,
            },
        };
        let expired_cores_package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            Some(make_wallet("consuming")),
            remaining_route,
            client_response_payload,
            0,
        );

        subject_addr.try_send(expired_cores_package).unwrap();

        System::current().stop();
        system.run();
    }

    #[test]
    #[should_panic(expected = "Hopper unbound in ProxyServer")]
    fn panics_if_hopper_is_unbound() {
        let system = System::new("panics_if_hopper_is_unbound");
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(80),
            sequence_number_opt: Some(0),
            last_data: false,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let subject_addr: Addr<ProxyServer> = subject.start();

        subject_addr.try_send(msg_from_dispatcher).unwrap();

        System::current().stop();
        system.run();
    }

    #[test]
    fn report_response_services_consumed_complains_and_drops_package_if_return_route_id_is_unrecognized(
    ) {
        init_test_logging();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let (dispatcher, _, dispatcher_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let system = System::new("report_response_services_consumed_complains_and_drops_package_if_return_route_id_is_unrecognized");
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();
        subject
            .keys_and_addrs
            .insert(stream_key, SocketAddr::from_str("1.2.3.4:5678").unwrap());
        let subject_addr: Addr<ProxyServer> = subject.start();
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher)
            .accountant(accountant)
            .build();
        let client_response_payload = ClientResponsePayload_0v1 {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket {
                data: b"some data".to_vec(),
                sequence_number: 4321,
                last_data: false,
            },
        };
        let expired_cores_package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            Some(make_wallet("irrelevant")),
            return_route(cryptde),
            client_response_payload,
            0,
        );
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(expired_cores_package).unwrap();

        System::current().stop();
        system.run();
        TestLogHandler::new().exists_log_containing(format!("ERROR: ProxyServer: Can't pay for return services consumed: received response with unrecognized stream key {}. Ignoring", stream_key).as_str());
        assert_eq!(dispatcher_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(accountant_recording_arc.lock().unwrap().len(), 0);
    }

    #[test]
    fn handle_stream_shutdown_msg_handles_unknown_peer_addr() {
        init_test_logging();
        let test_name = "handle_stream_shutdown_msg_handles_unknown_peer_addr";
        let mut subject = ProxyServer::new(CRYPTDE_PAIR.clone(), true, None, false, false);
        subject.logger = Logger::new(test_name);
        let unaffected_socket_addr = SocketAddr::from_str("2.3.4.5:6789").unwrap();
        let unaffected_stream_key = StreamKey::make_meaningful_stream_key("unaffected");
        subject
            .keys_and_addrs
            .insert(unaffected_stream_key, unaffected_socket_addr);
        subject.stream_info.insert(
            unaffected_stream_key,
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(vec![], vec![]),
                    host: Host::new("booga.com", HTTP_PORT),
                })
                .tunneled_host("blah")
                .build(),
        );

        subject.handle_stream_shutdown_msg(StreamShutdownMsg {
            peer_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            stream_type: RemovedStreamType::NonClandestine(NonClandestineAttributes {
                reception_port: HTTP_PORT,
                sequence_number: 1234,
            }),
            report_to_counterpart: true,
        });

        // Subject is unbound but didn't panic; therefore, no attempt to send to Hopper: perfect!
        assert!(subject
            .keys_and_addrs
            .a_to_b(&unaffected_stream_key)
            .is_some());
        assert!(subject.stream_info.contains_key(&unaffected_stream_key));
        assert!(subject
            .stream_info(&unaffected_stream_key)
            .unwrap()
            .tunneled_host_opt
            .is_some());
        let logs = TestLogHandler::new();
        logs.exists_log_containing(&format!(
            "WARN: {test_name}: Received instruction to shut down nonexistent stream; peer redacted - ignoring"
        ));
        logs.exists_no_log_containing(&format!(
            "WARN: {test_name}: Received instruction to shut down nonexistent stream to peer 1.2.3.4:5678"
        ));
    }

    #[test]
    fn handle_stream_shutdown_msg_reports_to_counterpart_through_tunnel_when_necessary() {
        let system = System::new("test");
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let unaffected_socket_addr = SocketAddr::from_str("2.3.4.5:6789").unwrap();
        let unaffected_stream_key = StreamKey::make_meaningful_stream_key("unaffected");
        let affected_socket_addr = SocketAddr::from_str("3.4.5.6:7890").unwrap();
        let affected_stream_key = StreamKey::make_meaningful_stream_key("affected");
        let affected_cryptde = CryptDENull::from(&PublicKey::new(b"affected"), TEST_DEFAULT_CHAIN);
        subject
            .keys_and_addrs
            .insert(unaffected_stream_key, unaffected_socket_addr);
        subject
            .keys_and_addrs
            .insert(affected_stream_key, affected_socket_addr);
        subject.stream_info.insert(
            unaffected_stream_key,
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(vec![], vec![]),
                    host: Host::new("gooba.com", HTTP_PORT),
                })
                .tunneled_host("blah")
                .build(),
        );
        let affected_route = Route::round_trip(
            RouteSegment::new(
                vec![
                    CRYPTDE_PAIR.main.as_ref().public_key(),
                    affected_cryptde.public_key(),
                ],
                Component::ProxyClient,
            ),
            RouteSegment::new(
                vec![
                    affected_cryptde.public_key(),
                    CRYPTDE_PAIR.main.as_ref().public_key(),
                ],
                Component::ProxyServer,
            ),
            CRYPTDE_PAIR.main.as_ref(),
            Some(make_paying_wallet(b"consuming")),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
        )
        .unwrap();
        let affected_expected_services = vec![ExpectedService::Exit(
            affected_cryptde.public_key().clone(),
            make_paying_wallet(b"1234"),
            DEFAULT_RATE_PACK,
        )];
        subject.stream_info.insert(
            affected_stream_key,
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: affected_route.clone(),
                    expected_services: ExpectedServices::RoundTrip(
                        affected_expected_services,
                        vec![],
                    ),
                    host: Host::new("gooba.com", TLS_PORT),
                })
                .tunneled_host("tunneled.com")
                .build(),
        );
        let subject_addr = subject.start();
        let (hopper, _, hopper_recording_arc) = make_recorder();
        let (proxy_server, _, proxy_server_recording_arc) = make_recorder();
        let peer_actors = peer_actors_builder()
            .hopper(hopper)
            .proxy_server(proxy_server)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr
            .try_send(StreamShutdownMsg {
                peer_addr: affected_socket_addr,
                stream_type: RemovedStreamType::NonClandestine(NonClandestineAttributes {
                    reception_port: TLS_PORT,
                    sequence_number: 1234,
                }),
                report_to_counterpart: true,
            })
            .unwrap();

        System::current().stop();
        system.run();
        let recording = hopper_recording_arc.lock().unwrap();
        let record = recording.get_record::<IncipientCoresPackage>(0);
        assert_eq!(record.route, affected_route);
        let payload = decodex::<MessageType>(&affected_cryptde, &record.payload).unwrap();
        match payload {
            MessageType::ClientRequest(vd) => assert_eq!(
                vd.extract(&crate::sub_lib::migrations::client_request_payload::MIGRATIONS)
                    .unwrap(),
                ClientRequestPayload_0v1 {
                    stream_key: affected_stream_key,
                    sequenced_packet: SequencedPacket::new(vec![], 1234, true),
                    target_hostname: String::from("tunneled.com"),
                    target_port: TLS_PORT,
                    protocol: ProxyProtocol::TLS,
                    originator_public_key: CRYPTDE_PAIR.alias.as_ref().public_key().clone(),
                    dns_attempt_id_opt: Some(0),
                    receipt_session_request_opt: None,
                }
            ),
            other => panic!("Wrong payload type: {:?}", other),
        }
        let recording = proxy_server_recording_arc.lock().unwrap();
        let record = recording.get_record::<StreamShutdownMsg>(recording.len() - 1);
        assert_eq!(
            record,
            &StreamShutdownMsg {
                peer_addr: affected_socket_addr,
                stream_type: RemovedStreamType::NonClandestine(NonClandestineAttributes {
                    reception_port: 0,
                    sequence_number: 0
                }),
                report_to_counterpart: false
            }
        );
    }

    #[test]
    fn handle_stream_shutdown_msg_reports_to_counterpart_without_tunnel_when_necessary() {
        init_test_logging();
        let test_name =
            "handle_stream_shutdown_msg_reports_to_counterpart_without_tunnel_when_necessary";
        let system = System::new(test_name);
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        let unaffected_socket_addr = SocketAddr::from_str("2.3.4.5:6789").unwrap();
        let unaffected_stream_key = StreamKey::make_meaningful_stream_key("unaffected");
        let affected_socket_addr = SocketAddr::from_str("3.4.5.6:7890").unwrap();
        let affected_stream_key = StreamKey::make_meaningful_stream_key("affected");
        let affected_cryptde = CryptDENull::from(&PublicKey::new(b"affected"), TEST_DEFAULT_CHAIN);
        subject
            .keys_and_addrs
            .insert(unaffected_stream_key, unaffected_socket_addr);
        subject
            .keys_and_addrs
            .insert(affected_stream_key, affected_socket_addr);
        subject.stream_info.insert(
            unaffected_stream_key,
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(vec![], vec![]),
                    host: Host::new("booga.com", HTTP_PORT),
                })
                .build(),
        );
        subject.next_return_route_id = Cell::new(1234);
        let affected_route = Route::round_trip(
            RouteSegment::new(
                vec![
                    CRYPTDE_PAIR.main.as_ref().public_key(),
                    affected_cryptde.public_key(),
                ],
                Component::ProxyClient,
            ),
            RouteSegment::new(
                vec![
                    affected_cryptde.public_key(),
                    CRYPTDE_PAIR.main.as_ref().public_key(),
                ],
                Component::ProxyServer,
            ),
            CRYPTDE_PAIR.main.as_ref(),
            Some(make_paying_wallet(b"consuming")),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
        )
        .unwrap();
        let affected_expected_services = vec![ExpectedService::Exit(
            affected_cryptde.public_key().clone(),
            make_paying_wallet(b"1234"),
            DEFAULT_RATE_PACK,
        )];
        subject.stream_info.insert(
            affected_stream_key,
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: affected_route.clone(),
                    expected_services: ExpectedServices::RoundTrip(
                        affected_expected_services,
                        vec![],
                    ),
                    host: Host::new("booga.com", HTTP_PORT),
                })
                .build(),
        );
        subject.logger = Logger::new(test_name);
        let subject_addr = subject.start();
        let (hopper, _, hopper_recording_arc) = make_recorder();
        let (proxy_server, _, proxy_server_recording_arc) = make_recorder();
        let peer_actors = peer_actors_builder()
            .hopper(hopper)
            .proxy_server(proxy_server)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr
            .try_send(StreamShutdownMsg {
                peer_addr: affected_socket_addr,
                stream_type: RemovedStreamType::NonClandestine(NonClandestineAttributes {
                    reception_port: HTTP_PORT,
                    sequence_number: 1234,
                }),
                report_to_counterpart: true,
            })
            .unwrap();

        System::current().stop();
        system.run();
        let recording = hopper_recording_arc.lock().unwrap();
        let record = recording.get_record::<IncipientCoresPackage>(0);
        assert_eq!(record.route, affected_route);
        let payload = decodex::<MessageType>(&affected_cryptde, &record.payload).unwrap();
        match payload {
            MessageType::ClientRequest(vd) => assert_eq!(
                vd.extract(&crate::sub_lib::migrations::client_request_payload::MIGRATIONS)
                    .unwrap(),
                ClientRequestPayload_0v1 {
                    stream_key: affected_stream_key,
                    sequenced_packet: SequencedPacket::new(vec![], 1234, true),
                    target_hostname: "booga.com".to_string(),
                    target_port: HTTP_PORT,
                    protocol: ProxyProtocol::HTTP,
                    originator_public_key: CRYPTDE_PAIR.alias.as_ref().public_key().clone(),
                    dns_attempt_id_opt: Some(0),
                    receipt_session_request_opt: None,
                }
            ),
            other => panic!("Wrong payload type: {:?}", other),
        }
        let recording = proxy_server_recording_arc.lock().unwrap();
        let record = recording.get_record::<StreamShutdownMsg>(recording.len() - 1);
        assert_eq!(
            record,
            &StreamShutdownMsg {
                peer_addr: affected_socket_addr,
                stream_type: RemovedStreamType::NonClandestine(NonClandestineAttributes {
                    reception_port: 0,
                    sequence_number: 0
                }),
                report_to_counterpart: false
            }
        );
        TestLogHandler::new().exists_log_containing(&format!(
            "DEBUG: {test_name}: Client closed a direct stream; destination and stream identifiers redacted. It will be purged after {:?}.",
            STREAM_KEY_PURGE_DELAY
        ));
    }

    #[test]
    fn handle_stream_shutdown_msg_logs_errors_from_handling_normal_client_data() {
        init_test_logging();
        let mut subject = ProxyServer::new(CRYPTDE_PAIR.clone(), true, Some(0), false, false);
        subject.subs = Some(make_proxy_server_out_subs());
        let helper = IBCDHelperMock::default()
            .handle_normal_client_data_result(Err("Our help is not welcome".to_string()));
        subject.inbound_client_data_helper_opt = Some(Box::new(helper));
        let socket_addr = SocketAddr::from_str("3.4.5.6:7777").unwrap();
        let stream_key = StreamKey::make_meaningful_stream_key("All Things Must Pass");
        subject.keys_and_addrs.insert(stream_key, socket_addr);
        subject
            .stream_info
            .insert(stream_key.clone(), StreamInfoBuilder::new().build());
        let msg = StreamShutdownMsg {
            peer_addr: socket_addr,
            stream_type: RemovedStreamType::NonClandestine(NonClandestineAttributes {
                reception_port: HTTP_PORT,
                sequence_number: 1234,
            }),
            report_to_counterpart: true,
        };

        subject.handle_stream_shutdown_msg(msg);

        TestLogHandler::new().exists_log_containing("ERROR: ProxyServer: Our help is not welcome");
    }

    #[test]
    fn stream_shutdown_msg_populates_correct_inbound_client_data_msg() {
        let help_to_handle_normal_client_data_params_arc = Arc::new(Mutex::new(vec![]));
        let mut subject = ProxyServer::new(CRYPTDE_PAIR.clone(), true, Some(0), false, false);
        subject.subs = Some(make_proxy_server_out_subs());
        let icd_helper = IBCDHelperMock::default()
            .handle_normal_client_data_params(&help_to_handle_normal_client_data_params_arc)
            .handle_normal_client_data_result(Ok(()));
        subject.inbound_client_data_helper_opt = Some(Box::new(icd_helper));
        let socket_addr = SocketAddr::from_str("3.4.5.6:7890").unwrap();
        let stream_key = StreamKey::make_meaningful_stream_key("All Things Must Pass");
        subject.keys_and_addrs.insert(stream_key, socket_addr);
        subject.stream_info.insert(
            stream_key,
            StreamInfoBuilder::new()
                .route(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(vec![], vec![]),
                    host: Host::new("booga.com", HTTP_PORT),
                })
                .tunneled_host("blah")
                .build(),
        );
        let msg = StreamShutdownMsg {
            peer_addr: socket_addr,
            stream_type: RemovedStreamType::NonClandestine(NonClandestineAttributes {
                reception_port: HTTP_PORT,
                sequence_number: 1234,
            }),
            report_to_counterpart: true,
        };
        let before = SystemTime::now();

        subject.handle_stream_shutdown_msg(msg);

        let after = SystemTime::now();
        let handle_normal_client_data =
            help_to_handle_normal_client_data_params_arc.lock().unwrap();
        let inbound_client_data_msg = &handle_normal_client_data[0];
        assert_eq!(inbound_client_data_msg.client_addr, socket_addr);
        assert_eq!(inbound_client_data_msg.data, Vec::<u8>::new());
        assert_eq!(inbound_client_data_msg.last_data, true);
        assert_eq!(inbound_client_data_msg.is_clandestine, false);
        let actual_timestamp = inbound_client_data_msg.timestamp;
        assert!(before <= actual_timestamp && actual_timestamp <= after);
    }

    #[test]
    fn help_to_handle_normal_client_data_missing_consuming_wallet_and_protocol_pack_not_found() {
        let mut proxy_server = ProxyServer::new(CRYPTDE_PAIR.clone(), true, None, false, false);
        proxy_server.subs = Some(make_proxy_server_out_subs());
        let inbound_client_data_msg = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:4578").unwrap(),
            reception_port_opt: None,
            last_data: true,
            is_clandestine: false,
            sequence_number_opt: Some(123),
            data: vec![],
        };

        let result = IBCDHelperReal::new()
            .handle_normal_client_data(&mut proxy_server, inbound_client_data_msg);

        assert_eq!(
            result,
            Err(
                "No origin port specified with 0-byte non-clandestine packet; contents redacted"
                    .to_string()
            )
        );
    }

    #[test]
    fn resolve_message_defers_mailbox_error_side_effects_to_proxy_server() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let payload = make_request_payload(111, cryptde);
        let stream_key = payload.stream_key;
        let (proxy_server, _, proxy_server_recording_arc) = make_recorder();
        let addr = proxy_server.start();
        let proxy_server_sub = recipient!(&addr, AddRouteResultMessage);
        let args = TransmitToHopperArgs {
            main_cryptde: cryptde.dup(),
            payload,
            return_route_id: 8888,
            route_request_id: 77,
            client_addr: SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            timestamp: SystemTime::now(),
            is_decentralized: false,
            require_service_receipt_capability: false,
            logger: Logger::new("test"),
            hopper_sub: recipient!(&addr, IncipientCoresPackage),
            dispatcher_sub: recipient!(&addr, TransmitDataMsg),
            accountant_sub: recipient!(&addr, ReportServicesConsumedMessage),
            record_exit_request_for_receipt: recipient!(&addr, RecordExitRequestForReceipt),
            retire_stream_key_sub_opt: None,
        };
        let subject = RouteQueryResponseResolverReal {};
        let system =
            System::new("resolve_message_defers_mailbox_error_side_effects_to_proxy_server");

        subject.resolve_message(args, proxy_server_sub, Err(MailboxError::Timeout));

        System::current().stop();
        system.run();
        let proxy_server_recording = proxy_server_recording_arc.lock().unwrap();
        assert_eq!(proxy_server_recording.len(), 1);
        let message = proxy_server_recording.get_record::<AddRouteResultMessage>(0);
        let expected_error_message =
            "Neighborhood refused to answer route request; details redacted";
        assert_eq!(
            message,
            &AddRouteResultMessage {
                stream_key,
                route_request_id: 77,
                result: Err(expected_error_message.to_string())
            }
        );
    }

    #[derive(Default)]
    struct ClientRequestPayloadFactoryMock {
        make_params: Arc<
            Mutex<
                Vec<(
                    InboundClientData,
                    StreamKey,
                    Option<Host>,
                    Box<dyn CryptDE>,
                    Logger,
                )>,
            >,
        >,
        make_results: RefCell<Vec<Option<ClientRequestPayload_0v1>>>,
    }

    impl ClientRequestPayloadFactory for ClientRequestPayloadFactoryMock {
        fn make(
            &self,
            ibcd: &InboundClientData,
            stream_key: StreamKey,
            host_opt: Option<Host>,
            cryptde: &dyn CryptDE,
            logger: &Logger,
        ) -> Option<ClientRequestPayload_0v1> {
            self.make_params.lock().unwrap().push((
                ibcd.clone(),
                stream_key,
                host_opt,
                cryptde.dup(),
                logger.clone(),
            ));
            self.make_results.borrow_mut().remove(0)
        }
    }

    impl ClientRequestPayloadFactoryMock {
        fn new() -> Self {
            Self::default()
        }

        fn make_params(
            mut self,
            params: &Arc<
                Mutex<
                    Vec<(
                        InboundClientData,
                        StreamKey,
                        Option<Host>,
                        Box<dyn CryptDE>,
                        Logger,
                    )>,
                >,
            >,
        ) -> Self {
            self.make_params = params.clone();
            self
        }

        fn make_result(self, result: Option<ClientRequestPayload_0v1>) -> Self {
            self.make_results.borrow_mut().push(result);
            self
        }
    }

    #[test]
    fn help_to_handle_normal_client_data_make_payload_failed() {
        let mut proxy_server = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        proxy_server.subs = Some(make_proxy_server_out_subs());
        proxy_server.client_request_payload_factory =
            Box::new(ClientRequestPayloadFactoryMock::default().make_result(None));
        let inbound_client_data_msg = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:4578").unwrap(),
            reception_port_opt: Some(568),
            last_data: true,
            is_clandestine: false,
            sequence_number_opt: Some(123),
            data: vec![],
        };

        let result = IBCDHelperReal::new()
            .handle_normal_client_data(&mut proxy_server, inbound_client_data_msg);

        assert_eq!(
            result,
            Err("Couldn't create ClientRequestPayload".to_string())
        );
    }

    #[test]
    fn new_http_request_creates_new_entry_inside_dns_retries_hashmap() {
        let test_name = "new_http_request_creates_new_entry_inside_dns_retries_hashmap";
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let (neighborhood_mock, _, _) = make_recorder();
        let destination_key = PublicKey::from(&b"our destination"[..]);
        let neighborhood_mock = neighborhood_mock.route_query_response(Some(RouteQueryResponse {
            route: Route { hops: vec![] },
            expected_services: ExpectedServices::RoundTrip(
                vec![make_exit_service_from_key(destination_key.clone())],
                vec![],
            ),
            host: Host::new("booga.com", HTTP_PORT),
        }));
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let mut expected_payload = ClientRequestPayloadFactoryReal::new()
            .make(
                &msg_from_dispatcher,
                stream_key.clone(),
                None,
                CRYPTDE_PAIR.alias.as_ref(),
                &Logger::new("test"),
            )
            .unwrap();
        expected_payload.dns_attempt_id_opt = Some(0);
        let stream_key_factory = StreamKeyFactoryMock::new().make_result(stream_key.clone());
        let system = System::new(test_name);
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.stream_key_factory = Box::new(stream_key_factory);
        let subject_addr: Addr<ProxyServer> = subject.start();
        let peer_actors = peer_actors_builder()
            .neighborhood(neighborhood_mock)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(msg_from_dispatcher).unwrap();

        subject_addr
            .try_send(AssertionsMessage {
                assertions: Box::new(move |proxy_server: &mut ProxyServer| {
                    let dns_retry = proxy_server
                        .stream_info(&stream_key)
                        .unwrap()
                        .dns_failure_retry_opt
                        .as_ref()
                        .unwrap();
                    assert_eq!(dns_retry.retries_left, 3);
                    assert_eq!(dns_retry.active_attempt_id, 0);
                    assert_eq!(dns_retry.unsuccessful_request, expected_payload);
                }),
            })
            .unwrap();
        System::current().stop();
        system.run();
    }

    #[test]
    fn new_http_request_creates_new_exhausted_entry_inside_dns_retries_hashmap_zero_hop() {
        let test_name =
            "new_http_request_creates_new_exhausted_entry_inside_dns_retries_hashmap_zero_hop";
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: nowhere.com\r\n\r\n";
        let (neighborhood_mock, _, _) = make_recorder();
        let destination_key = PublicKey::from(&b"our destination"[..]);
        let neighborhood_mock = neighborhood_mock.route_query_response(Some(RouteQueryResponse {
            route: Route { hops: vec![] },
            expected_services: ExpectedServices::RoundTrip(
                vec![make_exit_service_from_key(destination_key.clone())],
                vec![],
            ),
            host: Host::new("booga.com", HTTP_PORT),
        }));
        let socket_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let stream_key = StreamKey::make_meaningless_stream_key();
        let expected_data = http_request.to_vec();
        let msg_from_dispatcher = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr.clone(),
            reception_port_opt: Some(HTTP_PORT),
            sequence_number_opt: Some(0),
            last_data: true,
            is_clandestine: false,
            data: expected_data.clone(),
        };
        let mut expected_payload = ClientRequestPayloadFactoryReal::new()
            .make(
                &msg_from_dispatcher,
                stream_key.clone(),
                None,
                CRYPTDE_PAIR.alias.as_ref(),
                &Logger::new("test"),
            )
            .unwrap();
        expected_payload.dns_attempt_id_opt = Some(0);
        let stream_key_factory = StreamKeyFactoryMock::new().make_result(stream_key.clone());
        let system = System::new(test_name);
        let mut subject = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            false,
            Some(STANDARD_CONSUMING_WALLET_BALANCE),
            false,
            false,
        );
        subject.stream_key_factory = Box::new(stream_key_factory);
        let subject_addr: Addr<ProxyServer> = subject.start();
        let peer_actors = peer_actors_builder()
            .neighborhood(neighborhood_mock)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(msg_from_dispatcher).unwrap();

        subject_addr
            .try_send(AssertionsMessage {
                assertions: Box::new(move |proxy_server: &mut ProxyServer| {
                    let dns_retry = proxy_server
                        .stream_info(&stream_key)
                        .unwrap()
                        .dns_failure_retry_opt
                        .as_ref()
                        .unwrap();
                    assert_eq!(dns_retry.retries_left, 0);
                    assert_eq!(dns_retry.active_attempt_id, 0);
                    assert_eq!(dns_retry.unsuccessful_request, expected_payload);
                }),
            })
            .unwrap();
        System::current().stop();
        system.run();
    }

    #[test]
    fn hostname_works() {
        assert_on_hostname(
            "https://www.example.com/folder/file.html",
            "www.example.com",
        );
        assert_on_hostname("www.example.com/index.php?arg=test", "www.example.com");
        assert_on_hostname("sub.example.com/index.php?arg=test", "sub.example.com");
        assert_on_hostname("1.1.1.1", "1.1.1.1");
        assert_on_hostname("", "");
        assert_on_hostname("example", "example");
        assert_on_hostname(
            "htttttps://www.example.com/folder/file.html",
            "htttttps://www.example.com/folder/file.html",
        );
    }

    fn assert_on_hostname(raw_url: &str, expected_hostname: &str) {
        let clean_hostname = Hostname::new(raw_url);
        let expected_result = Hostname {
            hostname: expected_hostname.to_string(),
        };
        assert_eq!(expected_result, clean_hostname);
    }

    #[test]
    fn hostname_is_valid_works() {
        // IPv4
        assert_eq!(
            Hostname::new("0.0.0.0").validate_non_loopback_host(),
            Err("0.0.0.0".to_string())
        );
        assert_eq!(
            Hostname::new("192.168.1.158").validate_non_loopback_host(),
            Ok(())
        );
        // IPv6
        assert_eq!(
            Hostname::new("0:0:0:0:0:0:0:0").validate_non_loopback_host(),
            Err("::".to_string())
        );
        assert_eq!(
            Hostname::new("0:0:0:0:0:0:0:1").validate_non_loopback_host(),
            Err("::1".to_string())
        );
        assert_eq!(
            Hostname::new("2001:0db8:85a3:0000:0000:8a2e:0370:7334").validate_non_loopback_host(),
            Ok(())
        );
        // Hostname
        assert_eq!(
            Hostname::new("localhost").validate_non_loopback_host(),
            Err("localhost".to_string())
        );
        assert_eq!(
            Hostname::new("www.example.com").validate_non_loopback_host(),
            Ok(())
        );
        assert_eq!(
            Hostname::new("https://www.example.com").validate_non_loopback_host(),
            Ok(())
        );
    }

    #[test]
    fn proxy_server_field_test_is_running_in_integration_test() {
        let is_running_in_integration_test = false;
        let http_request = b"GET /index.html HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let expected_data = http_request.to_vec();
        let mut proxy_server = ProxyServer::new(
            CRYPTDE_PAIR.clone(),
            true,
            Some(58),
            false,
            is_running_in_integration_test,
        );
        proxy_server.subs = Some(make_proxy_server_out_subs());
        let inbound_client_data_msg = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:4578").unwrap(),
            reception_port_opt: Some(80),
            last_data: true,
            is_clandestine: false,
            sequence_number_opt: Some(123),
            data: expected_data,
        };

        let result = IBCDHelperReal::new()
            .handle_normal_client_data(&mut proxy_server, inbound_client_data_msg);

        assert_eq!(
            result,
            Err("Request to wildcard IP detected - localhost (Most likely because Blockchain Service URL is not set)".to_string())
        );
    }

    #[test]
    fn make_payload_passes_no_hostname_if_none_is_known() {
        let mut subject = ProxyServer::new(CRYPTDE_PAIR.clone(), true, Some(58), false, false);
        let make_params_arc = Arc::new(Mutex::new(vec![]));
        let client_request_payload_factory = ClientRequestPayloadFactoryMock::new()
            .make_params(&make_params_arc)
            .make_result(None);
        subject.client_request_payload_factory = Box::new(client_request_payload_factory);
        let stream_key = StreamKey::make_meaningless_stream_key();
        // Do not create an entry in subject.stream_info for stream_key, so that no hostname is known

        let _ = subject.make_payload(
            InboundClientData {
                // irrelevant
                timestamp: SystemTime::now(),
                client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                reception_port_opt: Some(HTTP_PORT),
                last_data: false,
                is_clandestine: false,
                sequence_number_opt: Some(123),
                data: vec![],
            },
            &stream_key,
        );

        let (_ibcd, _sk, hostname_opt, _cryptde, _logger) = &make_params_arc.lock().unwrap()[0];
        assert_eq!(hostname_opt, &None);
    }

    #[test]
    fn make_payload_passes_hostname_if_known() {
        let mut subject = ProxyServer::new(CRYPTDE_PAIR.clone(), true, Some(58), false, false);
        let make_params_arc = Arc::new(Mutex::new(vec![]));
        let client_request_payload_factory = ClientRequestPayloadFactoryMock::new()
            .make_params(&make_params_arc)
            .make_result(None); // Don't care about return value, only parameters
        subject.client_request_payload_factory = Box::new(client_request_payload_factory);
        let stream_key = StreamKey::make_meaningless_stream_key();
        let si_host = Host::new("knownhostname.com", 2345);
        subject.stream_info.insert(
            stream_key.clone(),
            StreamInfo {
                tunneled_host_opt: None,
                dns_failure_retry_opt: None,
                route_opt: Some(RouteQueryResponse {
                    route: Route { hops: vec![] },
                    expected_services: ExpectedServices::RoundTrip(vec![], vec![]),
                    host: Host::new(&si_host.name, 2345),
                }),
                protocol_opt: None,
                browser_proxy_sequence_offset: false,
                response_sequence_replay_window: ResponseSequenceReplayWindow::default(),
                request_started_at_opt: None,
                time_to_live_opt: None,
                route_success_metadata_reported: false,
            },
        );

        let _ = subject.make_payload(
            InboundClientData {
                // irrelevant
                timestamp: SystemTime::now(),
                client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                reception_port_opt: Some(HTTP_PORT),
                last_data: false,
                is_clandestine: false,
                sequence_number_opt: Some(123),
                data: vec![],
            },
            &stream_key,
        );

        let (_ibcd, _sk, host_opt, _cryptde, _logger) = &make_params_arc.lock().unwrap()[0];
        assert_eq!(host_opt, &Some(si_host));
    }

    #[test]
    #[should_panic(
        expected = "ProxyServer should never get ShutdownStreamMsg about clandestine stream"
    )]
    fn handle_stream_shutdown_complains_about_clandestine_message() {
        let system = System::new("test");
        let subject = ProxyServer::new(CRYPTDE_PAIR.clone(), true, None, false, false);
        let subject_addr = subject.start();

        subject_addr
            .try_send(StreamShutdownMsg {
                peer_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                stream_type: RemovedStreamType::Clandestine,
                report_to_counterpart: false,
            })
            .unwrap();

        System::current().stop();
        system.run();
    }

    #[test]
    #[should_panic(
        expected = "panic message (processed with: node_lib::sub_lib::utils::crash_request_analyzer)"
    )]
    fn proxy_server_can_be_crashed_properly_but_not_improperly() {
        let proxy_server = ProxyServer::new(CRYPTDE_PAIR.clone(), true, None, true, false);

        prove_that_crash_request_handler_is_hooked_up(proxy_server, CRASH_KEY);
    }

    #[test]
    fn find_or_generate_stream_key_prioritizes_existing_stream_key_first() {
        init_test_logging();
        let test_name = "find_or_generate_stream_key_prioritizes_existing_stream_key_first";
        let socket_addr = SocketAddr::from_str("1.2.3.4:4321").unwrap();
        let stream_key = StreamKey::new(CRYPTDE_PAIR.main.as_ref().public_key(), socket_addr);
        let mut subject = ProxyServer::new(CRYPTDE_PAIR.clone(), true, None, false, false);
        subject.logger = Logger::new(test_name);
        subject.keys_and_addrs.insert(stream_key, socket_addr);
        let ibcd = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr,
            reception_port_opt: Some(2222),
            last_data: true,
            is_clandestine: false,
            sequence_number_opt: Some(333),
            data: b"GET /index.html HTTP/1.1\r\nHost: header.com:3333\r\n\r\n".to_vec(),
        };

        let result = subject.find_or_generate_stream_key(&ibcd);

        assert_eq!(result, stream_key);
        assert_eq!(
            subject.keys_and_addrs.a_to_b(&stream_key),
            Some(socket_addr)
        );
        let logs = TestLogHandler::new();
        logs.exists_log_containing(&format!(
            "DEBUG: {test_name}: find_or_generate_stream_key() retrieved existing mapping; stream and client redacted"
        ));
        logs.exists_no_log_containing(&format!(
            "DEBUG: {test_name}: find_or_generate_stream_key() retrieved existing key"
        ));
    }

    #[test]
    fn find_or_generate_stream_key_creates_stream_key_if_necessary() {
        init_test_logging();
        let test_name = "find_or_generate_stream_key_creates_stream_key_if_necessary";
        let socket_addr = SocketAddr::from_str("1.2.3.4:4321").unwrap();
        let stream_key = StreamKey::new(CRYPTDE_PAIR.main.as_ref().public_key(), socket_addr);
        let mut subject = ProxyServer::new(CRYPTDE_PAIR.clone(), true, None, false, false);
        subject.logger = Logger::new(test_name);
        let ibcd = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: socket_addr,
            reception_port_opt: Some(2222),
            last_data: true,
            is_clandestine: false,
            sequence_number_opt: Some(333),
            data: b"GET /index.html HTTP/1.1\r\nHost: header.com:4444\r\n\r\n".to_vec(),
        };

        let result = subject.find_or_generate_stream_key(&ibcd);

        assert_eq!(result, stream_key);
        assert_eq!(
            subject.keys_and_addrs.a_to_b(&stream_key),
            Some(socket_addr)
        );
        let logs = TestLogHandler::new();
        logs.exists_log_containing(&format!(
            "DEBUG: {test_name}: find_or_generate_stream_key() inserted new mapping; stream and client redacted"
        ));
        logs.exists_no_log_containing(&format!(
            "DEBUG: {test_name}: find_or_generate_stream_key() inserted new key"
        ));
    }

    fn make_server_com_client_hello() -> Vec<u8> {
        [
            0x16, // content_type: Handshake
            0x00, 0x00, 0x00, 0x00, // version, length: don't care
            0x01, // handshake_type: ClientHello
            0x00, 0x00, 0x00, 0x00, 0x00, // length, version: don't care
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, // random: don't care
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, // random: don't care
            0x00, // session_id_length
            0x00, 0x00, // cipher_suites_length
            0x00, // compression_methods_length
            0x00, 0x13, // extensions_length
            0x00, 0x00, // extension_type: server_name
            0x00, 0x0F, // extension_length
            0x00, 0x0D, // server_name_list_length
            0x00, // server_name_type
            0x00, 0x0A, // server_name_length
            b's', b'e', b'r', b'v', b'e', b'r', b'.', b'c', b'o', b'm', // server_name
        ]
        .to_vec()
    }

    fn make_exit_service_from_key(public_key: PublicKey) -> ExpectedService {
        ExpectedService::Exit(public_key, make_wallet("exit wallet"), rate_pack(100))
    }
}
