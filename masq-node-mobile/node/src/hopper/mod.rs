// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

mod consuming_service;
pub mod live_cores_package;
mod provider_settlement;
mod routing_service;

use crate::accountant::db_access_objects::receipt_checkpoint_dao::ReceiptCheckpointDaoFactory;
use crate::accountant::db_access_objects::utils::DaoFactoryReal;
use crate::bootstrapper::CryptDEPair;
use crate::database::db_initializer::DbInitializationConfig;
use crate::hopper::provider_settlement::{
    ProviderSettlementConfig, ProviderSettlementManager, ProviderSettlementStatus,
};
use crate::hopper::routing_service::RoutingServiceSubs;
use crate::sub_lib::blockchain_bridge::{
    ProviderSettlementReconciliationContext, ProviderSettlementReconciliationRequest,
    ProviderSettlementReconciliationResult,
};
use crate::sub_lib::dispatcher::InboundClientData;
use crate::sub_lib::hopper::HopperSubs;
use crate::sub_lib::hopper::IncipientCoresPackage;
use crate::sub_lib::hopper::{HopperConfig, NoLookupIncipientCoresPackage, ServiceReceiptConfig};
use crate::sub_lib::peer_actors::BindMessage;
use crate::sub_lib::utils::{handle_ui_crash_request, NODE_MAILBOX_CAPACITY};
use actix::Actor;
use actix::Addr;
use actix::Context;
use actix::Handler;
use consuming_service::ConsumingService;
use masq_lib::constants::PROVIDER_SETTLEMENT_ERROR;
use masq_lib::logger::Logger;
use masq_lib::messages::{
    FromMessageBody, ToMessageBody, UiProviderSettlementActivateRequest,
    UiProviderSettlementActivateResponse, UiProviderSettlementContractClaim,
    UiProviderSettlementExportRequest, UiProviderSettlementExportResponse,
    UiProviderSettlementProposalRequest, UiProviderSettlementProposalResponse,
    UiProviderSettlementReconcileRequest, UiProviderSettlementReconcileResponse,
    UiProviderSettlementStatusRequest, UiProviderSettlementStatusResponse,
    UiProviderSettlementStopRequest, UiProviderSettlementStopResponse,
};
use masq_lib::ui_gateway::{
    MessageBody, MessagePath, MessageTarget, NodeFromUiMessage, NodeToUiMessage,
};
use routing_service::RoutingService;
use rustc_hex::{FromHex, ToHex};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CRASH_KEY: &str = "HOPPER";

pub struct Hopper {
    cryptde_pair: CryptDEPair,
    consuming_service: Option<ConsumingService>,
    routing_service: Option<RoutingService>,
    per_routing_service: u64,
    per_routing_byte: u64,
    is_decentralized: bool,
    logger: Logger,
    crashable: bool,
    service_receipt_config_opt: Option<ServiceReceiptConfig>,
    provider_settlement_manager_opt: Option<ProviderSettlementManager>,
    ui_gateway_opt: Option<actix::Recipient<NodeToUiMessage>>,
    provider_settlement_reconciliation_sub_opt:
        Option<actix::Recipient<ProviderSettlementReconciliationRequest>>,
}

impl Actor for Hopper {
    type Context = Context<Self>;
}

impl Handler<BindMessage> for Hopper {
    type Result = ();

