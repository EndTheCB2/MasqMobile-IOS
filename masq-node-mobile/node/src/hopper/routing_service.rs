// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use super::live_cores_package::LiveCoresPackage;
use crate::accountant::db_access_objects::receipt_checkpoint_dao::{
    ReceiptCheckpointDao, RoutingReceiptOfferState,
};
use crate::blockchain::payer::Payer;
use crate::bootstrapper::CryptDEPair;
use crate::neighborhood::gossip::Gossip_0v1;
use crate::sub_lib::accountant::ReportRoutingServiceProvidedMessage;
use crate::sub_lib::cryptde::{decodex, encodex, CryptData, CryptdecError};
use crate::sub_lib::dispatcher::{Component, Endpoint, InboundClientData};
use crate::sub_lib::hop::LiveHop;
use crate::sub_lib::hopper::{ExpiredCoresPackage, HopperSubs, MessageType};
use crate::sub_lib::neighborhood::{GossipFailure_0v1, NeighborhoodSubs};
use crate::sub_lib::proxy_client::{
    ClientResponsePayload_0v1, DnsResolveFailure_0v1, MeteredClientResponsePayload_0v1,
    ProxyClientSubs,
};
use crate::sub_lib::proxy_server::{ClientRequestPayload_0v1, ProxyServerSubs};
use crate::sub_lib::service_receipt::{
    ReceiptSequenceCheckpoint, ReceiptSessionRequest, ServiceKind, ServiceReceipt,
    ServiceReceiptOfferPayload_0v1, ServiceReceiptPayload_0v1,
};
use crate::sub_lib::stream_handler_pool::TransmitDataMsg;
use actix::Recipient;
use masq_lib::logger::Logger;
use std::borrow::Borrow;
use std::cell::RefCell;
use std::convert::TryFrom;
use std::net::SocketAddr;
use std::time::SystemTime;
use web3::types::Address;

const MAX_ROUTING_RECEIPT_OFFERS_PER_PACKAGE: usize = 64;
const MAX_ROUTING_RECEIPT_SESSION_STATES: usize = 4096;

struct ServiceReceiptSettlementContext {
    dao: RefCell<Box<dyn ReceiptCheckpointDao>>,
    chain_id: u64,
    settlement_contract: Address,
}

pub struct RoutingServiceSubs {
    pub proxy_client_subs_opt: Option<ProxyClientSubs>,
    pub proxy_server_subs: ProxyServerSubs,
    pub neighborhood_subs: NeighborhoodSubs,
    pub hopper_subs: HopperSubs,
    pub to_dispatcher: Recipient<TransmitDataMsg>,
    pub to_accountant_routing: Recipient<ReportRoutingServiceProvidedMessage>,
}

pub struct RoutingService {
    cryptde_pair: CryptDEPair,
    routing_service_subs: RoutingServiceSubs,
    per_routing_service: u64,
    per_routing_byte: u64,
    logger: Logger,
    is_decentralized: bool,
    service_receipt_context_opt: Option<ServiceReceiptSettlementContext>,
}

impl RoutingService {
    pub fn new(
        cryptde_pair: CryptDEPair,
        routing_service_subs: RoutingServiceSubs,
        per_routing_service: u64,
        per_routing_byte: u64,
        is_decentralized: bool,
    ) -> RoutingService {
        RoutingService {
            cryptde_pair,
            routing_service_subs,
            per_routing_service,
            per_routing_byte,
            logger: Logger::new("RoutingService"),
            is_decentralized,
            service_receipt_context_opt: None,
        }
    }

    pub fn enable_service_receipts(
        &mut self,
        dao: Box<dyn ReceiptCheckpointDao>,
        chain_id: u64,
        settlement_contract: Address,
    ) {
        self.service_receipt_context_opt = Some(ServiceReceiptSettlementContext {
            dao: RefCell::new(dao),
            chain_id,
            settlement_contract,
        });
    }

    pub fn route(&self, ibcd: InboundClientData) {
        let data_size = ibcd.data.len();
        debug!(
            self.logger,
            "Instructed to route {} bytes of InboundClientData ({}) from Dispatcher",
            data_size,
            ibcd.client_addr
        );
        let peer_addr = ibcd.client_addr;
        let last_data = ibcd.last_data;
        let ibcd_but_data = ibcd.clone_but_data();

        let live_package = match decodex::<LiveCoresPackage>(
            self.cryptde_pair.main.as_ref(),
            &CryptData::new(&ibcd.data[..]),
        ) {
            Ok(lcp) => lcp,
            Err(e) => {
                error!(
                    self.logger,
                    "Couldn't decode CORES package in {}-byte buffer from {}: {:?}",
                    ibcd.data.len(),
                    ibcd.client_addr,
                    e
                );
                return;
            }
        };

        let next_hop = match live_package.route.next_hop(self.cryptde_pair.main.borrow()) {
            Ok(hop) => hop,
            Err(e) => {
                error!(
                    self.logger,
                    "Invalid {}-byte CORES package: {:?}", data_size, e
                );
                return;
            }
        };

        self.route_data(peer_addr, next_hop, live_package, last_data, &ibcd_but_data);
    }

    fn route_data(
        &self,
        sender_addr: SocketAddr,
        next_hop: LiveHop,
        live_package: LiveCoresPackage,
        last_data: bool,
        ibcd_but_data: &InboundClientData,
    ) {
        if (next_hop.component == Component::Hopper) && (!self.is_destined_for_here(&next_hop)) {
            debug!(
                self.logger,
                "Routing LiveCoresPackage with {}-byte payload to {}",
                live_package.payload.len(),
                next_hop.public_key
            );
            self.route_data_externally(
                live_package,
                next_hop.payer,
                next_hop.routing_receipt_request_opt,
                last_data,
            );
        } else {
            debug!(
                self.logger,
                "Transferring LiveCoresPackage with {}-byte payload to {:?}",
                live_package.payload.len(),
                next_hop.component
            );
            self.route_data_internally(&next_hop, sender_addr, live_package, ibcd_but_data)
        }
    }

    fn is_destined_for_here(&self, next_hop: &LiveHop) -> bool {
        &next_hop.public_key == self.cryptde_pair.main.public_key()
    }

    fn route_data_internally(
        &self,
        next_hop: &LiveHop,
        immediate_neighbor_addr: SocketAddr,
        live_package: LiveCoresPackage,
        ibcd_but_data: &InboundClientData,
    ) {
        let payload_size = live_package.payload.len();
        if next_hop.component == Component::Hopper {
            self.route_data_around_again(live_package, ibcd_but_data)
        } else {
            match &next_hop.payer {
                None => (),
                Some(payer) => {
                    if payer.is_delinquent() {
                        warning!(self.logger,
                        "A paying wallet is delinquent; electing not to route {}-byte payload to {:?}",
                        payload_size,
                        next_hop.component,
                    );
                        return;
                    }
                }
            }
            self.route_data_to_peripheral_component(
                next_hop.component,
                immediate_neighbor_addr,
                live_package,
                next_hop.payer_owns_secret_key(&self.cryptde_pair.main.digest()),
            )
        }
    }

    fn route_data_around_again(
        &self,
        live_package: LiveCoresPackage,
        ibcd_but_data: &InboundClientData,
    ) {
        let (_, next_lcp) = match live_package.into_next_live(self.cryptde_pair.main.as_ref()) {
            Ok(x) => x,
            Err(e) => {
                error!(self.logger, "bad zero-hop route: {:?}", e);
                return;
            }
        };
        let payload = encodex(
            self.cryptde_pair.main.as_ref(),
            self.cryptde_pair.main.public_key(),
            &next_lcp,
        )
        .expect("Encryption of LiveCoresPackage failed");
        let inbound_client_data = InboundClientData {
            timestamp: ibcd_but_data.timestamp,
            client_addr: ibcd_but_data.client_addr,
            reception_port_opt: ibcd_but_data.reception_port_opt,
            last_data: ibcd_but_data.last_data,
            is_clandestine: ibcd_but_data.is_clandestine,
            sequence_number_opt: ibcd_but_data.sequence_number_opt,
            data: payload.into(),
        };
        self.routing_service_subs
            .hopper_subs
            .from_dispatcher
            .try_send(inbound_client_data)
            .expect("Hopper is dead");
    }

    fn route_data_to_peripheral_component(
        &self,
        component: Component,
        immediate_neighbor_addr: SocketAddr,
        live_package: LiveCoresPackage,
        payer_owns_secret_key: bool,
    ) {
        let expired_package =
            match self.extract_expired_package(immediate_neighbor_addr, live_package, component) {
                None => return,
                Some(p) => p,
            };
        trace!(
            self.logger,
            "Forwarding ExpiredCoresPackage to {:?}",
            component
        );
        self.route_expired_package(component, expired_package, payer_owns_secret_key)
    }

    fn extract_expired_package(
        &self,
        immediate_neighbor_addr: SocketAddr,
        live_package: LiveCoresPackage,
        component: Component,
    ) -> Option<ExpiredCoresPackage<MessageType>> {
        let data_len = live_package.payload.len();
        let (payload_cryptde, cryptde_name) = match component {
            Component::ProxyServer => (self.cryptde_pair.alias.as_ref(), "alias"),
            _ => (self.cryptde_pair.main.as_ref(), "main"),
        };
        let expired_package = match live_package.to_expired(
            immediate_neighbor_addr,
            self.cryptde_pair.main.as_ref(),
            payload_cryptde,
        ) {
            Ok(pkg) => pkg,
            Err(e) => {
                error!(
                    self.logger,
                    "Couldn't expire CORES package with {}-byte payload to {:?} using {} key: {:?}",
                    data_len,
                    component,
                    cryptde_name,
                    e
                );
                return None;
            }
        };
        Some(expired_package)
    }

    fn route_expired_package(
        &self,
        component: Component,
        expired_package: ExpiredCoresPackage<MessageType>,
        payer_owns_secret_key: bool,
    ) {
        let immediate_neighbor = expired_package.immediate_neighbor;
        match (component, expired_package.payload) {
            (Component::ProxyClient, MessageType::ClientRequest(vd)) => {
                if !self.is_decentralized || payer_owns_secret_key {
                    let proxy_client_subs = match &self.routing_service_subs.proxy_client_subs_opt {
                        Some(pcs) => pcs,
                        None => {
                            warning!(self.logger, "Received CORES package from {:?} for Proxy Client, but Proxy Client isn't running", immediate_neighbor);
                            return;
                        }
                    };
                    let client_request = match ClientRequestPayload_0v1::try_from(vd) {
                        Ok(crp) => crp,
                        Err(e) => {
                            error!(
                                self.logger,
                                "Received unmigratable ClientRequestPayload: {:?}", e
                            );
                            return;
                        }
                    };
                    proxy_client_subs
                        .from_hopper
                        .try_send(ExpiredCoresPackage::new_with_routing_receipt_offers(
                            expired_package.immediate_neighbor,
                            expired_package.paying_wallet,
                            expired_package.remaining_route,
                            client_request,
                            expired_package.payload_len,
                            expired_package.routing_receipt_offers,
                        ))
                        .expect("ProxyClient is dead")
                } else {
                    let payload_len = &expired_package.payload_len;
                    warning!(
                        self.logger,
                            "Refusing to route Expired CORES package with {}-byte payload without proof of paying wallet ownership.",
                        payload_len
                    );
                }
            }
            (Component::ProxyServer, MessageType::ClientResponse(vd)) => {
                let client_response = match ClientResponsePayload_0v1::try_from(vd) {
                    Ok(crp) => crp,
                    Err(e) => {
                        error!(
                            self.logger,
                            "Received unmigratable ClientResponsePayload: {:?}", e
                        );
                        return;
                    }
                };
                self.routing_service_subs
                    .proxy_server_subs
                    .from_hopper
                    .try_send(ExpiredCoresPackage::new_with_routing_receipt_offers(
                        expired_package.immediate_neighbor,
                        expired_package.paying_wallet,
                        expired_package.remaining_route,
                        client_response,
                        expired_package.payload_len,
                        expired_package.routing_receipt_offers,
                    ))
                    .expect("ProxyServer is dead")
            }
            (Component::ProxyServer, MessageType::MeteredClientResponse(vd)) => {
                let metered_response = match MeteredClientResponsePayload_0v1::try_from(vd) {
                    Ok(response) => response,
                    Err(error) => {
                        error!(
                            self.logger,
                            "Received unmigratable MeteredClientResponsePayload: {:?}", error
                        );
                        return;
                    }
                };
                self.routing_service_subs
                    .proxy_server_subs
                    .metered_response_from_hopper
                    .try_send(ExpiredCoresPackage::new_with_routing_receipt_offers(
                        expired_package.immediate_neighbor,
                        expired_package.paying_wallet,
                        expired_package.remaining_route,
                        metered_response,
                        expired_package.payload_len,
                        expired_package.routing_receipt_offers,
                    ))
                    .expect("ProxyServer is dead")
            }
            (Component::ProxyServer, MessageType::DnsResolveFailed(vd)) => {
                let failure = match DnsResolveFailure_0v1::try_from(vd) {
                    Ok(f) => f,
                    Err(e) => {
                        error!(
                            self.logger,
                            "Received unmigratable DnsResolveFailed: {:?}", e
                        );
                        return;
                    }
                };
                self.routing_service_subs
                    .proxy_server_subs
                    .dns_failure_from_hopper
                    .try_send(ExpiredCoresPackage::new_with_routing_receipt_offers(
                        expired_package.immediate_neighbor,
                        expired_package.paying_wallet,
                        expired_package.remaining_route,
                        failure,
                        expired_package.payload_len,
                        expired_package.routing_receipt_offers,
                    ))
                    .expect("ProxyServer is dead")
            }
            (Component::Neighborhood, MessageType::Gossip(vd)) => {
                let gossip = match Gossip_0v1::try_from(vd) {
                    Ok(g) => g,
                    Err(e) => {
                        error!(self.logger, "Received unmigratable Gossip: {:?}", e);
                        return;
                    }
                };
                self.routing_service_subs
                    .neighborhood_subs
                    .from_hopper
                    .try_send(ExpiredCoresPackage::new_with_routing_receipt_offers(
                        expired_package.immediate_neighbor,
                        expired_package.paying_wallet,
                        expired_package.remaining_route,
                        gossip,
                        expired_package.payload_len,
                        expired_package.routing_receipt_offers,
                    ))
                    .expect("Neighborhood is dead")
            }
            (Component::Neighborhood, MessageType::GossipFailure(vd)) => {
                let failure = match GossipFailure_0v1::try_from(vd) {
                    Ok(f) => f,
                    Err(e) => {
                        error!(self.logger, "Received unmigratable GossipFailure: {:?}", e);
                        return;
                    }
                };
                self.routing_service_subs
                    .neighborhood_subs
                    .gossip_failure
                    .try_send(ExpiredCoresPackage::new_with_routing_receipt_offers(
                        expired_package.immediate_neighbor,
                        expired_package.paying_wallet,
                        expired_package.remaining_route,
                        failure,
                        expired_package.payload_len,
                        expired_package.routing_receipt_offers,
                    ))
                    .expect("Neighborhood is dead")
            }
            (Component::ProxyServer, MessageType::ServiceReceiptOffer(vd)) => {
                let offer = match ServiceReceiptOfferPayload_0v1::try_from(vd) {
                    Ok(offer) => offer,
                    Err(error) => {
                        error!(
                            self.logger,
                            "Received unmigratable ServiceReceiptOfferPayload: {:?}", error
                        );
                        return;
                    }
                };
                self.routing_service_subs
                    .proxy_server_subs
                    .receipt_offer_from_hopper
                    .try_send(ExpiredCoresPackage::new_with_routing_receipt_offers(
                        expired_package.immediate_neighbor,
                        expired_package.paying_wallet,
                        expired_package.remaining_route,
                        offer,
                        expired_package.payload_len,
                        expired_package.routing_receipt_offers,
                    ))
                    .expect("ProxyServer is dead")
            }
            (Component::Hopper, MessageType::ServiceReceipt(vd)) => {
                let payload = match ServiceReceiptPayload_0v1::try_from(vd) {
                    Ok(payload) => payload,
                    Err(error) => {
                        error!(
                            self.logger,
                            "Received unmigratable ServiceReceiptPayload: {:?}", error
                        );
                        return;
                    }
                };
                self.accept_service_receipt(payload);
            }
            (destination, payload) => error!(
                self.logger,
                "Attempt to send invalid combination {:?} to {:?}", payload, destination
            ),
        };
    }

    fn accept_service_receipt(&self, payload: ServiceReceiptPayload_0v1) {
        let context = match self.service_receipt_context_opt.as_ref() {
            Some(context) => context,
            None => {
                warning!(
                    self.logger,
                    "Refusing service receipt because receipt settlement is disabled"
                );
                return;
            }
        };
        let receipt = &payload.acknowledged_receipt.signed_receipt.receipt;
        if &receipt.provider_public_key != self.cryptde_pair.main.public_key() {
            warning!(
                self.logger,
                "Refusing service receipt addressed to a different provider"
            );
            return;
        }
        let now = SystemTime::now();
        let now_unix_s = match now.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(duration) => duration.as_secs(),
            Err(_) => {
                error!(
                    self.logger,
                    "Refusing service receipt before the Unix epoch"
                );
                return;
            }
        };
        if let Err(error) = payload.authorization.verify_for_receipt(
            &payload.acknowledged_receipt,
            self.cryptde_pair.main.as_ref(),
            context.chain_id,
            context.settlement_contract,
            now_unix_s,
        ) {
            warning!(
                self.logger,
                "Refusing service receipt that failed verification: {:?}",
                error
            );
            return;
        }

        let mut dao = context.dao.borrow_mut();
        let checkpoint_result = dao.checkpoint(
            &receipt.route_epoch,
            &receipt.provider_public_key,
            &payload.acknowledged_receipt.payer_session_public_key,
        );
        let checkpoint = match checkpoint_result {
            Ok(Some(mut checkpoint)) => {
                if let Err(error) = checkpoint.advance_for_settlement(
                    &payload.acknowledged_receipt,
                    self.cryptde_pair.main.as_ref(),
                ) {
                    warning!(
                        self.logger,
                        "Refusing invalid cumulative service receipt: {:?}",
                        error
                    );
                    return;
                }
                checkpoint
            }
            Ok(None) => match ReceiptSequenceCheckpoint::begin_for_settlement(
                &payload.acknowledged_receipt,
                self.cryptde_pair.main.as_ref(),
            ) {
                Ok(checkpoint) => checkpoint,
                Err(error) => {
                    warning!(
                        self.logger,
                        "Refusing invalid initial service receipt: {:?}",
                        error
                    );
                    return;
                }
            },
            Err(error) => {
                error!(
                    self.logger,
                    "Could not load service receipt checkpoint: {:?}", error
                );
                return;
            }
        };