    fn handle(&mut self, msg: BindMessage, ctx: &mut Self::Context) -> Self::Result {
        ctx.set_mailbox_capacity(NODE_MAILBOX_CAPACITY);
        self.ui_gateway_opt = Some(msg.peer_actors.ui_gateway.node_to_ui_message_sub.clone());
        self.provider_settlement_reconciliation_sub_opt = Some(
            msg.peer_actors
                .blockchain_bridge
                .provider_settlement_reconciliation,
        );
        self.consuming_service = Some(ConsumingService::new(
            self.cryptde_pair.main.dup(),
            msg.peer_actors.dispatcher.from_dispatcher_client.clone(),
            msg.peer_actors.hopper.from_dispatcher.clone(),
        ));
        let mut routing_service = RoutingService::new(
            self.cryptde_pair.clone(),
            RoutingServiceSubs {
                proxy_client_subs_opt: msg.peer_actors.proxy_client_opt,
                proxy_server_subs: msg.peer_actors.proxy_server,
                neighborhood_subs: msg.peer_actors.neighborhood,
                hopper_subs: msg.peer_actors.hopper,
                to_dispatcher: msg.peer_actors.dispatcher.from_dispatcher_client,
                to_accountant_routing: msg.peer_actors.accountant.report_routing_service_provided,
            },
            self.per_routing_service,
            self.per_routing_byte,
            self.is_decentralized,
        );
        if let Some(config) = self.service_receipt_config_opt.take() {
            let factory = DaoFactoryReal::new(
                &config.data_directory,
                DbInitializationConfig::panic_on_migration(),
            );
            routing_service.enable_service_receipts(
                ReceiptCheckpointDaoFactory::make(&factory),
                config.chain_id,
                config.settlement_contract,
            );
            let now_unix_s = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            match ProviderSettlementManager::new(
                ProviderSettlementConfig {
                    chain: config.chain,
                    chain_id: config.chain_id,
                    settlement_contract: config.settlement_contract,
                    payout_wallet_address: config.payout_wallet_address,
                },
                self.cryptde_pair.main.dup(),
                ReceiptCheckpointDaoFactory::make(&factory),
                now_unix_s,
            ) {
                Ok(manager) => self.provider_settlement_manager_opt = Some(manager),
                Err(error) => error!(
                    self.logger,
                    "Provider settlement management is unavailable: {}", error
                ),
            }
        }
        self.routing_service = Some(routing_service);
    }
}

// TODO: Make this message return a Future, so that the Neighborhood can tell if its
// message didn't go through.
impl Handler<NoLookupIncipientCoresPackage> for Hopper {
    type Result = ();

    fn handle(
        &mut self,
        msg: NoLookupIncipientCoresPackage,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        self.consuming_service
            .as_ref()
            .expect("Hopper unbound: no ConsumingService")
            .consume_no_lookup(msg);
    }
}

// TODO: Make this message return a Future, so that the ProxyServer (or whatever) can tell if its
// message didn't go through.
impl Handler<IncipientCoresPackage> for Hopper {
    type Result = ();

    fn handle(&mut self, msg: IncipientCoresPackage, _ctx: &mut Self::Context) -> Self::Result {
        self.consuming_service
            .as_ref()
            .expect("Hopper unbound: no ConsumingService")
            .consume(msg);
    }
}

impl Handler<InboundClientData> for Hopper {
    type Result = ();

    fn handle(&mut self, msg: InboundClientData, _ctx: &mut Self::Context) -> Self::Result {
        self.routing_service
            .as_ref()
            .expect("Hopper unbound: no RoutingService")
            .route(msg);
    }
}

impl Handler<NodeFromUiMessage> for Hopper {
    type Result = ();

    fn handle(&mut self, msg: NodeFromUiMessage, _ctx: &mut Self::Context) -> Self::Result {
        let client_id = msg.client_id;
        if let Ok((request, context_id)) =
            UiProviderSettlementProposalRequest::fmb(msg.body.clone())
        {
            self.handle_provider_settlement_proposal(request, client_id, context_id)
        } else if let Ok((request, context_id)) =
            UiProviderSettlementActivateRequest::fmb(msg.body.clone())
        {
            self.handle_provider_settlement_activation(request, client_id, context_id)
        } else if let Ok((_, context_id)) = UiProviderSettlementStatusRequest::fmb(msg.body.clone())
        {
            self.handle_provider_settlement_status(client_id, context_id)
        } else if let Ok((_, context_id)) = UiProviderSettlementStopRequest::fmb(msg.body.clone()) {
            self.handle_provider_settlement_stop(client_id, context_id)
        } else if let Ok((request, context_id)) =
            UiProviderSettlementExportRequest::fmb(msg.body.clone())
        {
            self.handle_provider_settlement_export(request, client_id, context_id)
        } else if let Ok((request, context_id)) =
            UiProviderSettlementReconcileRequest::fmb(msg.body.clone())
        {
            self.handle_provider_settlement_reconcile(request, client_id, context_id)
        } else {
            handle_ui_crash_request(msg, &self.logger, self.crashable, CRASH_KEY)
        }
    }
}