        match dao.accept_verified_receipt(&payload, &checkpoint, now) {
            Ok(charge_wei) => debug!(
                self.logger,
                "Accepted service receipt sequence {} for {} wei",
                checkpoint.last_sequence,
                charge_wei
            ),
            Err(error) => warning!(
                self.logger,
                "Refusing service receipt during atomic accounting: {:?}",
                error
            ),
        }
    }

    fn route_data_externally(
        &self,
        mut live_package: LiveCoresPackage,
        payer: Option<Payer>,
        routing_receipt_request_opt: Option<ReceiptSessionRequest>,
        last_data: bool,
    ) {
        let payload_size = live_package.payload.len();
        match payer {
            Some(payer) => {
                if !payer.owns_secret_key(&self.cryptde_pair.main.digest()) {
                    warning!(self.logger,
                        "Refusing to route Live CORES package with {}-byte payload without proof of paying wallet ownership.",
                        payload_size
                    );
                    return;
                }
                if payer.is_delinquent() {
                    warning!(self.logger,
                        "A paying wallet is delinquent; electing not to route {}-byte payload further",
                        payload_size,
                    );
                    return;
                }
                match routing_receipt_request_opt {
                    Some(request) => {
                        if request.authorization.policy.payer_wallet_address
                            != payer.wallet.address()
                        {
                            warning!(
                                self.logger,
                                "Refusing routing receipt whose delegated wallet differs from the hop payer"
                            );
                            return;
                        }
                        if live_package.routing_receipt_offers.len()
                            >= MAX_ROUTING_RECEIPT_OFFERS_PER_PACKAGE
                        {
                            warning!(
                                self.logger,
                                "Refusing receipt-metered route whose encrypted offer capsule is full"
                            );
                            return;
                        }
                        let encrypted_offer =
                            match self.make_routing_receipt_offer(&request, payload_size) {
                                Ok(offer) => offer,
                                Err(error) => {
                                    warning!(
                                        self.logger,
                                        "Refusing receipt-metered routing service: {}",
                                        error
                                    );
                                    return;
                                }
                            };
                        live_package.routing_receipt_offers.push(encrypted_offer);
                    }
                    None => {
                        match self.routing_service_subs.to_accountant_routing.try_send(
                            ReportRoutingServiceProvidedMessage {
                                timestamp: SystemTime::now(),
                                paying_wallet: payer.wallet,
                                payload_size,
                                service_rate: self.per_routing_service,
                                byte_rate: self.per_routing_byte,
                            },
                        ) {
                            Ok(_) => (),
                            Err(e) => {
                                fatal!(self.logger, "Accountant is dead: {:?}", e);
                            }
                        }
                    }
                }
            }
            None => {
                warning!(
                    self.logger,
                    "Refusing to route Live CORES package with {}-byte payload without paying wallet",
                    payload_size
                );
                return;
            }
        }

        let transmit_msg = match self.to_transmit_data_msg(live_package, last_data) {
            Ok(m) => m,
            Err(e) => {
                error!(self.logger, "{:?}", e);
                return;
            }
        };

        debug!(
            self.logger,
            "Relaying {}-byte LiveCoresPackage to Dispatcher inside a TransmitDataMsg",
            transmit_msg.data.len()
        );
        self.routing_service_subs
            .to_dispatcher
            .try_send(transmit_msg)
            .expect("Dispatcher is dead");
    }

    fn make_routing_receipt_offer(
        &self,
        request: &ReceiptSessionRequest,
        payload_size: usize,
    ) -> Result<CryptData, String> {
        let context = self
            .service_receipt_context_opt
            .as_ref()
            .ok_or_else(|| "service-receipt settlement is disabled".to_string())?;
        let now_unix_s = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| "system time is before the Unix epoch".to_string())?
            .as_secs();
        request
            .verify(context.chain_id, context.settlement_contract, now_unix_s)
            .map_err(|error| format!("invalid receipt authorization: {:?}", error))?;
        let payload_size = u64::try_from(payload_size)
            .map_err(|_| "routing payload does not fit in a receipt".to_string())?;
        let payer_session_public_key = request
            .authorization
            .policy
            .payer_session_public_key
            .clone();
        let provider_public_key = self.cryptde_pair.main.public_key().clone();
        let persisted_state_opt = context
            .dao
            .borrow()
            .routing_offer_state(
                &request.authorization.policy.authorization_nonce,
                &request.route_epoch,
                &provider_public_key,
                &payer_session_public_key,
            )
            .map_err(|error| format!("cannot load durable routing receipt: {:?}", error))?;
        let checkpoint_opt = context
            .dao
            .borrow()
            .checkpoint(
                &request.route_epoch,
                &provider_public_key,
                &payer_session_public_key,
            )
            .map_err(|error| format!("cannot load routing acknowledgement: {:?}", error))?;
        if let Some(checkpoint) = checkpoint_opt.as_ref() {
            if checkpoint.route_epoch != request.route_epoch
                || checkpoint.provider_public_key != provider_public_key
                || checkpoint.payer_session_public_key != payer_session_public_key
                || checkpoint.accounting_commitment != request.accounting_commitment
            {
                return Err("durable routing acknowledgement identity changed".to_string());
            }
        }
        let mut persist_new_state = false;
        let signed_receipt = match persisted_state_opt {
            Some(state) => {
                let previous = &state.last_signed_receipt.receipt;
                if state.authorization_nonce != request.authorization.policy.authorization_nonce
                    || state.payer_session_public_key != payer_session_public_key
                    || state.expires_at_unix_s != request.authorization.policy.expires_at_unix_s
                    || previous.route_epoch != request.route_epoch
                    || previous.provider_public_key != provider_public_key
                    || previous.accounting_commitment != request.accounting_commitment
                    || previous.service_kind != ServiceKind::Routing
                    || previous.service_rate != self.per_routing_service
                    || previous.byte_rate != self.per_routing_byte
                {
                    return Err("durable routing receipt identity changed".to_string());
                }
                state
                    .last_signed_receipt
                    .verify(self.cryptde_pair.main.as_ref())
                    .map_err(|error| {
                        format!("durable routing receipt signature is invalid: {:?}", error)
                    })?;
                match checkpoint_opt {
                    Some(checkpoint)
                        if checkpoint.last_sequence == previous.sequence
                            && checkpoint.cumulative_charge_wei
                                == previous.cumulative_charge_wei =>
                    {
                        let next_sequence = previous
                            .sequence
                            .checked_add(1)
                            .ok_or_else(|| "routing receipt sequence exhausted".to_string())?;
                        let receipt = previous
                            .next_for_same_route(
                                next_sequence,
                                ServiceKind::Routing,
                                payload_size,
                                self.per_routing_service,
                                self.per_routing_byte,
                            )
                            .map_err(|error| {
                                format!("cannot advance routing receipt: {:?}", error)
                            })?;
                        persist_new_state = true;
                        receipt
                            .sign(self.cryptde_pair.main.as_ref())
                            .map_err(|error| format!("cannot sign routing receipt: {:?}", error))?
                    }
                    Some(checkpoint)
                        if checkpoint.last_sequence < previous.sequence
                            && checkpoint.cumulative_charge_wei
                                < previous.cumulative_charge_wei =>
                    {
                        // The exact prior offer is still pending. Re-transmit it instead of
                        // creating a sequence gap or charging the current packet twice.
                        state.last_signed_receipt
                    }
                    None => state.last_signed_receipt,
                    Some(_) => {
                        return Err("routing acknowledgement is inconsistent with durable offer"
                            .to_string())
                    }
                }
            }
            None => {
                if checkpoint_opt.is_some() {
                    return Err(
                        "routing acknowledgement exists without durable offer state".to_string()
                    );
                }
                persist_new_state = true;
                ServiceReceipt::new(
                    request.route_epoch,
                    1,
                    ServiceKind::Routing,
                    provider_public_key,
                    request.accounting_commitment,
                    payload_size,
                    self.per_routing_service,
                    self.per_routing_byte,
                )
                .sign(self.cryptde_pair.main.as_ref())
                .map_err(|error| format!("cannot sign routing receipt: {:?}", error))?
            }
        };
        request
            .authorization
            .verify(
                context.chain_id,
                context.settlement_contract,
                &payer_session_public_key,
                now_unix_s,
                signed_receipt.receipt.cumulative_charge_wei,
            )
            .map_err(|error| format!("routing receipt exceeds authorization: {:?}", error))?;
        let encrypted_offer = encodex(
            self.cryptde_pair.main.as_ref(),
            &payer_session_public_key,
            &ServiceReceiptOfferPayload_0v1 {
                signed_receipt: signed_receipt.clone(),
            },
        )
        .map_err(|error| format!("cannot encrypt routing receipt: {:?}", error))?;
        if persist_new_state {
            context
                .dao
                .borrow_mut()
                .save_routing_offer_state(
                    &RoutingReceiptOfferState {
                        authorization_nonce: request.authorization.policy.authorization_nonce,
                        payer_session_public_key,
                        expires_at_unix_s: request.authorization.policy.expires_at_unix_s,
                        last_signed_receipt: signed_receipt,
                    },
                    now_unix_s,
                    MAX_ROUTING_RECEIPT_SESSION_STATES,
                )
                .map_err(|error| format!("cannot persist routing receipt: {:?}", error))?;
        }
        Ok(encrypted_offer)
    }

    fn to_transmit_data_msg(
        &self,
        live_package: LiveCoresPackage,
        last_data: bool,
    ) -> Result<TransmitDataMsg, CryptdecError> {
        let (next_hop, next_live_package) =
            match live_package.into_next_live(self.cryptde_pair.main.borrow()) {
                Err(e) => {
                    let msg = format!(
                        "Couldn't get next hop and outgoing LCP from incoming LCP: {:?}",
                        e
                    );
                    error!(self.logger, "{}", &msg);
                    return Err(CryptdecError::OtherError(msg));
                }
                Ok(p) => p,
            };
        let next_live_package_enc = match encodex(
            self.cryptde_pair.main.as_ref(),
            &next_hop.public_key,
            &next_live_package,
        ) {
            Ok(nlpe) => nlpe,
            Err(e) => {
                let msg = format!("Couldn't serialize or encrypt outgoing LCP: {:?}", e);
                error!(self.logger, "{}", &msg);
                return Err(CryptdecError::OtherError(msg));
            }
        };
        Ok(TransmitDataMsg {
            endpoint: Endpoint::Key(next_hop.public_key),
            last_data,
            data: next_live_package_enc.into(),
            sequence_number_opt: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accountant::db_access_objects::banned_dao::BAN_CACHE;
    use crate::accountant::db_access_objects::receipt_checkpoint_dao::ReceiptCheckpointDaoError;
    use crate::neighborhood::gossip::{GossipBuilder, Gossip_0v1};
    use crate::node_test_utils::check_timestamp;
    use crate::sub_lib::accountant::ReportRoutingServiceProvidedMessage;
    use crate::sub_lib::cryptde::{encodex, CryptDE, PlainData, PublicKey};
    use crate::sub_lib::cryptde_null::CryptDENull;
    use crate::sub_lib::cryptde_real::CryptDEReal;
    use crate::sub_lib::hopper::{IncipientCoresPackage, MessageType, MessageType::ClientRequest};
    use crate::sub_lib::neighborhood::GossipFailure_0v1;
    use crate::sub_lib::peer_actors::PeerActors;
    use crate::sub_lib::proxy_client::{
        ClientResponsePayload_0v1, DnsResolveFailure_0v1, MeteredClientResponsePayload_0v1,
    };
    use crate::sub_lib::proxy_server::{ClientRequestPayload_0v1, ProxyProtocol};
    use crate::sub_lib::route::{Route, RouteSegment};
    use crate::sub_lib::sequence_buffer::SequencedPacket;
    use crate::sub_lib::service_receipt::{
        make_accounting_commitment, AuthorizedReceiptSession, ReceiptSessionPolicy, ServiceKind,
        ServiceReceipt,
    };
    use crate::sub_lib::stream_key::StreamKey;
    use crate::sub_lib::versioned_data::VersionedData;
    use crate::sub_lib::wallet::Wallet;
    use crate::test_utils::recorder::{make_recorder, peer_actors_builder};
    use crate::test_utils::unshared_test_utils::{make_request_payload, make_response_payload};
    use crate::test_utils::{
        make_meaningless_message_type, make_paying_wallet, rate_pack_routing,
        rate_pack_routing_byte, route_from_proxy_client, route_to_proxy_client,
        route_to_proxy_server,
    };
    use actix::System;
    use lazy_static::lazy_static;
    use masq_lib::test_utils::environment_guard::EnvironmentGuard;
    use masq_lib::test_utils::logging::{init_test_logging, TestLogHandler};
    use masq_lib::test_utils::utils::TEST_DEFAULT_CHAIN;
    use std::net::SocketAddr;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    lazy_static! {
        static ref CRYPTDE_PAIR: CryptDEPair = CryptDEPair::null();
    }

    fn active_receipt_window() -> (u64, u64) {
        let now_unix_s = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        (now_unix_s.saturating_sub(1), now_unix_s + 600)
    }

    #[derive(Default)]
    struct ReceiptDaoState {
        checkpoint_opt: Option<ReceiptSequenceCheckpoint>,
        routing_offer_state_opt: Option<RoutingReceiptOfferState>,
        accepted_count: usize,
    }

    struct ReceiptDaoMock {
        state: Arc<Mutex<ReceiptDaoState>>,
    }

    impl ReceiptCheckpointDao for ReceiptDaoMock {
        fn checkpoint(
            &self,
            _route_epoch: &[u8; 32],
            _provider_public_key: &PublicKey,
            _payer_session_public_key: &PublicKey,
        ) -> Result<Option<ReceiptSequenceCheckpoint>, ReceiptCheckpointDaoError> {
            Ok(self.state.lock().unwrap().checkpoint_opt.clone())
        }

        fn save_checkpoint(
            &mut self,
            checkpoint: &ReceiptSequenceCheckpoint,
        ) -> Result<(), ReceiptCheckpointDaoError> {
            self.state.lock().unwrap().checkpoint_opt = Some(checkpoint.clone());
            Ok(())
        }

        fn authorization(
            &self,
            _authorization_nonce: &[u8; 32],
        ) -> Result<Option<AuthorizedReceiptSession>, ReceiptCheckpointDaoError> {
            Ok(None)
        }

        fn save_authorization(
            &mut self,
            _authorization: &AuthorizedReceiptSession,
        ) -> Result<(), ReceiptCheckpointDaoError> {
            Ok(())
        }

        fn routing_offer_state(
            &self,
            _authorization_nonce: &[u8; 32],
            _route_epoch: &[u8; 32],
            _provider_public_key: &PublicKey,
            _payer_session_public_key: &PublicKey,
        ) -> Result<Option<RoutingReceiptOfferState>, ReceiptCheckpointDaoError> {
            Ok(self.state.lock().unwrap().routing_offer_state_opt.clone())
        }

        fn save_routing_offer_state(
            &mut self,
            state: &RoutingReceiptOfferState,
            _now_unix_s: u64,
            _maximum_states: usize,
        ) -> Result<(), ReceiptCheckpointDaoError> {
            self.state.lock().unwrap().routing_offer_state_opt = Some(state.clone());
            Ok(())
        }

        fn accept_verified_receipt(
            &mut self,
            _payload: &ServiceReceiptPayload_0v1,
            checkpoint: &ReceiptSequenceCheckpoint,
            _timestamp: SystemTime,
        ) -> Result<u128, ReceiptCheckpointDaoError> {
            let mut state = self.state.lock().unwrap();
            state.checkpoint_opt = Some(checkpoint.clone());
            state.accepted_count += 1;
            Ok(checkpoint.cumulative_charge_wei)
        }

        fn pending_settlement_claims(
            &self,
        ) -> Result<Vec<ServiceReceiptPayload_0v1>, ReceiptCheckpointDaoError> {
            Ok(vec![])
        }

        fn pending_settlement_claims_page(
            &self,
            _start_after_claim_id_opt: Option<[u8; 32]>,
            _limit: usize,
        ) -> Result<Vec<ServiceReceiptPayload_0v1>, ReceiptCheckpointDaoError> {
            Ok(vec![])
        }

        fn pending_settlement_claim_records_page(
            &self,
            _start_after_claim_id_opt: Option<[u8; 32]>,
            _limit: usize,
        ) -> Result<
            Vec<crate::accountant::db_access_objects::receipt_checkpoint_dao::PendingSettlementClaimRecord>,
            ReceiptCheckpointDaoError,
        >{
            Ok(vec![])
        }

        fn settlement_reconciliation_candidates_page(
            &self,
            _start_after_claim_id_opt: Option<[u8; 32]>,
            _limit: usize,
        ) -> Result<
            Vec<crate::accountant::db_access_objects::receipt_checkpoint_dao::SettlementReconciliationCandidate>,
            ReceiptCheckpointDaoError,
        >{
            Ok(vec![])
        }

        fn reconcile_settlement_claims(
            &mut self,
            _observation: &crate::accountant::db_access_objects::receipt_checkpoint_dao::SettlementChainObservation,
        ) -> Result<
            crate::accountant::db_access_objects::receipt_checkpoint_dao::SettlementReconciliationOutcome,
            ReceiptCheckpointDaoError,
        >{
            Ok(crate::accountant::db_access_objects::receipt_checkpoint_dao::SettlementReconciliationOutcome {
                archived_claim_count: 0,
                restored_claim_count: 0,
                still_pending_claim_count: 0,
                revalidated_archive_count: 0,
                unknown_claim_count: 0,
            })
        }

        fn provider_settlement_authorization(
            &self,
        ) -> Result<
            Option<crate::sub_lib::service_receipt::AuthorizedProviderSettlement>,
            ReceiptCheckpointDaoError,
        > {
            Ok(None)
        }

        fn save_provider_settlement_authorization(
            &mut self,
            _authorization: &crate::sub_lib::service_receipt::AuthorizedProviderSettlement,
        ) -> Result<(), ReceiptCheckpointDaoError> {
            Ok(())
        }

        fn clear_provider_settlement_authorization(
            &mut self,
        ) -> Result<(), ReceiptCheckpointDaoError> {
            Ok(())
        }
    }

    fn make_service_receipt_payload() -> ServiceReceiptPayload_0v1 {
        let payer_public_key = PublicKey::new(b"routing receipt payer session");
        let payer_cryptde = CryptDENull::from(&payer_public_key, TEST_DEFAULT_CHAIN);
        let route_epoch = [0x91; 32];
        let acknowledged_receipt = ServiceReceipt::new(
            route_epoch,
            1,
            ServiceKind::Routing,
            CRYPTDE_PAIR.main.public_key().clone(),
            make_accounting_commitment(&route_epoch, &payer_public_key),
            100,
            5,
            2,
        )
        .sign(CRYPTDE_PAIR.main.as_ref())
        .unwrap()
        .acknowledge(&payer_cryptde)
        .unwrap();
        let payer_wallet = make_paying_wallet(b"routing receipt wallet");
        let (valid_from_unix_s, expires_at_unix_s) = active_receipt_window();
        let authorization = ReceiptSessionPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
            payer_wallet.address(),
            payer_public_key,
            10_000,
            valid_from_unix_s,
            expires_at_unix_s,
            [0x92; 32],
        )
        .authorize(&payer_wallet)
        .unwrap();
        ServiceReceiptPayload_0v1 {
            authorization,
            acknowledged_receipt,
        }
    }

    #[test]
    fn routing_provider_durably_retransmits_then_advances_exact_cumulative_offers() {
        let payer_public_key = PublicKey::new(b"routing capsule payer");
        let payer_cryptde = CryptDENull::from(&payer_public_key, TEST_DEFAULT_CHAIN);
        let payer_wallet = make_paying_wallet(b"routing capsule wallet");
        let (valid_from_unix_s, expires_at_unix_s) = active_receipt_window();
        let authorization = ReceiptSessionPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
            payer_wallet.address(),
            payer_public_key.clone(),
            1_000_000,
            valid_from_unix_s,
            expires_at_unix_s,
            [0x94; 32],
        )
        .authorize(&payer_wallet)
        .unwrap();
        let request = ReceiptSessionRequest::new(authorization, [0x95; 32]).unwrap();
        let peer_actors = peer_actors_builder().build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            make_routing_service_subs(peer_actors),
            5,
            2,
            true,
        );
        let dao_state = Arc::new(Mutex::new(ReceiptDaoState::default()));
        subject.enable_service_receipts(
            Box::new(ReceiptDaoMock {
                state: dao_state.clone(),
            }),
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
        );

        let first_encrypted = subject.make_routing_receipt_offer(&request, 100).unwrap();
        let first =
            decodex::<ServiceReceiptOfferPayload_0v1>(&payer_cryptde, &first_encrypted).unwrap();
        assert_eq!(first.signed_receipt.receipt.sequence, 1);
        assert_eq!(first.signed_receipt.receipt.payload_size, 100);
        assert_eq!(first.signed_receipt.receipt.service_units, 1);
        assert_eq!(first.signed_receipt.receipt.cumulative_charge_wei, 205);
        assert_eq!(
            first
                .signed_receipt
                .verify(CRYPTDE_PAIR.main.as_ref())
                .unwrap(),
            205
        );

        let retransmitted_encrypted = subject.make_routing_receipt_offer(&request, 50).unwrap();
        let retransmitted =
            decodex::<ServiceReceiptOfferPayload_0v1>(&payer_cryptde, &retransmitted_encrypted)
                .unwrap();
        assert_eq!(retransmitted, first);

        dao_state.lock().unwrap().checkpoint_opt = Some(ReceiptSequenceCheckpoint {
            route_epoch: first.signed_receipt.receipt.route_epoch,
            provider_public_key: first.signed_receipt.receipt.provider_public_key.clone(),
            accounting_commitment: first.signed_receipt.receipt.accounting_commitment,
            payer_session_public_key: payer_public_key,
            last_sequence: first.signed_receipt.receipt.sequence,
            cumulative_charge_wei: first.signed_receipt.receipt.cumulative_charge_wei,
        });
        let second_encrypted = subject.make_routing_receipt_offer(&request, 50).unwrap();
        let second =
            decodex::<ServiceReceiptOfferPayload_0v1>(&payer_cryptde, &second_encrypted).unwrap();
        assert_eq!(second.signed_receipt.receipt.sequence, 2);
        assert_eq!(second.signed_receipt.receipt.payload_size, 50);
        assert_eq!(second.signed_receipt.receipt.cumulative_charge_wei, 310);
    }

    #[test]
    fn verified_service_receipt_is_accounted_once_and_replay_is_rejected() {
        let system =
            System::new("verified_service_receipt_is_accounted_once_and_replay_is_rejected");
        let peer_actors = peer_actors_builder().build();
        let state = Arc::new(Mutex::new(ReceiptDaoState::default()));
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            make_routing_service_subs(peer_actors),
            100,
            200,
            true,
        );
        subject.enable_service_receipts(
            Box::new(ReceiptDaoMock {
                state: Arc::clone(&state),
            }),
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
        );
        let payload = make_service_receipt_payload();

        subject.accept_service_receipt(payload.clone());
        subject.accept_service_receipt(payload);

        assert_eq!(state.lock().unwrap().accepted_count, 1);
        System::current().stop();
        system.run();
    }

    #[test]
    fn provider_accounts_a_newer_payer_acknowledgement_after_an_intermediate_ack_is_lost() {
        let system = System::new(
            "provider_accounts_a_newer_payer_acknowledgement_after_an_intermediate_ack_is_lost",
        );
        let peer_actors = peer_actors_builder().build();
        let state = Arc::new(Mutex::new(ReceiptDaoState::default()));
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            make_routing_service_subs(peer_actors),
            100,
            200,
            true,
        );
        subject.enable_service_receipts(
            Box::new(ReceiptDaoMock {
                state: Arc::clone(&state),
            }),
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
        );
        let first = make_service_receipt_payload();
        let payer = CryptDENull::from(
            &first.acknowledged_receipt.payer_session_public_key,
            TEST_DEFAULT_CHAIN,
        );
        let first_receipt = &first.acknowledged_receipt.signed_receipt.receipt;
        let mut later_receipt = first_receipt
            .next_for_same_route(3, ServiceKind::Routing, 50, 5, 2)
            .unwrap();
        later_receipt.cumulative_charge_wei += 321;
        let expected_cumulative = later_receipt.cumulative_charge_wei;
        let later = ServiceReceiptPayload_0v1 {
            authorization: first.authorization.clone(),
            acknowledged_receipt: later_receipt
                .sign(CRYPTDE_PAIR.main.as_ref())
                .unwrap()
                .acknowledge(&payer)
                .unwrap(),
        };

        subject.accept_service_receipt(first);
        subject.accept_service_receipt(later);

        let state = state.lock().unwrap();
        assert_eq!(state.accepted_count, 2);
        assert_eq!(
            state.checkpoint_opt.as_ref().unwrap().cumulative_charge_wei,
            expected_cumulative
        );
        System::current().stop();
        system.run();
    }

    #[test]
    fn dns_resolution_failures_are_reported_to_the_proxy_server() {
        let route = route_to_proxy_server(
            &CRYPTDE_PAIR.main.public_key(),
            CRYPTDE_PAIR.main.as_ref(),
            false,
        );
        let stream_key = StreamKey::make_meaningless_stream_key();
        let dns_resolve_failure = DnsResolveFailure_0v1::new(stream_key);
        let lcp = LiveCoresPackage::new(
            route,
            encodex(
                CRYPTDE_PAIR.alias.as_ref(),
                &CRYPTDE_PAIR.alias.public_key(),
                &MessageType::DnsResolveFailed(VersionedData::new(
                    &crate::sub_lib::migrations::dns_resolve_failure::MIGRATIONS,
                    &dns_resolve_failure.clone(),
                )),
            )
            .unwrap(),
        );
        let data_enc = encodex(
            CRYPTDE_PAIR.main.as_ref(),
            &CRYPTDE_PAIR.main.public_key(),
            &lcp,
        )
        .unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            sequence_number_opt: None,
            last_data: false,
            is_clandestine: false,
            data: data_enc.into(),
        };
        let (proxy_server, _, proxy_server_recording) = make_recorder();

        let system = System::new("dns_resolution_failures_are_reported_to_the_proxy_server");
        let peer_actors = peer_actors_builder().proxy_server(proxy_server).build();
        let subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );

        subject.route(inbound_client_data);

        System::current().stop();
        system.run();

        let recordings = proxy_server_recording.lock().unwrap();
        let message = recordings.get_record::<ExpiredCoresPackage<DnsResolveFailure_0v1>>(0);
        assert_eq!(dns_resolve_failure, message.payload);
    }

    #[test]
    fn service_receipt_offers_are_reported_to_the_proxy_server() {
        let route = route_to_proxy_server(
            &CRYPTDE_PAIR.main.public_key(),
            CRYPTDE_PAIR.main.as_ref(),
            false,
        );
        let offer = ServiceReceiptOfferPayload_0v1 {
            signed_receipt: make_service_receipt_payload()
                .acknowledged_receipt
                .signed_receipt,
        };
        let lcp = LiveCoresPackage::new(
            route,
            encodex(
                CRYPTDE_PAIR.alias.as_ref(),
                &CRYPTDE_PAIR.alias.public_key(),
                &MessageType::from(offer.clone()),
            )
            .unwrap(),
        );
        let data_enc = encodex(
            CRYPTDE_PAIR.main.as_ref(),
            &CRYPTDE_PAIR.main.public_key(),
            &lcp,
        )
        .unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            sequence_number_opt: None,
            last_data: false,
            is_clandestine: false,
            data: data_enc.into(),
        };
        let (proxy_server, _, proxy_server_recording) = make_recorder();
        let system = System::new("service_receipt_offers_are_reported_to_the_proxy_server");
        let peer_actors = peer_actors_builder().proxy_server(proxy_server).build();
        let subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );

        subject.route(inbound_client_data);
        System::current().stop();
        system.run();

        let recordings = proxy_server_recording.lock().unwrap();
        let message =
            recordings.get_record::<ExpiredCoresPackage<ServiceReceiptOfferPayload_0v1>>(0);
        assert_eq!(offer, message.payload);
    }

    #[test]
    fn metered_client_responses_are_forwarded_as_one_package() {
        let route = route_to_proxy_server(
            &CRYPTDE_PAIR.main.public_key(),
            CRYPTDE_PAIR.main.as_ref(),
            false,
        );
        let metered = MeteredClientResponsePayload_0v1 {
            response: make_response_payload(123),
            receipt_offer: ServiceReceiptOfferPayload_0v1 {
                signed_receipt: make_service_receipt_payload()
                    .acknowledged_receipt
                    .signed_receipt,
            },
        };
        let lcp = LiveCoresPackage::new(
            route,
            encodex(
                CRYPTDE_PAIR.alias.as_ref(),
                &CRYPTDE_PAIR.alias.public_key(),
                &MessageType::from(metered.clone()),
            )
            .unwrap(),
        );
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            sequence_number_opt: None,
            last_data: false,
            is_clandestine: false,
            data: encodex(
                CRYPTDE_PAIR.main.as_ref(),
                &CRYPTDE_PAIR.main.public_key(),
                &lcp,
            )
            .unwrap()
            .into(),
        };
        let (proxy_server, _, proxy_server_recording) = make_recorder();
        let system = System::new("metered_client_responses_are_forwarded_as_one_package");
        let peer_actors = peer_actors_builder().proxy_server(proxy_server).build();
        let subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            make_routing_service_subs(peer_actors),
            100,
            200,
            false,
        );

        subject.route(inbound_client_data);
        System::current().stop();
        system.run();

        let recordings = proxy_server_recording.lock().unwrap();
        let message =
            recordings.get_record::<ExpiredCoresPackage<MeteredClientResponsePayload_0v1>>(0);
        assert_eq!(message.payload, metered);
        assert_eq!(recordings.len(), 1);
    }

    #[test]
    fn logs_and_ignores_message_that_cannot_be_deserialized() {
        init_test_logging();
        let test_name = "logs_and_ignores_message_that_cannot_be_deserialized";
        let route = route_from_proxy_client(
            &CRYPTDE_PAIR.main.public_key(),
            CRYPTDE_PAIR.main.as_ref(),
            false,
        );
        let lcp = LiveCoresPackage::new(
            route,
            encodex(
                CRYPTDE_PAIR.main.as_ref(),
                &CRYPTDE_PAIR.main.public_key(),
                &[42u8],
            )
            .unwrap(),
        );
        let data_enc = encodex(
            CRYPTDE_PAIR.main.as_ref(),
            &CRYPTDE_PAIR.main.public_key(),
            &lcp,
        )
        .unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            sequence_number_opt: None,
            last_data: false,
            is_clandestine: false,
            data: data_enc.into(),
        };
        let peer_actors = peer_actors_builder().build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );
        subject.logger = Logger::new(test_name);

        subject.route(inbound_client_data);

        TestLogHandler::new().exists_log_containing(
            &format!("ERROR: {test_name}: Couldn't expire CORES package with 35-byte payload to ProxyClient using main key"),
        );
    }

    #[test]
    fn logs_and_ignores_message_that_cannot_be_decrypted() {
        init_test_logging();
        let test_name = "logs_and_ignores_message_that_cannot_be_decrypted";
        let main_cryptde = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let rogue_cryptde = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let route = route_from_proxy_client(main_cryptde.public_key(), &main_cryptde, false);
        let lcp = LiveCoresPackage::new(
            route,
            encodex(&rogue_cryptde, rogue_cryptde.public_key(), &[42u8]).unwrap(),
        );
        let data_enc = encodex(&main_cryptde, main_cryptde.public_key(), &lcp).unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            sequence_number_opt: None,
            last_data: false,
            is_clandestine: false,
            data: data_enc.into(),
        };
        let peer_actors = peer_actors_builder().build();
        let cryptde_pair = CryptDEPair::new(
            main_cryptde.dup(),
            Box::new(CryptDEReal::new(TEST_DEFAULT_CHAIN)),
        );
        let mut subject = RoutingService::new(
            cryptde_pair,
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );
        subject.logger = Logger::new(test_name);

        subject.route(inbound_client_data);

        TestLogHandler::new().exists_log_containing(
            &format!("ERROR: {test_name}: Couldn't expire CORES package with 51-byte payload to ProxyClient using main key: DecryptionError(OpeningFailed)")
        );
    }

    #[test]
    fn logs_and_ignores_message_that_had_invalid_destination() {
        init_test_logging();
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let route = route_from_proxy_client(&main_cryptde.public_key(), main_cryptde, false);
        let payload = GossipBuilder::empty();
        let lcp = LiveCoresPackage::new(
            route,
            encodex(
                main_cryptde,
                &main_cryptde.public_key(),
                &MessageType::Gossip(payload.into()),
            )
            .unwrap(),
        );
        let data_enc = encodex(main_cryptde, &main_cryptde.public_key(), &lcp).unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            sequence_number_opt: None,
            last_data: false,
            is_clandestine: false,
            data: data_enc.into(),
        };
        let peer_actors = peer_actors_builder().build();
        let subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );
        subject.route(inbound_client_data);
        TestLogHandler::new().exists_log_matching("Attempt to send invalid combination .* to .*");
    }

    #[test]
    fn converts_live_message_to_expired_for_existing_proxy_client() {
        let _eg = EnvironmentGuard::new();
        BAN_CACHE.clear();
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let (component, _, component_recording_arc) = make_recorder();
        let route = route_to_proxy_client(&main_cryptde.public_key(), main_cryptde, false);
        let payload = make_request_payload(0, main_cryptde);
        let lcp = LiveCoresPackage::new(
            route,
            encodex::<MessageType>(
                main_cryptde,
                &main_cryptde.public_key(),
                &payload.clone().into(),
            )
            .unwrap(),
        );
        let lcp_a = lcp.clone();
        let data_ser = PlainData::new(&serde_cbor::ser::to_vec(&lcp).unwrap()[..]);
        let data_enc = main_cryptde
            .encode(&main_cryptde.public_key(), &data_ser)
            .unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            sequence_number_opt: None,
            last_data: true,
            is_clandestine: false,
            data: data_enc.into(),
        };

        let system = System::new("converts_live_message_to_expired_for_proxy_client");
        let peer_actors = peer_actors_builder().proxy_client(component).build();
        let subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            0,
            0,
            false,
        );

        subject.route(inbound_client_data);

        System::current().stop();
        system.run();
        let component_recording = component_recording_arc.lock().unwrap();
        let record =
            component_recording.get_record::<ExpiredCoresPackage<ClientRequestPayload_0v1>>(0);
        let expected_ecp = lcp_a
            .to_expired(
                SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                main_cryptde,
                main_cryptde,
            )
            .unwrap();
        assert_eq!(record.immediate_neighbor, expected_ecp.immediate_neighbor);
        assert_eq!(record.paying_wallet, expected_ecp.paying_wallet);
        assert_eq!(record.remaining_route, expected_ecp.remaining_route);
        assert_eq!(record.payload, payload);
        assert_eq!(record.payload_len, expected_ecp.payload_len);
    }

    #[test]
    fn complains_about_live_message_for_nonexistent_proxy_client() {
        let _eg = EnvironmentGuard::new();
        init_test_logging();
        BAN_CACHE.clear();
        let test_name = "complains_about_live_message_for_nonexistent_proxy_client";
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let route = route_to_proxy_client(&main_cryptde.public_key(), main_cryptde, false);
        let payload = make_request_payload(0, main_cryptde);
        let lcp = LiveCoresPackage::new(
            route,
            encodex::<MessageType>(
                main_cryptde,
                &main_cryptde.public_key(),
                &payload.clone().into(),
            )
            .unwrap(),
        );
        let data_ser = PlainData::new(&serde_cbor::ser::to_vec(&lcp).unwrap()[..]);
        let data_enc = main_cryptde
            .encode(&main_cryptde.public_key(), &data_ser)
            .unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            sequence_number_opt: None,
            last_data: true,
            is_clandestine: false,
            data: data_enc.into(),
        };

        let system = System::new("converts_live_message_to_expired_for_proxy_client");
        let peer_actors = peer_actors_builder().build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: None,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            0,
            0,
            false,
        );
        subject.logger = Logger::new(test_name);

        subject.route(inbound_client_data);

        System::current().stop();
        system.run();
        let tlh = TestLogHandler::new();
        tlh.exists_no_log_containing("Couldn't decode CORES package in 8-byte buffer");
        tlh.exists_log_containing(&format!("WARN: {test_name}: Received CORES package from 1.2.3.4:5678 for Proxy Client, but Proxy Client isn't running"));
    }

    #[test]
    fn converts_live_message_to_expired_for_proxy_server() {
        let _eg = EnvironmentGuard::new();
        BAN_CACHE.clear();
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let alias_cryptde = CRYPTDE_PAIR.alias.as_ref();
        let (proxy_server, _, proxy_server_recording_arc) = make_recorder();
        let route = route_to_proxy_server(&main_cryptde.public_key(), main_cryptde, false);
        let payload = make_response_payload(0);
        let lcp = LiveCoresPackage::new(
            route,
            encodex::<MessageType>(
                alias_cryptde,
                &alias_cryptde.public_key(),
                &payload.clone().into(),
            )
            .unwrap(),
        );
        let lcp_a = lcp.clone();
        let lcp_enc = encodex(main_cryptde, main_cryptde.public_key(), &lcp).unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.3.2.4:5678").unwrap(),
            reception_port_opt: None,
            last_data: false,
            is_clandestine: true,
            sequence_number_opt: None,
            data: lcp_enc.into(),
        };

        let system = System::new("converts_live_message_to_expired_for_proxy_server");
        let peer_actors = peer_actors_builder().proxy_server(proxy_server).build();
        let subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            0,
            0,
            false,
        );

        subject.route(inbound_client_data);

        System::current().stop();
        system.run();
        let proxy_server_recording = proxy_server_recording_arc.lock().unwrap();
        let record =
            proxy_server_recording.get_record::<ExpiredCoresPackage<ClientResponsePayload_0v1>>(0);
        let expected_ecp = lcp_a
            .to_expired(
                SocketAddr::from_str("1.3.2.4:5678").unwrap(),
                main_cryptde,
                alias_cryptde,
            )
            .unwrap();
        assert_eq!(record.immediate_neighbor, expected_ecp.immediate_neighbor);
        assert_eq!(record.paying_wallet, expected_ecp.paying_wallet);
        assert_eq!(record.remaining_route, expected_ecp.remaining_route);
        assert_eq!(record.payload, payload);
        assert_eq!(record.payload_len, expected_ecp.payload_len);
    }

    #[test]
    fn converts_live_gossip_message_to_expired_for_neighborhood() {
        let _eg = EnvironmentGuard::new();
        BAN_CACHE.clear();
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let (component, _, component_recording_arc) = make_recorder();
        let mut route = Route::one_way(
            RouteSegment::new(
                vec![&main_cryptde.public_key(), &main_cryptde.public_key()],
                Component::Neighborhood,
            ),
            main_cryptde,
            None,
            None,
        )
        .unwrap();
        route.shift(main_cryptde).unwrap();
        let payload = GossipBuilder::empty();
        let lcp = LiveCoresPackage::new(
            route,
            encodex::<MessageType>(
                main_cryptde,
                &main_cryptde.public_key(),
                &payload.clone().into(),
            )
            .unwrap(),
        );
        let lcp_a = lcp.clone();
        let data_enc = encodex(main_cryptde, &main_cryptde.public_key(), &lcp).unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.3.2.4:5678").unwrap(),
            reception_port_opt: None,
            last_data: false,
            is_clandestine: true,
            sequence_number_opt: None,
            data: data_enc.into(),
        };

        let system = System::new("converts_live_gossip_message_to_expired_for_neighborhood");
        let peer_actors = peer_actors_builder().neighborhood(component).build();
        let subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            0,
            0,
            false,
        );

        subject.route(inbound_client_data);

        System::current().stop();
        system.run();
        let component_recording = component_recording_arc.lock().unwrap();
        let record = component_recording.get_record::<ExpiredCoresPackage<Gossip_0v1>>(0);
        let expected_ecp = lcp_a
            .to_expired(
                SocketAddr::from_str("1.3.2.4:5678").unwrap(),
                main_cryptde,
                main_cryptde,
            )
            .unwrap();
        assert_eq!(record.immediate_neighbor, expected_ecp.immediate_neighbor);
        assert_eq!(record.paying_wallet, expected_ecp.paying_wallet);
        assert_eq!(record.remaining_route, expected_ecp.remaining_route);
        assert_eq!(record.payload, payload);
        assert_eq!(record.payload_len, expected_ecp.payload_len);
    }

    #[test]
    fn converts_live_gossip_failure_message_to_expired_for_neighborhood() {
        let _eg = EnvironmentGuard::new();
        BAN_CACHE.clear();
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let (component, _, component_recording_arc) = make_recorder();
        let mut route = Route::one_way(
            RouteSegment::new(
                vec![&cryptde.public_key(), &cryptde.public_key()],
                Component::Neighborhood,
            ),
            cryptde,
            None,
            None,
        )
        .unwrap();
        route.shift(cryptde).unwrap();
        let payload = MessageType::GossipFailure(VersionedData::new(
            &crate::sub_lib::migrations::gossip_failure::MIGRATIONS,
            &GossipFailure_0v1::NoNeighbors,
        ));
        let lcp = LiveCoresPackage::new(
            route,
            encodex::<MessageType>(cryptde, &cryptde.public_key(), &payload).unwrap(),
        );
        let data_enc = encodex(cryptde, &cryptde.public_key(), &lcp).unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.3.2.4:5678").unwrap(),
            reception_port_opt: None,
            last_data: false,
            is_clandestine: true,
            sequence_number_opt: None,
            data: data_enc.into(),
        };

        let system =
            System::new("converts_live_gossip_failure_message_to_expired_for_neighborhood");
        let peer_actors = peer_actors_builder().neighborhood(component).build();
        let subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            0,
            0,
            true,
        );

        subject.route(inbound_client_data);

        System::current().stop();
        system.run();
        let component_recording = component_recording_arc.lock().unwrap();
        let record = component_recording.get_record::<ExpiredCoresPackage<GossipFailure_0v1>>(0);
        let expected_ecp = lcp
            .to_expired(
                SocketAddr::from_str("1.3.2.4:5678").unwrap(),
                cryptde,
                cryptde,
            )
            .unwrap();
        assert_eq!(record.immediate_neighbor, expected_ecp.immediate_neighbor);
        assert_eq!(record.paying_wallet, expected_ecp.paying_wallet);
        assert_eq!(record.remaining_route, expected_ecp.remaining_route);
        assert_eq!(record.payload, GossipFailure_0v1::NoNeighbors);
        assert_eq!(record.payload_len, expected_ecp.payload_len);
    }

    #[test]
    fn passes_on_inbound_client_data_not_meant_for_this_node() {
        let _eg = EnvironmentGuard::new();
        BAN_CACHE.clear();
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let paying_wallet = make_paying_wallet(b"wallet");
        let address_paying_wallet = Wallet::from(paying_wallet.address());
        let (dispatcher, _, dispatcher_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let next_key = PublicKey::new(&[65, 65, 65]);
        let contract_address = TEST_DEFAULT_CHAIN.rec().contract;
        let route = Route::one_way(
            RouteSegment::new(
                vec![&main_cryptde.public_key(), &next_key],
                Component::Neighborhood,
            ),
            main_cryptde,
            Some(paying_wallet.clone()),
            Some(contract_address.clone()),
        )
        .unwrap();
        let payload = PlainData::new(&b"abcd"[..]);
        let lcp = LiveCoresPackage::new(route, main_cryptde.encode(&next_key, &payload).unwrap());
        let lcp_a = lcp.clone();
        let data_ser = PlainData::new(&serde_cbor::ser::to_vec(&lcp).unwrap()[..]);
        let data_enc = main_cryptde
            .encode(&main_cryptde.public_key(), &data_ser)
            .unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            last_data: true,
            is_clandestine: true,
            sequence_number_opt: None,
            data: data_enc.into(),
        };

        let system = System::new("passes_on_inbound_client_data_not_meant_for_this_node");
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher)
            .accountant(accountant)
            .build();
        let subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            rate_pack_routing(103),
            rate_pack_routing_byte(103),
            false,
        );
        let before = SystemTime::now();

        subject.route(inbound_client_data);

        System::current().stop();
        system.run();
        let after = SystemTime::now();
        let dispatcher_recording = dispatcher_recording_arc.lock().unwrap();
        let record = dispatcher_recording.get_record::<TransmitDataMsg>(0);
        let expected_lcp = lcp_a.into_next_live(main_cryptde).unwrap().1;
        let expected_lcp_ser = PlainData::new(&serde_cbor::ser::to_vec(&expected_lcp).unwrap());
        let expected_lcp_enc = main_cryptde.encode(&next_key, &expected_lcp_ser).unwrap();
        assert_eq!(
            *record,
            TransmitDataMsg {
                endpoint: Endpoint::Key(next_key.clone()),
                last_data: true,
                sequence_number_opt: None,
                data: expected_lcp_enc.into(),
            }
        );
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        let message = accountant_recording.get_record::<ReportRoutingServiceProvidedMessage>(0);
        check_timestamp(before, message.timestamp, after);
        assert!(message.paying_wallet.congruent(&paying_wallet));
        assert_eq!(
            *message,
            ReportRoutingServiceProvidedMessage {
                timestamp: message.timestamp,
                paying_wallet: address_paying_wallet,
                payload_size: lcp.payload.len(),
                service_rate: rate_pack_routing(103),
                byte_rate: rate_pack_routing_byte(103),
            }
        )
    }

    #[test]
    fn receipt_metered_external_route_appends_private_offer_without_legacy_booking() {
        let _eg = EnvironmentGuard::new();
        BAN_CACHE.clear();
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let next_key = PublicKey::new(b"receipt next hop");
        let next_cryptde = CryptDENull::from(&next_key, TEST_DEFAULT_CHAIN);
        let payer_public_key = PublicKey::new(b"receipt route payer");
        let payer_cryptde = CryptDENull::from(&payer_public_key, TEST_DEFAULT_CHAIN);
        let payer_wallet = make_paying_wallet(b"receipt route paying wallet");
        let contract_address = TEST_DEFAULT_CHAIN.rec().contract;
        let (valid_from_unix_s, expires_at_unix_s) = active_receipt_window();
        let authorization = ReceiptSessionPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            contract_address,
            payer_wallet.address(),
            payer_public_key,
            1_000_000,
            valid_from_unix_s,
            expires_at_unix_s,
            [0xa1; 32],
        )
        .authorize(&payer_wallet)
        .unwrap();
        let request = ReceiptSessionRequest::new(authorization, [0xa2; 32]).unwrap();
        let route = Route::one_way(
            RouteSegment::new(
                vec![main_cryptde.public_key(), &next_key],
                Component::Neighborhood,
            ),
            main_cryptde,
            Some(payer_wallet.clone()),
            Some(contract_address),
        )
        .unwrap();
        let live_package = LiveCoresPackage::new(
            route,
            main_cryptde
                .encode(&next_key, &PlainData::new(b"metered routing payload"))
                .unwrap(),
        );
        let payload_size = live_package.payload.len() as u64;
        let (dispatcher, _, dispatcher_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let system = System::new(
            "receipt_metered_external_route_appends_private_offer_without_legacy_booking",
        );
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher)
            .accountant(accountant)
            .build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            make_routing_service_subs(peer_actors),
            5,
            2,
            true,
        );
        subject.enable_service_receipts(
            Box::new(ReceiptDaoMock {
                state: Arc::new(Mutex::new(ReceiptDaoState::default())),
            }),
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            contract_address,
        );

        subject.route_data_externally(
            live_package,
            Some(payer_wallet.as_payer(main_cryptde.public_key(), &contract_address)),
            Some(request),
            true,
        );
        System::current().stop();
        system.run();

        assert!(accountant_recording_arc.lock().unwrap().is_empty());
        let dispatcher_recording = dispatcher_recording_arc.lock().unwrap();
        let transmitted = dispatcher_recording.get_record::<TransmitDataMsg>(0);
        let forwarded =
            decodex::<LiveCoresPackage>(&next_cryptde, &CryptData::new(&transmitted.data)).unwrap();
        assert_eq!(forwarded.routing_receipt_offers.len(), 1);
        let offer = decodex::<ServiceReceiptOfferPayload_0v1>(
            &payer_cryptde,
            &forwarded.routing_receipt_offers[0],
        )
        .unwrap();
        assert_eq!(offer.signed_receipt.receipt.payload_size, payload_size);
        assert_eq!(offer.signed_receipt.receipt.service_rate, 5);
        assert_eq!(offer.signed_receipt.receipt.byte_rate, 2);
    }

    #[test]
    fn reprocesses_inbound_client_data_meant_for_this_node_and_destined_for_hopper() {
        let _eg = EnvironmentGuard::new();
        BAN_CACHE.clear();
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let paying_wallet = make_paying_wallet(b"wallet");
        let (hopper, _, hopper_recording_arc) = make_recorder();
        let route = Route::one_way(
            RouteSegment::new(
                vec![&main_cryptde.public_key(), &main_cryptde.public_key()],
                Component::Neighborhood,
            ),
            main_cryptde,
            Some(paying_wallet.clone()),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
        )
        .unwrap();
        let payload = PlainData::new(&b"abcd"[..]);
        let lcp = LiveCoresPackage::new(
            route,
            main_cryptde
                .encode(&main_cryptde.public_key(), &payload)
                .unwrap(),
        );
        let lcp_a = lcp.clone();
        let data_enc = encodex(main_cryptde, &main_cryptde.public_key(), &lcp).unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            last_data: true,
            is_clandestine: true,
            sequence_number_opt: None,
            data: data_enc.into(),
        };

        let system = System::new(
            "reprocesses_inbound_client_data_meant_for_this_node_and_destined_for_hopper",
        );
        let peer_actors = peer_actors_builder().hopper(hopper).build();
        let subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            rate_pack_routing(103),
            rate_pack_routing_byte(103),
            false,
        );
        let before = SystemTime::now();

        subject.route(inbound_client_data);

        System::current().stop();
        system.run();
        let after = SystemTime::now();
        let hopper_recording = hopper_recording_arc.lock().unwrap();
        let record = hopper_recording.get_record::<InboundClientData>(0);
        check_timestamp(before, record.timestamp, after);
        let expected_lcp = lcp_a.into_next_live(main_cryptde).unwrap().1;
        let expected_lcp_enc =
            encodex(main_cryptde, &main_cryptde.public_key(), &expected_lcp).unwrap();
        assert_eq!(
            *record,
            InboundClientData {
                timestamp: record.timestamp,
                client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                reception_port_opt: None,
                last_data: true,
                is_clandestine: true,
                sequence_number_opt: None,
                data: expected_lcp_enc.into()
            }
        );
    }

    #[test]
    fn route_logs_and_ignores_cores_package_that_demands_routing_without_paying_wallet() {
        let _eg = EnvironmentGuard::new();
        BAN_CACHE.clear();
        init_test_logging();
        let test_name =
            "route_logs_and_ignores_cores_package_that_demands_routing_without_paying_wallet";
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let origin_key = PublicKey::new(&[1, 2]);
        let origin_cryptde = CryptDENull::from(&origin_key, TEST_DEFAULT_CHAIN);
        let destination_key = PublicKey::new(&[3, 4]);
        let payload = make_meaningless_message_type(StreamKey::make_meaningless_stream_key());
        let route = Route::one_way(
            RouteSegment::new(
                vec![&origin_key, &main_cryptde.public_key(), &destination_key],
                Component::ProxyClient,
            ),
            &origin_cryptde,
            None,
            None,
        )
        .unwrap();
        let icp =
            IncipientCoresPackage::new(&origin_cryptde, route, payload, &destination_key).unwrap();
        let (lcp, _) = LiveCoresPackage::from_incipient(icp, &origin_cryptde).unwrap();
        let data_ser = PlainData::new(&serde_cbor::ser::to_vec(&lcp).unwrap()[..]);
        let data_enc = main_cryptde
            .encode(&main_cryptde.public_key(), &data_ser)
            .unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            last_data: true,
            is_clandestine: true,
            sequence_number_opt: None,
            data: data_enc.into(),
        };
        let system = System::new(test_name);
        let (proxy_client, _, proxy_client_recording_arc) = make_recorder();
        let (proxy_server, _, proxy_server_recording_arc) = make_recorder();
        let (neighborhood, _, neighborhood_recording_arc) = make_recorder();
        let (dispatcher, _, dispatcher_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let peer_actors = peer_actors_builder()
            .proxy_client(proxy_client)
            .proxy_server(proxy_server)
            .neighborhood(neighborhood)
            .dispatcher(dispatcher)
            .accountant(accountant)
            .build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            true,
        );
        subject.logger = Logger::new(test_name);

        subject.route(inbound_client_data);

        System::current().stop_with_code(0);
        system.run();
        TestLogHandler::new().exists_log_matching(
            &format!("WARN: {test_name}: Refusing to route Live CORES package with \\d+-byte payload without paying wallet"),
        );
        assert_eq!(proxy_client_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(proxy_server_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(neighborhood_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(dispatcher_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(accountant_recording_arc.lock().unwrap().len(), 0);
    }

    #[test]
    fn route_logs_and_ignores_cores_package_that_demands_proxy_client_routing_with_paying_wallet_that_cant_pay(
    ) {
        let _eg = EnvironmentGuard::new();
        BAN_CACHE.clear();
        init_test_logging();
        let test_name = "route_logs_and_ignores_cores_package_that_demands_proxy_client_routing_with_paying_wallet_that_cant_pay";
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let public_key = main_cryptde.public_key();
        let payload = ClientRequest(VersionedData::new(
            &crate::sub_lib::migrations::client_response_payload::MIGRATIONS,
            &make_request_payload(0, main_cryptde),
        ));
        let paying_wallet = Some(make_paying_wallet(b"paying wallet"));
        let contract_address = TEST_DEFAULT_CHAIN.rec().contract;
        let live_hops: Vec<LiveHop> = vec![
            LiveHop::new(
                &public_key,
                paying_wallet
                    .clone()
                    .map(|w| w.as_payer(&public_key, &contract_address)),
                Component::Hopper,
            ),
            LiveHop::new(
                &public_key,
                paying_wallet
                    .clone()
                    .map(|w| w.as_payer(&PublicKey::new(b"can't pay"), &contract_address)),
                Component::ProxyClient,
            ),
        ];
        let hops = live_hops
            .iter()
            .map(|hop| match hop.encode(&hop.public_key, main_cryptde) {
                Ok(cryptdata) => cryptdata,
                Err(e) => panic!("Couldn't encode hop: {:?}", e),
            })
            .collect();
        let route = Route { hops };
        let icp = IncipientCoresPackage::new(main_cryptde, route, payload, public_key).unwrap();
        let (lcp, _) = LiveCoresPackage::from_incipient(icp, main_cryptde).unwrap();
        let data_ser = PlainData::new(&serde_cbor::ser::to_vec(&lcp).unwrap()[..]);
        let data_enc = main_cryptde
            .encode(&main_cryptde.public_key(), &data_ser)
            .unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            last_data: true,
            is_clandestine: true,
            sequence_number_opt: None,
            data: data_enc.into(),
        };
        let system = System::new(test_name);
        let (proxy_client, _, proxy_client_recording_arc) = make_recorder();
        let (proxy_server, _, proxy_server_recording_arc) = make_recorder();
        let (neighborhood, _, neighborhood_recording_arc) = make_recorder();
        let (dispatcher, _, dispatcher_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let peer_actors = peer_actors_builder()
            .proxy_client(proxy_client)
            .proxy_server(proxy_server)
            .neighborhood(neighborhood)
            .dispatcher(dispatcher)
            .accountant(accountant)
            .build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            true,
        );
        subject.logger = Logger::new(test_name);

        subject.route(inbound_client_data);

        System::current().stop_with_code(0);
        system.run();
        TestLogHandler::new().exists_log_matching(
            &format!("WARN: {test_name}: Refusing to route Expired CORES package with \\d+-byte payload without proof of paying wallet ownership."),
        );
        assert_eq!(proxy_client_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(proxy_server_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(neighborhood_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(dispatcher_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(accountant_recording_arc.lock().unwrap().len(), 0);
    }

    #[test]
    fn route_logs_and_ignores_cores_package_that_demands_hopper_routing_with_paying_wallet_that_cant_pay(
    ) {
        let _eg = EnvironmentGuard::new();
        BAN_CACHE.clear();
        init_test_logging();
        let test_name = "route_logs_and_ignores_cores_package_that_demands_hopper_routing_with_paying_wallet_that_cant_pay";
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let current_key = main_cryptde.public_key();
        let origin_key = PublicKey::new(&[1, 2]);
        let destination_key = PublicKey::new(&[5, 6]);
        let destination_cryptde = CryptDENull::from(&destination_key, TEST_DEFAULT_CHAIN);

        let payload = ClientRequest(VersionedData::new(
            &crate::sub_lib::migrations::client_response_payload::MIGRATIONS,
            &make_request_payload(0, &destination_cryptde),
        ));
        let paying_wallet = Some(make_paying_wallet(b"paying wallet"));
        let contract_address = TEST_DEFAULT_CHAIN.rec().contract;
        let live_hops: Vec<LiveHop> = vec![
            LiveHop::new(
                &current_key,
                paying_wallet
                    .clone()
                    .map(|w| w.as_payer(&origin_key, &contract_address)),
                Component::Hopper,
            ),
            LiveHop::new(
                &destination_key,
                paying_wallet
                    .clone()
                    .map(|w| w.as_payer(&PublicKey::new(b"can't pay"), &contract_address)),
                Component::Hopper,
            ),
        ];

        let hops = live_hops
            .iter()
            .map(|hop| match hop.encode(&hop.public_key, main_cryptde) {
                Ok(cryptdata) => cryptdata,
                Err(e) => panic!("Couldn't encode hop: {:?}", e),
            })
            .collect();

        let route = Route { hops };

        let lcp = LiveCoresPackage::new(
            route,
            encodex(main_cryptde, &destination_key, &payload).unwrap(),
        );

        let system = System::new(test_name);
        let (proxy_client, _, proxy_client_recording_arc) = make_recorder();
        let (proxy_server, _, proxy_server_recording_arc) = make_recorder();
        let (neighborhood, _, neighborhood_recording_arc) = make_recorder();
        let (dispatcher, _, dispatcher_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let peer_actors = peer_actors_builder()
            .proxy_client(proxy_client)
            .proxy_server(proxy_server)
            .neighborhood(neighborhood)
            .dispatcher(dispatcher)
            .accountant(accountant)
            .build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            true,
        );
        subject.logger = Logger::new(test_name);

        subject.route_data_externally(
            lcp,
            paying_wallet.map(|w| w.as_payer(&PublicKey::new(b"can't pay"), &contract_address)),
            None,
            true,
        );

        System::current().stop_with_code(0);
        system.run();
        TestLogHandler::new().exists_log_matching(
            &format!("WARN: {test_name}: Refusing to route Live CORES package with \\d+-byte payload without proof of paying wallet ownership."),
        );
        assert_eq!(proxy_client_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(proxy_server_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(neighborhood_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(dispatcher_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(accountant_recording_arc.lock().unwrap().len(), 0);
    }

    #[test]
    fn route_logs_and_ignores_cores_package_from_delinquent_that_demands_external_routing() {
        let _eg = EnvironmentGuard::new();
        BAN_CACHE.clear();
        init_test_logging();
        let test_name =
            "route_logs_and_ignores_cores_package_from_delinquent_that_demands_external_routing";
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let paying_wallet = make_paying_wallet(b"wallet");
        let contract_address = TEST_DEFAULT_CHAIN.rec().contract;
        BAN_CACHE.insert(paying_wallet.clone());
        let (dispatcher, _, dispatcher_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let next_key = PublicKey::new(&[65, 65, 65]);
        let route = Route::one_way(
            RouteSegment::new(
                vec![&main_cryptde.public_key(), &next_key],
                Component::Neighborhood,
            ),
            main_cryptde,
            Some(paying_wallet.clone()),
            Some(contract_address.clone()),
        )
        .unwrap();
        let payload = PlainData::new(&b"abcd"[..]);
        let lcp = LiveCoresPackage::new(route, main_cryptde.encode(&next_key, &payload).unwrap());
        let data_enc = encodex(main_cryptde, &main_cryptde.public_key(), &lcp).unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            last_data: true,
            is_clandestine: true,
            sequence_number_opt: None,
            data: data_enc.into(),
        };
        let system = System::new("test");
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher)
            .accountant(accountant)
            .build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            rate_pack_routing(103),
            rate_pack_routing_byte(103),
            false,
        );
        subject.logger = Logger::new(test_name);

        subject.route(inbound_client_data);

        System::current().stop();
        system.run();

        let dispatcher_recording = dispatcher_recording_arc.lock().unwrap();
        assert_eq!(dispatcher_recording.len(), 0);
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        assert_eq!(accountant_recording.len(), 0);
        TestLogHandler::new().exists_log_containing(&format!("WARN: {test_name}: A paying wallet is delinquent; electing not to route 7-byte payload further"));
    }

    #[test]
    fn route_logs_and_ignores_cores_package_from_delinquent_that_demands_internal_routing() {
        let _eg = EnvironmentGuard::new();
        BAN_CACHE.clear();
        init_test_logging();
        let test_name =
            "route_logs_and_ignores_cores_package_from_delinquent_that_demands_internal_routing";
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let paying_wallet = make_paying_wallet(b"wallet");
        BAN_CACHE.insert(paying_wallet.clone());
        let (dispatcher, _, dispatcher_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let mut route = Route::one_way(
            RouteSegment::new(
                vec![&main_cryptde.public_key(), &main_cryptde.public_key()],
                Component::ProxyServer,
            ),
            main_cryptde,
            Some(paying_wallet.clone()),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
        )
        .unwrap();
        route.shift(main_cryptde).unwrap();
        let payload = PlainData::new(&b"abcd"[..]);
        let lcp = LiveCoresPackage::new(
            route,
            main_cryptde
                .encode(&main_cryptde.public_key(), &payload)
                .unwrap(),
        );
        let data_enc = encodex(main_cryptde, &main_cryptde.public_key(), &lcp).unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            last_data: true,
            is_clandestine: true,
            sequence_number_opt: None,
            data: data_enc.into(),
        };
        let system = System::new("test");
        let peer_actors = peer_actors_builder()
            .dispatcher(dispatcher)
            .accountant(accountant)
            .build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            rate_pack_routing(103),
            rate_pack_routing_byte(103),
            false,
        );
        subject.logger = Logger::new(test_name);

        subject.route(inbound_client_data);

        System::current().stop();
        system.run();

        let dispatcher_recording = dispatcher_recording_arc.lock().unwrap();
        assert_eq!(dispatcher_recording.len(), 0);
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        assert_eq!(accountant_recording.len(), 0);
        TestLogHandler::new().exists_log_containing(&format!("WARN: {test_name}: A paying wallet is delinquent; electing not to route 36-byte payload to ProxyServer"));
    }

    #[test]
    fn route_logs_and_ignores_inbound_client_data_that_doesnt_deserialize_properly() {
        init_test_logging();
        let test_name =
            "route_logs_and_ignores_inbound_client_data_that_doesnt_deserialize_properly";
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            last_data: true,
            is_clandestine: true,
            sequence_number_opt: None,
            data: vec![],
        };
        let system = System::new("consume_logs_error_when_given_bad_input_data");
        let (proxy_client, _, proxy_client_recording_arc) = make_recorder();
        let (proxy_server, _, proxy_server_recording_arc) = make_recorder();
        let (neighborhood, _, neighborhood_recording_arc) = make_recorder();
        let (dispatcher, _, dispatcher_recording_arc) = make_recorder();
        let peer_actors = peer_actors_builder()
            .proxy_client(proxy_client)
            .proxy_server(proxy_server)
            .neighborhood(neighborhood)
            .dispatcher(dispatcher)
            .build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );
        subject.logger = Logger::new(test_name);

        subject.route(inbound_client_data);

        System::current().stop_with_code(0);
        system.run();
        TestLogHandler::new().exists_log_containing(
            &format!("ERROR: {test_name}: Couldn't decode CORES package in 0-byte buffer from 1.2.3.4:5678: DecryptionError(EmptyData)"),
        );
        assert_eq!(proxy_client_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(proxy_server_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(neighborhood_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(dispatcher_recording_arc.lock().unwrap().len(), 0);
    }

    #[test]
    fn route_logs_and_ignores_invalid_live_cores_package() {
        init_test_logging();
        let test_name = "route_logs_and_ignores_invalid_live_cores_package";
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let lcp = LiveCoresPackage::new(Route { hops: vec![] }, CryptData::new(&[]));
        let data_ser = PlainData::new(&serde_cbor::ser::to_vec(&lcp).unwrap()[..]);
        let data_enc = main_cryptde
            .encode(&main_cryptde.public_key(), &data_ser)
            .unwrap();
        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            last_data: true,
            is_clandestine: true,
            sequence_number_opt: None,
            data: data_enc.into(),
        };
        let inbound_data_len = inbound_client_data.data.len();
        let system = System::new("consume_logs_error_when_given_bad_input_data");
        let (proxy_client, _, proxy_client_recording_arc) = make_recorder();
        let (proxy_server, _, proxy_server_recording_arc) = make_recorder();
        let (neighborhood, _, neighborhood_recording_arc) = make_recorder();
        let (dispatcher, _, dispatcher_recording_arc) = make_recorder();
        let peer_actors = peer_actors_builder()
            .proxy_client(proxy_client)
            .proxy_server(proxy_server)
            .neighborhood(neighborhood)
            .dispatcher(dispatcher)
            .build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );
        subject.logger = Logger::new(test_name);

        subject.route(inbound_client_data);

        System::current().stop_with_code(0);
        system.run();
        TestLogHandler::new().exists_log_containing(&format!(
            "ERROR: {test_name}: Invalid {inbound_data_len}-byte CORES package: RoutingError(EmptyRoute)"
        ));
        assert_eq!(proxy_client_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(proxy_server_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(neighborhood_recording_arc.lock().unwrap().len(), 0);
        assert_eq!(dispatcher_recording_arc.lock().unwrap().len(), 0);
    }

    #[test]
    fn route_data_around_again_logs_and_ignores_bad_lcp() {
        init_test_logging();
        let test_name = "route_data_around_again_logs_and_ignores_bad_lcp";
        let peer_actors = peer_actors_builder().build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );
        subject.logger = Logger::new(test_name);
        let lcp = LiveCoresPackage::new(Route { hops: vec![] }, CryptData::new(&[]));
        let ibcd = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
            reception_port_opt: None,
            last_data: true,
            is_clandestine: true,
            sequence_number_opt: None,
            data: vec![],
        };

        subject.route_data_around_again(lcp, &ibcd);

        TestLogHandler::new().exists_log_containing(&format!(
            "ERROR: {test_name}: bad zero-hop route: RoutingError(EmptyRoute)"
        ));
    }

    fn make_routing_service_subs(peer_actors: PeerActors) -> RoutingServiceSubs {
        RoutingServiceSubs {
            proxy_client_subs_opt: peer_actors.proxy_client_opt,
            proxy_server_subs: peer_actors.proxy_server,
            neighborhood_subs: peer_actors.neighborhood,
            hopper_subs: peer_actors.hopper,
            to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
            to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
        }
    }

    fn route_data_to_peripheral_component_uses_proper_key_on_payload_for_component<F>(
        payload_factory: F,
        target_component: Component,
    ) where
        F: FnOnce(&CryptDEPair) -> CryptData,
    {
        let peer_actors = peer_actors_builder().build();
        let subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            make_routing_service_subs(peer_actors),
            100,
            200,
            true,
        );
        let route = Route::single_hop(&PublicKey::new(b"1234"), subject.cryptde_pair.main.as_ref())
            .unwrap();
        let payload = payload_factory(&subject.cryptde_pair);
        let live_package = LiveCoresPackage::new(route, payload);

        subject.route_data_to_peripheral_component(
            target_component,
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            live_package,
            true,
        );

        // CryptDENull panics when you try decrypting with the wrong key; no panic means test passes
    }

    #[test]
    fn route_data_to_peripheral_component_uses_main_key_on_payload_for_proxy_client() {
        let payload_factory = |cryptde_pair: &CryptDEPair| {
            encodex(
                cryptde_pair.main.as_ref(),
                cryptde_pair.main.public_key(),
                &MessageType::ClientRequest(VersionedData::new(
                    &crate::sub_lib::migrations::client_request_payload::MIGRATIONS,
                    &ClientRequestPayload_0v1 {
                        stream_key: StreamKey::make_meaningless_stream_key(),
                        sequenced_packet: SequencedPacket::new(vec![1, 2, 3, 4], 1234, false),
                        target_hostname: "hostname".to_string(),
                        target_port: 1234,
                        protocol: ProxyProtocol::TLS,
                        originator_public_key: PublicKey::new(b"1234"),
                        dns_attempt_id_opt: None,
                        receipt_session_request_opt: None,
                    },
                )),
            )
            .unwrap()
        };
        route_data_to_peripheral_component_uses_proper_key_on_payload_for_component(
            payload_factory,
            Component::ProxyClient,
        );
    }

    #[test]
    fn route_data_to_peripheral_component_uses_alias_key_on_payload_for_proxy_server() {
        let payload_factory = |cryptde_pair: &CryptDEPair| {
            encodex(
                cryptde_pair.alias.as_ref(),
                cryptde_pair.alias.public_key(),
                &MessageType::DnsResolveFailed(VersionedData::new(
                    &crate::sub_lib::migrations::dns_resolve_failure::MIGRATIONS,
                    &DnsResolveFailure_0v1 {
                        stream_key: StreamKey::make_meaningless_stream_key(),
                        dns_attempt_id_opt: None,
                    },
                )),
            )
            .unwrap()
        };
        route_data_to_peripheral_component_uses_proper_key_on_payload_for_component(
            payload_factory,
            Component::ProxyServer,
        );
    }

    #[test]
    fn route_data_to_peripheral_component_uses_main_key_on_payload_for_neighborhood() {
        let payload_factory = |cryptde_pair: &CryptDEPair| {
            encodex(
                cryptde_pair.main.as_ref(),
                cryptde_pair.main.public_key(),
                &MessageType::GossipFailure(VersionedData::new(
                    &crate::sub_lib::migrations::gossip_failure::MIGRATIONS,
                    &GossipFailure_0v1::Unknown,
                )),
            )
            .unwrap()
        };
        route_data_to_peripheral_component_uses_proper_key_on_payload_for_component(
            payload_factory,
            Component::Neighborhood,
        );
    }

    #[test]
    fn route_data_to_peripheral_component_uses_main_key_on_payload_for_hopper() {
        let payload_factory = |cryptde_pair: &CryptDEPair| {
            encodex(
                cryptde_pair.main.as_ref(),
                cryptde_pair.main.public_key(),
                &MessageType::ClientResponse(VersionedData::new(
                    &crate::sub_lib::migrations::client_request_payload::MIGRATIONS,
                    &ClientResponsePayload_0v1 {
                        stream_key: StreamKey::make_meaningless_stream_key(),
                        sequenced_packet: SequencedPacket::new(vec![1, 2, 3, 4], 1234, false),
                    },
                )),
            )
            .unwrap()
        };
        route_data_to_peripheral_component_uses_proper_key_on_payload_for_component(
            payload_factory,
            Component::Hopper,
        );
    }

    #[test]
    fn route_expired_package_handles_unmigratable_gossip() {
        init_test_logging();
        let test_name = "route_expired_package_handles_unmigratable_gossip";
        let (neighborhood, _, neighborhood_recording_arc) = make_recorder();
        let peer_actors = peer_actors_builder().neighborhood(neighborhood).build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );
        subject.logger = Logger::new(test_name);
        let expired_package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            None,
            Route { hops: vec![] },
            MessageType::Gossip(VersionedData::test_new(dv!(0, 0), vec![])),
            0,
        );
        let system = System::new(test_name);

        subject.route_expired_package(Component::Neighborhood, expired_package, true);

        System::current().stop_with_code(0);
        system.run();
        let neighborhood_recording = neighborhood_recording_arc.lock().unwrap();
        assert_eq!(neighborhood_recording.len(), 0);
        TestLogHandler::new().exists_log_containing(
            &format!("ERROR: {test_name}: Received unmigratable Gossip: MigrationNotFound(DataVersion {{ major: 0, minor: 0 }}, DataVersion {{ major: 0, minor: 1 }})"),
        );
    }

    #[test]
    fn route_expired_package_handles_unmigratable_client_request() {
        init_test_logging();
        let (proxy_client, _, proxy_client_recording_arc) = make_recorder();
        let peer_actors = peer_actors_builder().proxy_client(proxy_client).build();
        let subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );
        let expired_package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            None,
            Route { hops: vec![] },
            MessageType::ClientRequest(VersionedData::test_new(dv!(0, 0), vec![])),
            0,
        );
        let system = System::new("route_expired_package_handles_unmigratable_client_request");

        subject.route_expired_package(Component::ProxyClient, expired_package, true);

        System::current().stop_with_code(0);
        system.run();
        let proxy_client_recording = proxy_client_recording_arc.lock().unwrap();
        assert_eq!(proxy_client_recording.len(), 0);
        TestLogHandler::new().exists_log_containing(
            "ERROR: RoutingService: Received unmigratable ClientRequestPayload: MigrationNotFound(DataVersion { major: 0, minor: 0 }, DataVersion { major: 0, minor: 1 })",
        );
    }

    #[test]
    fn route_expired_package_handles_unmigratable_client_response() {
        init_test_logging();
        let test_name = "route_expired_package_handles_unmigratable_client_response";
        let (proxy_server, _, proxy_server_recording_arc) = make_recorder();
        let peer_actors = peer_actors_builder().proxy_server(proxy_server).build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );
        subject.logger = Logger::new(test_name);
        let expired_package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            None,
            Route { hops: vec![] },
            MessageType::ClientResponse(VersionedData::test_new(dv!(0, 0), vec![])),
            0,
        );
        let system = System::new(test_name);

        subject.route_expired_package(Component::ProxyServer, expired_package, true);

        System::current().stop_with_code(0);
        system.run();
        let proxy_server_recording = proxy_server_recording_arc.lock().unwrap();
        assert_eq!(proxy_server_recording.len(), 0);
        TestLogHandler::new().exists_log_containing(
            &format!("ERROR: {test_name}: Received unmigratable ClientResponsePayload: MigrationNotFound(DataVersion {{ major: 0, minor: 0 }}, DataVersion {{ major: 0, minor: 1 }})"),
        );
    }

    #[test]
    fn route_expired_package_handles_unmigratable_dns_resolve_failure() {
        init_test_logging();
        let test_name = "route_expired_package_handles_unmigratable_dns_resolve_failure";
        let (hopper, _, hopper_recording_arc) = make_recorder();
        let peer_actors = peer_actors_builder().hopper(hopper).build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );
        subject.logger = Logger::new(test_name);
        let expired_package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            None,
            Route { hops: vec![] },
            MessageType::DnsResolveFailed(VersionedData::test_new(dv!(0, 0), vec![])),
            0,
        );
        let system = System::new(test_name);

        subject.route_expired_package(Component::ProxyServer, expired_package, true);

        System::current().stop_with_code(0);
        system.run();
        let hopper_recording = hopper_recording_arc.lock().unwrap();
        assert_eq!(hopper_recording.len(), 0);
        TestLogHandler::new().exists_log_containing(
            &format!("ERROR: {test_name}: Received unmigratable DnsResolveFailed: MigrationNotFound(DataVersion {{ major: 0, minor: 0 }}, DataVersion {{ major: 0, minor: 1 }})"),
        );
    }

    #[test]
    fn route_expired_package_handles_unmigratable_gossip_failure() {
        init_test_logging();
        let test_name = "route_expired_package_handles_unmigratable_gossip_failure";
        let (neighborhood, _, neighborhood_recording_arc) = make_recorder();
        let peer_actors = peer_actors_builder().neighborhood(neighborhood).build();
        let mut subject = RoutingService::new(
            CRYPTDE_PAIR.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: peer_actors.proxy_client_opt,
                proxy_server_subs: peer_actors.proxy_server,
                neighborhood_subs: peer_actors.neighborhood,
                hopper_subs: peer_actors.hopper,
                to_dispatcher: peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: peer_actors.accountant.report_routing_service_provided,
            },
            100,
            200,
            false,
        );
        subject.logger = Logger::new(test_name);
        let expired_package = ExpiredCoresPackage::new(
            SocketAddr::from_str("1.2.3.4:1234").unwrap(),
            None,
            Route { hops: vec![] },
            MessageType::GossipFailure(VersionedData::test_new(dv!(0, 0), vec![])),
            0,
        );
        let system = System::new("route_expired_package_handles_unmigratable_gossip_failure");

        subject.route_expired_package(Component::Neighborhood, expired_package, true);

        System::current().stop_with_code(0);
        system.run();
        let neighborhood_recording = neighborhood_recording_arc.lock().unwrap();
        assert_eq!(neighborhood_recording.len(), 0);
        TestLogHandler::new().exists_log_containing(
            &format!("ERROR: {test_name}: Received unmigratable GossipFailure: MigrationNotFound(DataVersion {{ major: 0, minor: 0 }}, DataVersion {{ major: 0, minor: 1 }})"),
        );
    }
}