impl Hopper {
    pub fn new(config: HopperConfig) -> Hopper {
        Hopper {
            cryptde_pair: config.cryptde_pair,
            consuming_service: None,
            routing_service: None,
            crashable: config.crashable,
            per_routing_service: config.per_routing_service,
            per_routing_byte: config.per_routing_byte,
            is_decentralized: config.is_decentralized,
            service_receipt_config_opt: config.service_receipt_config_opt,
            provider_settlement_manager_opt: None,
            ui_gateway_opt: None,
            provider_settlement_reconciliation_sub_opt: None,
            logger: Logger::new("Hopper"),
        }
    }

    fn unix_time_now() -> Result<u64, String> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .map_err(|_| "system clock is before the Unix epoch".to_string())
    }

    fn send_ui_response<T: ToMessageBody>(&self, response: T, client_id: u64, context_id: u64) {
        self.ui_gateway_opt
            .as_ref()
            .expect("Hopper is unbound: no UiGateway")
            .try_send(NodeToUiMessage {
                target: MessageTarget::ClientId(client_id),
                body: response.tmb(context_id),
            })
            .expect("UiGateway is dead");
    }

    fn send_provider_settlement_error(
        &self,
        opcode: &str,
        client_id: u64,
        context_id: u64,
        message: String,
    ) {
        self.ui_gateway_opt
            .as_ref()
            .expect("Hopper is unbound: no UiGateway")
            .try_send(NodeToUiMessage {
                target: MessageTarget::ClientId(client_id),
                body: MessageBody {
                    opcode: opcode.to_string(),
                    path: MessagePath::Conversation(context_id),
                    payload: Err((PROVIDER_SETTLEMENT_ERROR, message)),
                },
            })
            .expect("UiGateway is dead");
    }

    fn parse_claim_id(value: &str, description: &str) -> Result<[u8; 32], String> {
        let bytes: Vec<u8> = value
            .strip_prefix("0x")
            .unwrap_or(value)
            .from_hex()
            .map_err(|error| format!("invalid {}: {:?}", description, error))?;
        if bytes.len() != 32 {
            return Err(format!(
                "{} must contain 32 bytes, received {}",
                description,
                bytes.len()
            ));
        }
        let mut claim_id = [0u8; 32];
        claim_id.copy_from_slice(&bytes);
        Ok(claim_id)
    }

    fn provider_settlement_status_response(
        status: ProviderSettlementStatus,
    ) -> UiProviderSettlementStatusResponse {
        let authorization_opt = status.authorization_opt;
        let policy_opt = authorization_opt
            .as_ref()
            .map(|authorization| &authorization.policy);
        UiProviderSettlementStatusResponse {
            active: authorization_opt.is_some(),
            protocol_version_opt: policy_opt.map(|policy| policy.protocol_version),
            chain_name_opt: status.chain_name_opt,
            chain_id_opt: policy_opt.map(|policy| policy.chain_id),
            masq_token_contract_opt: status
                .masq_token_contract_opt
                .map(|address| format!("{:#x}", address)),
            settlement_contract_opt: policy_opt
                .map(|policy| format!("{:#x}", policy.settlement_contract)),
            payout_wallet_address_opt: policy_opt
                .map(|policy| format!("{:#x}", policy.payout_wallet_address)),
            provider_public_key_opt: policy_opt.map(|policy| {
                format!(
                    "0x{}",
                    policy.provider_public_key.as_slice().to_hex::<String>()
                )
            }),
            authorization_id_opt: policy_opt.map(|policy| {
                format!(
                    "0x{}",
                    policy.authorization_nonce.as_ref().to_hex::<String>()
                )
            }),
            valid_from_unix_s_opt: policy_opt.map(|policy| policy.valid_from_unix_s),
            expires_at_unix_s_opt: policy_opt.map(|policy| policy.expires_at_unix_s),
            pending_claim_count: status.pending_claim_count,
        }
    }

    fn handle_provider_settlement_proposal(
        &mut self,
        request: UiProviderSettlementProposalRequest,
        client_id: u64,
        context_id: u64,
    ) {
        let result = Self::unix_time_now().and_then(|now_unix_s| {
            self.provider_settlement_manager_opt
                .as_mut()
                .ok_or_else(|| "provider settlement is unavailable in this Node mode".to_string())?
                .propose(request.duration_seconds, now_unix_s)
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(proposal) => self.send_ui_response(
                UiProviderSettlementProposalResponse {
                    proposal_id: proposal.proposal_id,
                    protocol_version: proposal.policy.protocol_version,
                    chain_name: proposal.chain_name,
                    chain_id: proposal.policy.chain_id,
                    masq_token_contract: format!("{:#x}", proposal.masq_token_contract),
                    settlement_contract: format!("{:#x}", proposal.policy.settlement_contract),
                    payout_wallet_address: format!("{:#x}", proposal.policy.payout_wallet_address),
                    provider_public_key: format!(
                        "0x{}",
                        proposal
                            .policy
                            .provider_public_key
                            .as_slice()
                            .to_hex::<String>()
                    ),
                    authorization_id: format!(
                        "0x{}",
                        proposal
                            .policy
                            .authorization_nonce
                            .as_ref()
                            .to_hex::<String>()
                    ),
                    valid_from_unix_s: proposal.policy.valid_from_unix_s,
                    expires_at_unix_s: proposal.policy.expires_at_unix_s,
                    eip712_typed_data: proposal.eip712_typed_data,
                },
                client_id,
                context_id,
            ),
            Err(error) => self.send_provider_settlement_error(
                UiProviderSettlementProposalRequest::type_opcode(),
                client_id,
                context_id,
                error,
            ),
        }
    }

    fn handle_provider_settlement_activation(
        &mut self,
        request: UiProviderSettlementActivateRequest,
        client_id: u64,
        context_id: u64,
    ) {
        let result = Self::unix_time_now().and_then(|now_unix_s| {
            self.provider_settlement_manager_opt
                .as_mut()
                .ok_or_else(|| "provider settlement is unavailable in this Node mode".to_string())?
                .activate(&request.proposal_id, &request.wallet_signature, now_unix_s)
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(status) => self.send_ui_response(
                UiProviderSettlementActivateResponse {
                    status: Self::provider_settlement_status_response(status),
                },
                client_id,
                context_id,
            ),
            Err(error) => self.send_provider_settlement_error(
                UiProviderSettlementActivateRequest::type_opcode(),
                client_id,
                context_id,
                error,
            ),
        }
    }

    fn handle_provider_settlement_status(&mut self, client_id: u64, context_id: u64) {
        let result = Self::unix_time_now().and_then(|now_unix_s| {
            self.provider_settlement_manager_opt
                .as_mut()
                .ok_or_else(|| "provider settlement is unavailable in this Node mode".to_string())?
                .status(now_unix_s)
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(status) => self.send_ui_response(
                Self::provider_settlement_status_response(status),
                client_id,
                context_id,
            ),
            Err(error) => self.send_provider_settlement_error(
                UiProviderSettlementStatusRequest::type_opcode(),
                client_id,
                context_id,
                error,
            ),
        }
    }

    fn handle_provider_settlement_stop(&mut self, client_id: u64, context_id: u64) {
        let result = self
            .provider_settlement_manager_opt
            .as_mut()
            .ok_or_else(|| "provider settlement is unavailable in this Node mode".to_string())
            .and_then(|manager| manager.stop().map_err(|error| error.to_string()));
        match result {
            Ok(status) => self.send_ui_response(
                UiProviderSettlementStopResponse {
                    status: Self::provider_settlement_status_response(status),
                },
                client_id,
                context_id,
            ),
            Err(error) => self.send_provider_settlement_error(
                UiProviderSettlementStopRequest::type_opcode(),
                client_id,
                context_id,
                error,
            ),
        }
    }

    fn handle_provider_settlement_export(
        &mut self,
        request: UiProviderSettlementExportRequest,
        client_id: u64,
        context_id: u64,
    ) {
        let result = Self::unix_time_now().and_then(|now_unix_s| {
            let start_after_claim_id_opt = request
                .start_after_claim_id_opt
                .as_ref()
                .map(|value| Self::parse_claim_id(value, "start-after claim ID"))
                .transpose()?;
            let export = self
                .provider_settlement_manager_opt
                .as_mut()
                .ok_or_else(|| "provider settlement is unavailable in this Node mode".to_string())?
                .export(start_after_claim_id_opt, request.max_claims, now_unix_s)
                .map_err(|error| error.to_string())?;
            let batch_cbor = serde_cbor::to_vec(&export.batch)
                .map_err(|error| format!("cannot serialize settlement batch: {}", error))?;
            Ok(UiProviderSettlementExportResponse {
                total_pending_claims: export.total_pending_claims,
                start_after_claim_id_opt: export
                    .start_after_claim_id_opt
                    .map(|claim_id| format!("0x{}", claim_id.to_hex::<String>())),
                next_cursor: format!("0x{}", export.next_cursor.to_hex::<String>()),
                exported_claim_count: export.batch.contract_claims.len(),
                chain_id: export.batch.chain_id,
                settlement_contract: format!("{:#x}", export.batch.settlement_contract),
                merkle_root: format!("0x{}", export.batch.merkle_root.to_hex::<String>()),
                contract_merkle_root: format!(
                    "0x{}",
                    export.batch.contract_merkle_root.to_hex::<String>()
                ),
                total_claimed_wei: export.batch.total_claimed_wei.to_string(),
                batch_cbor: format!("0x{}", batch_cbor.to_hex::<String>()),
                contract_claims: export
                    .batch
                    .contract_claims
                    .iter()
                    .map(|claim| UiProviderSettlementContractClaim {
                        claim_id: format!("0x{}", claim.claim_id.to_hex::<String>()),
                        session_id: format!("0x{}", claim.session_id.to_hex::<String>()),
                        payer_wallet_address: format!("{:#x}", claim.payer_wallet_address),
                        payout_wallet_address: format!("{:#x}", claim.payout_wallet_address),
                        cumulative_charge_wei: claim.cumulative_charge_wei.to_string(),
                    })
                    .collect(),
            })
        });
        match result {
            Ok(response) => self.send_ui_response(response, client_id, context_id),
            Err(error) => self.send_provider_settlement_error(
                UiProviderSettlementExportRequest::type_opcode(),
                client_id,
                context_id,
                error,
            ),
        }
    }

    fn handle_provider_settlement_reconcile(
        &mut self,
        request: UiProviderSettlementReconcileRequest,
        client_id: u64,
        context_id: u64,
    ) {
        let result = request
            .start_after_claim_id_opt
            .as_ref()
            .map(|value| Self::parse_claim_id(value, "start-after claim ID"))
            .transpose()
            .and_then(|start_after_claim_id_opt| {
                self.provider_settlement_manager_opt
                    .as_ref()
                    .ok_or_else(|| {
                        "provider settlement is unavailable in this Node mode".to_string()
                    })?
                    .reconciliation_page(
                        start_after_claim_id_opt,
                        request.max_claims,
                        request.confirmation_depth,
                    )
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(page) => {
                self.provider_settlement_reconciliation_sub_opt
                    .as_ref()
                    .expect("Hopper is unbound: no BlockchainBridge")
                    .try_send(ProviderSettlementReconciliationRequest {
                        settlement_contract: page.settlement_contract,
                        claim_ids: page
                            .candidates
                            .iter()
                            .map(|candidate| candidate.claim_id)
                            .collect(),
                        confirmation_depth: request.confirmation_depth,
                        response_context: ProviderSettlementReconciliationContext {
                            client_id,
                            context_id,
                            start_after_claim_id_opt: page.start_after_claim_id_opt,
                            next_cursor_opt: Some(page.next_cursor),
                            candidate_count: page.candidates.len(),
                        },
                    })
                    .expect("BlockchainBridge is dead");
            }
            Err(error) => self.send_provider_settlement_error(
                UiProviderSettlementReconcileRequest::type_opcode(),
                client_id,
                context_id,
                error,
            ),
        }
    }

    pub fn make_subs_from(addr: &Addr<Hopper>) -> HopperSubs {
        HopperSubs {
            bind: recipient!(addr, BindMessage),
            from_hopper_client: recipient!(addr, IncipientCoresPackage),
            from_hopper_client_no_lookup: recipient!(addr, NoLookupIncipientCoresPackage),
            from_dispatcher: recipient!(addr, InboundClientData),
            node_from_ui: recipient!(addr, NodeFromUiMessage),
            provider_settlement_reconciliation_result: recipient!(
                addr,
                ProviderSettlementReconciliationResult
            ),
        }
    }
}

impl Handler<ProviderSettlementReconciliationResult> for Hopper {
    type Result = ();

    fn handle(
        &mut self,
        msg: ProviderSettlementReconciliationResult,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let response_context = msg.response_context;
        let result = msg.result.and_then(|observation| {
            let outcome = self
                .provider_settlement_manager_opt
                .as_mut()
                .ok_or_else(|| "provider settlement is unavailable in this Node mode".to_string())?
                .reconcile(&observation)
                .map_err(|error| error.to_string())?;
            Ok(UiProviderSettlementReconcileResponse {
                start_after_claim_id_opt: response_context
                    .start_after_claim_id_opt
                    .map(|claim_id| format!("0x{}", claim_id.to_hex::<String>())),
                next_cursor: response_context
                    .next_cursor_opt
                    .map(|claim_id| format!("0x{}", claim_id.to_hex::<String>()))
                    .unwrap_or_else(|| "[end]".to_string()),
                queried_claim_count: response_context.candidate_count,
                chain_id: observation.chain_id,
                settlement_contract: format!("{:#x}", observation.settlement_contract),
                confirmation_depth: observation.confirmation_depth,
                latest_block_number: observation.latest_block_number,
                observed_block_number: observation.observed_block_number,
                observed_block_hash: format!(
                    "0x{}",
                    observation.observed_block_hash.to_hex::<String>()
                ),
                archived_claim_count: outcome.archived_claim_count,
                restored_claim_count: outcome.restored_claim_count,
                still_pending_claim_count: outcome.still_pending_claim_count,
                revalidated_archive_count: outcome.revalidated_archive_count,
                unknown_claim_count: outcome.unknown_claim_count,
            })
        });
        match result {
            Ok(response) => self.send_ui_response(
                response,
                response_context.client_id,
                response_context.context_id,
            ),
            Err(error) => self.send_provider_settlement_error(
                UiProviderSettlementReconcileRequest::type_opcode(),
                response_context.client_id,
                response_context.context_id,
                error,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::live_cores_package::LiveCoresPackage;
    use super::*;
    use crate::sub_lib::cryptde::PlainData;
    use crate::sub_lib::cryptde::PublicKey;
    use crate::sub_lib::dispatcher::Component;
    use crate::sub_lib::hopper::IncipientCoresPackage;
    use crate::sub_lib::route::Route;
    use crate::sub_lib::route::RouteSegment;
    use crate::sub_lib::stream_key::StreamKey;
    use crate::test_utils::unshared_test_utils::prove_that_crash_request_handler_is_hooked_up;
    use crate::test_utils::{
        make_meaningless_message_type, make_paying_wallet, route_to_proxy_client,
    };
    use actix::Actor;
    use actix::System;
    use lazy_static::lazy_static;
    use masq_lib::test_utils::utils::TEST_DEFAULT_CHAIN;
    use std::net::SocketAddr;
    use std::str::FromStr;
    use std::time::SystemTime;

    lazy_static! {
        static ref CRYPTDE_PAIR: CryptDEPair = CryptDEPair::null();
    }

    #[test]
    fn constants_have_correct_values() {
        assert_eq!(CRASH_KEY, "HOPPER");
    }

    #[test]
    #[should_panic(expected = "Hopper unbound: no RoutingService")]
    fn panics_if_routing_service_is_unbound() {
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let client_addr = SocketAddr::from_str("1.2.3.4:5678").unwrap();
        let route = route_to_proxy_client(&main_cryptde.public_key(), main_cryptde, false);
        let stream_key = StreamKey::make_meaningless_stream_key();
        let serialized_payload =
            serde_cbor::ser::to_vec(&make_meaningless_message_type(stream_key)).unwrap();
        let data = main_cryptde
            .encode(
                &main_cryptde.public_key(),
                &PlainData::new(&serialized_payload[..]),
            )
            .unwrap();
        let live_package = LiveCoresPackage::new(route, data);
        let live_data = PlainData::new(&serde_cbor::ser::to_vec(&live_package).unwrap()[..]);
        let encrypted_package = main_cryptde
            .encode(&main_cryptde.public_key(), &live_data)
            .unwrap()
            .into();

        let inbound_client_data = InboundClientData {
            timestamp: SystemTime::now(),
            client_addr,
            reception_port_opt: None,
            last_data: false,
            is_clandestine: false,
            sequence_number_opt: None,
            data: encrypted_package,
        };
        let system = System::new("panics_if_routing_service_is_unbound");
        let subject = Hopper::new(HopperConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            per_routing_service: 100,
            per_routing_byte: 200,
            is_decentralized: false,
            crashable: false,
            service_receipt_config_opt: None,
        });
        let subject_addr = subject.start();

        subject_addr.try_send(inbound_client_data).unwrap();

        System::current().stop_with_code(0);
        system.run();
    }

    #[test]
    #[should_panic(expected = "Hopper unbound: no ConsumingService")]
    fn panics_if_consuming_service_is_unbound() {
        let main_cryptde = CRYPTDE_PAIR.main.as_ref();
        let paying_wallet = make_paying_wallet(b"wallet");
        let next_key = PublicKey::new(&[65, 65, 65]);
        let route = Route::one_way(
            RouteSegment::new(
                vec![&main_cryptde.public_key(), &next_key],
                Component::Neighborhood,
            ),
            main_cryptde,
            Some(paying_wallet),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
        )
        .unwrap();
        let incipient_package = IncipientCoresPackage::new(
            main_cryptde,
            route,
            make_meaningless_message_type(StreamKey::make_meaningless_stream_key()),
            &main_cryptde.public_key(),
        )
        .unwrap();
        let system = System::new("panics_if_consuming_service_is_unbound");
        let subject = Hopper::new(HopperConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            per_routing_service: 100,
            per_routing_byte: 200,
            is_decentralized: false,
            crashable: false,
            service_receipt_config_opt: None,
        });
        let subject_addr = subject.start();

        subject_addr.try_send(incipient_package).unwrap();

        System::current().stop_with_code(0);
        system.run();
    }

    #[test]
    #[should_panic(
        expected = "panic message (processed with: node_lib::sub_lib::utils::crash_request_analyzer)"
    )]
    fn hopper_can_be_crashed_properly_but_not_improperly() {
        let hopper = Hopper::new(HopperConfig {
            cryptde_pair: CRYPTDE_PAIR.clone(),
            per_routing_service: 100,
            per_routing_byte: 200,
            is_decentralized: false,
            crashable: true,
            service_receipt_config_opt: None,
        });

        prove_that_crash_request_handler_is_hooked_up(hopper, CRASH_KEY);
    }
}
