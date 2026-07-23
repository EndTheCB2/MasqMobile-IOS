// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::sub_lib::cryptde::{decodex, CryptDE, CryptData, PublicKey};
use crate::sub_lib::cryptde_real::CryptDEReal;
use crate::sub_lib::receipt_settlement::receipt_session_contract_id;
use crate::sub_lib::service_receipt::{
    AuthorizedReceiptSession, ReceiptSequenceCheckpoint, ReceiptSessionPolicy,
    ReceiptSessionRequest, ServiceKind, ServiceReceiptError, ServiceReceiptOfferPayload_0v1,
    ServiceReceiptPayload_0v1, SignedServiceReceipt,
};
pub use crate::sub_lib::service_receipt::{
    MAX_RECEIPT_SESSION_CHARGE_WEI, MAX_RECEIPT_SESSION_DURATION_SECONDS,
    MIN_RECEIPT_SESSION_DURATION_SECONDS,
};
use crate::sub_lib::stream_key::StreamKey;
use ethereum_types::Address;
use ethsign::Signature;
use masq_lib::blockchains::chains::Chain;
use rustc_hex::{FromHex, ToHex};
use serde_derive::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::{Debug, Formatter};

pub const MAX_RECEIPT_SESSION_ROUTES: usize = 4096;
pub const MAX_PENDING_RECEIPT_RESPONSES_PER_ROUTE: usize = 64;
const SECP256K1_HALF_ORDER: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b, 0x20, 0xa0,
];

#[derive(Clone)]
pub struct ReceiptSessionConfig {
    pub chain: Chain,
    pub chain_id: u64,
    pub settlement_contract: Address,
    pub payer_wallet_address: Address,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReceiptSessionProposal {
    pub proposal_id: String,
    pub chain_name: String,
    pub masq_token_contract: Address,
    pub authorization_id: [u8; 32],
    pub policy: ReceiptSessionPolicy,
    pub eip712_typed_data: Value,
}

impl Debug for ReceiptSessionProposal {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceiptSessionProposal { proposal_data: [REDACTED] }")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReceiptSessionStatus {
    pub protocol_version_opt: Option<u16>,
    pub chain_name_opt: Option<String>,
    pub chain_id_opt: Option<u64>,
    pub masq_token_contract_opt: Option<Address>,
    pub settlement_contract_opt: Option<Address>,
    pub payer_wallet_address_opt: Option<Address>,
    pub payer_session_public_key_opt: Option<PublicKey>,
    pub authorization_id_opt: Option<[u8; 32]>,
    pub max_total_charge_wei_opt: Option<u128>,
    pub spent_charge_wei_opt: Option<u128>,
    pub valid_from_unix_s_opt: Option<u64>,
    pub expires_at_unix_s_opt: Option<u64>,
}

impl Debug for ReceiptSessionStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ReceiptSessionStatus {{ active: {}, session_data: [REDACTED] }}",
            self.is_active()
        )
    }
}

impl ReceiptSessionStatus {
    pub fn inactive() -> Self {
        Self {
            protocol_version_opt: None,
            chain_name_opt: None,
            chain_id_opt: None,
            masq_token_contract_opt: None,
            settlement_contract_opt: None,
            payer_wallet_address_opt: None,
            payer_session_public_key_opt: None,
            authorization_id_opt: None,
            max_total_charge_wei_opt: None,
            spent_charge_wei_opt: None,
            valid_from_unix_s_opt: None,
            expires_at_unix_s_opt: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.payer_session_public_key_opt.is_some()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum ReceiptSessionManagerError {
    Amount(String),
    Duration(u64),
    Expired,
    NoPendingProposal,
    NoActiveSession,
    ProposalMismatch,
    ReceiptObservationMismatch,
    ReceiptQuoteUnavailable,
    ReceiptRouteNotFound,
    Signature(String),
    Persistence(String),
    TimeOverflow,
    Verification(ServiceReceiptError),
}

impl Debug for ReceiptSessionManagerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Amount(_) => f.write_str("Amount([REDACTED])"),
            Self::Duration(value) => f.debug_tuple("Duration").field(value).finish(),
            Self::Expired => f.write_str("Expired"),
            Self::NoPendingProposal => f.write_str("NoPendingProposal"),
            Self::NoActiveSession => f.write_str("NoActiveSession"),
            Self::ProposalMismatch => f.write_str("ProposalMismatch"),
            Self::ReceiptObservationMismatch => f.write_str("ReceiptObservationMismatch"),
            Self::ReceiptQuoteUnavailable => f.write_str("ReceiptQuoteUnavailable"),
            Self::ReceiptRouteNotFound => f.write_str("ReceiptRouteNotFound"),
            Self::Signature(_) => f.write_str("Signature([REDACTED])"),
            Self::Persistence(_) => f.write_str("Persistence([REDACTED])"),
            Self::TimeOverflow => f.write_str("TimeOverflow"),
            Self::Verification(error) => f.debug_tuple("Verification").field(error).finish(),
        }
    }
}

impl std::fmt::Display for ReceiptSessionManagerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Amount(_) => write!(
                formatter,
                "maxTotalChargeWei must be a positive decimal no greater than the accounting limit"
            ),
            Self::Duration(value) => write!(
                formatter,
                "durationSeconds {} is outside the allowed {}..={} second range",
                value, MIN_RECEIPT_SESSION_DURATION_SECONDS, MAX_RECEIPT_SESSION_DURATION_SECONDS
            ),
            Self::Expired => write!(formatter, "receipt-session proposal has expired"),
            Self::NoPendingProposal => write!(formatter, "no receipt-session proposal is pending"),
            Self::NoActiveSession => write!(formatter, "no authorized receipt session is active"),
            Self::ProposalMismatch => {
                write!(formatter, "receipt-session proposal ID does not match")
            }
            Self::ReceiptObservationMismatch => write!(
                formatter,
                "receipt offer does not match the next response observed by the consumer"
            ),
            Self::ReceiptQuoteUnavailable => write!(
                formatter,
                "receipt offer arrived before its route quote was bound"
            ),
            Self::ReceiptRouteNotFound => {
                write!(
                    formatter,
                    "receipt offer does not belong to an active request"
                )
            }
            Self::Signature(_) => write!(formatter, "invalid wallet signature"),
            Self::Persistence(_) => write!(formatter, "receipt-session recovery failed"),
            Self::TimeOverflow => write!(
                formatter,
                "receipt-session expiry exceeds the supported time range"
            ),
            Self::Verification(error) => write!(
                formatter,
                "receipt-session authorization failed: {:?}",
                error
            ),
        }
    }
}

struct PendingReceiptSession {
    proposal_id: String,
    policy: ReceiptSessionPolicy,
    payer_cryptde: Box<dyn CryptDE>,
}

struct ActiveReceiptSession {
    authorization: AuthorizedReceiptSession,
    payer_cryptde: Box<dyn CryptDE>,
    routes: Vec<ReceiptRouteState>,
    spent_charge_wei: u128,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PersistedReceiptSessionHeader {
    pub(crate) authorization: AuthorizedReceiptSession,
    pub(crate) payer_cryptde: String,
    #[serde(with = "crate::sub_lib::service_receipt::u128_be")]
    pub(crate) spent_charge_wei: u128,
}

impl Debug for PersistedReceiptSessionHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("PersistedReceiptSessionHeader { recovery_header: [REDACTED] }")
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ReceiptRouteState {
    pub(crate) stream_key: StreamKey,
    pub(crate) request: ReceiptSessionRequest,
    pub(crate) expected_provider_public_key_opt: Option<PublicKey>,
    pub(crate) exit_service_rate_opt: Option<u64>,
    pub(crate) exit_byte_rate_opt: Option<u64>,
    pub(crate) observed_responses: VecDeque<(u64, u64, u64)>,
    pub(crate) pending_request_payload_size: u64,
    pub(crate) pending_request_service_units: u64,
    pub(crate) checkpoint_opt: Option<ReceiptSequenceCheckpoint>,
    pub(crate) acknowledged_payload_opt: Option<ServiceReceiptPayload_0v1>,
    pub(crate) routing_request_providers: Vec<PublicKey>,
    pub(crate) routing_response_providers: Vec<PublicKey>,
    pub(crate) routing_provider_states: HashMap<PublicKey, RoutingProviderReceiptState>,
}

impl Debug for ReceiptRouteState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceiptRouteState { route_state: [REDACTED] }")
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RoutingProviderReceiptState {
    pub(crate) service_rate: u64,
    pub(crate) byte_rate: u64,
    pub(crate) observed_payloads: VecDeque<u64>,
    pub(crate) checkpoint_opt: Option<ReceiptSequenceCheckpoint>,
    pub(crate) acknowledged_payload_opt: Option<ServiceReceiptPayload_0v1>,
}

impl Debug for RoutingProviderReceiptState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("RoutingProviderReceiptState { provider_state: [REDACTED] }")
    }
}

pub(crate) trait ReceiptSessionRecoveryStore: Send {
    fn load(
        &mut self,
        now_unix_s: u64,
    ) -> Result<Option<(PersistedReceiptSessionHeader, Vec<ReceiptRouteState>)>, String>;
    fn save_header(&mut self, header: &PersistedReceiptSessionHeader) -> Result<(), String>;
    fn save_header_and_route(
        &mut self,
        header: &PersistedReceiptSessionHeader,
        route: &ReceiptRouteState,
    ) -> Result<(), String>;
    fn clear(&mut self) -> Result<(), String>;
}

#[derive(Clone, Eq, PartialEq)]
pub struct RoutingReceiptQuote {
    pub provider_public_key: PublicKey,
    pub service_rate: u64,
    pub byte_rate: u64,
}

impl Debug for RoutingReceiptQuote {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("RoutingReceiptQuote { quote_data: [REDACTED] }")
    }
}

pub struct ReceiptSessionManager {
    config: ReceiptSessionConfig,
    pending_opt: Option<PendingReceiptSession>,
    active_opt: Option<ActiveReceiptSession>,
    recovery_store_opt: Option<Box<dyn ReceiptSessionRecoveryStore>>,
    recovery_required: bool,
}

impl ReceiptSessionManager {
    fn fresh_route_request(
        active: &ActiveReceiptSession,
    ) -> Result<ReceiptSessionRequest, ReceiptSessionManagerError> {
        let mut route_epoch = [0u8; 32];
        active.payer_cryptde.random(&mut route_epoch);
        if route_epoch.iter().all(|byte| *byte == 0)
            || active
                .routes
                .iter()
                .any(|route| route.request.route_epoch == route_epoch)
        {
            return Err(ReceiptSessionManagerError::Signature(
                "secure route-epoch generation returned an invalid value".to_string(),
            ));
        }
        ReceiptSessionRequest::new(active.authorization.clone(), route_epoch)
            .map_err(ReceiptSessionManagerError::Verification)
    }

    fn route_state(stream_key: StreamKey, request: ReceiptSessionRequest) -> ReceiptRouteState {
        ReceiptRouteState {
            stream_key,
            request,
            expected_provider_public_key_opt: None,
            exit_service_rate_opt: None,
            exit_byte_rate_opt: None,
            observed_responses: VecDeque::new(),
            pending_request_payload_size: 0,
            pending_request_service_units: 0,
            checkpoint_opt: None,
            acknowledged_payload_opt: None,
            routing_request_providers: vec![],
            routing_response_providers: vec![],
            routing_provider_states: HashMap::new(),
        }
    }

    fn persisted_header(active: &ActiveReceiptSession) -> PersistedReceiptSessionHeader {
        PersistedReceiptSessionHeader {
            authorization: active.authorization.clone(),
            payer_cryptde: active.payer_cryptde.to_string(),
            spent_charge_wei: active.spent_charge_wei,
        }
    }

    fn restore_active(
        config: &ReceiptSessionConfig,
        header: PersistedReceiptSessionHeader,
        routes: Vec<ReceiptRouteState>,
        now_unix_s: u64,
    ) -> Result<ActiveReceiptSession, ReceiptSessionManagerError> {
        if routes.len() > MAX_RECEIPT_SESSION_ROUTES {
            return Err(ReceiptSessionManagerError::Persistence(
                "recovered route limit is invalid".to_string(),
            ));
        }
        let payer_cryptde = CryptDEReal::new(config.chain)
            .make_from_str(&header.payer_cryptde, config.chain)
            .map_err(ReceiptSessionManagerError::Persistence)?;
        header
            .authorization
            .verify(
                config.chain_id,
                config.settlement_contract,
                payer_cryptde.public_key(),
                now_unix_s,
                header.spent_charge_wei,
            )
            .map_err(ReceiptSessionManagerError::Verification)?;
        if header.authorization.policy.payer_wallet_address != config.payer_wallet_address {
            return Err(ReceiptSessionManagerError::Persistence(
                "recovered payer wallet differs from Node configuration".to_string(),
            ));
        }
        let mut stream_keys = HashSet::new();
        let mut route_epochs = HashSet::new();
        for route in &routes {
            if !stream_keys.insert(route.stream_key)
                || !route_epochs.insert(route.request.route_epoch)
                || route.request.authorization != header.authorization
                || route.observed_responses.len() > MAX_PENDING_RECEIPT_RESPONSES_PER_ROUTE
                || route.routing_provider_states.values().any(|provider| {
                    provider.observed_payloads.len() > MAX_PENDING_RECEIPT_RESPONSES_PER_ROUTE
                })
            {
                return Err(ReceiptSessionManagerError::Persistence(
                    "recovered receipt route is inconsistent".to_string(),
                ));
            }
            route
                .request
                .verify(config.chain_id, config.settlement_contract, now_unix_s)
                .map_err(ReceiptSessionManagerError::Verification)?;
            let checkpoint_matches = |checkpoint: &ReceiptSequenceCheckpoint,
                                      provider: &PublicKey| {
                checkpoint.route_epoch == route.request.route_epoch
                    && &checkpoint.provider_public_key == provider
                    && checkpoint.accounting_commitment == route.request.accounting_commitment
                    && checkpoint.payer_session_public_key
                        == header.authorization.policy.payer_session_public_key
            };
            if route
                .checkpoint_opt
                .as_ref()
                .map(|checkpoint| {
                    route
                        .expected_provider_public_key_opt
                        .as_ref()
                        .map(|provider| !checkpoint_matches(checkpoint, provider))
                        .unwrap_or(true)
                })
                .unwrap_or(false)
                || route
                    .routing_provider_states
                    .iter()
                    .any(|(provider, state)| {
                        state
                            .checkpoint_opt
                            .as_ref()
                            .map(|checkpoint| !checkpoint_matches(checkpoint, provider))
                            .unwrap_or(false)
                    })
            {
                return Err(ReceiptSessionManagerError::Persistence(
                    "recovered receipt checkpoint identity is inconsistent".to_string(),
                ));
            }
        }
        Ok(ActiveReceiptSession {
            authorization: header.authorization,
            payer_cryptde,
            routes,
            spent_charge_wei: header.spent_charge_wei,
        })
    }

    fn revoke_after_persistence_error(&mut self, message: String) -> ReceiptSessionManagerError {
        self.active_opt = None;
        if let Some(store) = self.recovery_store_opt.as_mut() {
            let _ = store.clear();
        }
        ReceiptSessionManagerError::Persistence(message)
    }

    fn persist_header(&mut self) -> Result<(), ReceiptSessionManagerError> {
        let header = match self.active_opt.as_ref() {
            Some(active) => Self::persisted_header(active),
            None => return Ok(()),
        };
        let result = match self.recovery_store_opt.as_mut() {
            Some(store) => store.save_header(&header),
            None if self.recovery_required => {
                Err("a database password is required for recoverable sessions".to_string())
            }
            None => Ok(()),
        };
        result.map_err(|message| self.revoke_after_persistence_error(message))
    }

    fn persist_route(&mut self, stream_key: &StreamKey) -> Result<(), ReceiptSessionManagerError> {
        let (header, route) = match self.active_opt.as_ref() {
            Some(active) => {
                let route = active
                    .routes
                    .iter()
                    .find(|route| &route.stream_key == stream_key)
                    .ok_or(ReceiptSessionManagerError::ReceiptRouteNotFound)?
                    .clone();
                (Self::persisted_header(active), route)
            }
            None => return Ok(()),
        };
        let result = match self.recovery_store_opt.as_mut() {
            Some(store) => store.save_header_and_route(&header, &route),
            None if self.recovery_required => {
                Err("a database password is required for recoverable sessions".to_string())
            }
            None => Ok(()),
        };
        result.map_err(|message| self.revoke_after_persistence_error(message))
    }

    pub fn new(config: ReceiptSessionConfig) -> Self {
        Self {
            config,
            pending_opt: None,
            active_opt: None,
            recovery_store_opt: None,
            recovery_required: false,
        }
    }

    pub(crate) fn new_recovery_required(
        config: ReceiptSessionConfig,
        mut recovery_store_opt: Option<Box<dyn ReceiptSessionRecoveryStore>>,
        now_unix_s: u64,
    ) -> Result<Self, ReceiptSessionManagerError> {
        let active_opt = match recovery_store_opt.as_mut() {
            Some(store) => match store
                .load(now_unix_s)
                .map_err(ReceiptSessionManagerError::Persistence)?
            {
                Some((header, routes)) => {
                    Some(Self::restore_active(&config, header, routes, now_unix_s)?)
                }
                None => None,
            },
            None => None,
        };
        Ok(Self {
            config,
            pending_opt: None,
            active_opt,
            recovery_store_opt,
            recovery_required: true,
        })
    }

    pub fn propose(
        &mut self,
        max_total_charge_wei: &str,
        duration_seconds: u64,
        now_unix_s: u64,
    ) -> Result<ReceiptSessionProposal, ReceiptSessionManagerError> {
        let max_total_charge_wei = max_total_charge_wei
            .parse::<u128>()
            .map_err(|_| ReceiptSessionManagerError::Amount(max_total_charge_wei.to_string()))?;
        if max_total_charge_wei == 0 || max_total_charge_wei > MAX_RECEIPT_SESSION_CHARGE_WEI {
            return Err(ReceiptSessionManagerError::Amount(
                max_total_charge_wei.to_string(),
            ));
        }
        if !(MIN_RECEIPT_SESSION_DURATION_SECONDS..=MAX_RECEIPT_SESSION_DURATION_SECONDS)
            .contains(&duration_seconds)
        {
            return Err(ReceiptSessionManagerError::Duration(duration_seconds));
        }
        let expires_at_unix_s = now_unix_s
            .checked_add(duration_seconds)
            .ok_or(ReceiptSessionManagerError::TimeOverflow)?;
        let payer_cryptde: Box<dyn CryptDE> = Box::new(CryptDEReal::new(self.config.chain));
        let mut authorization_nonce = [0u8; 32];
        payer_cryptde.random(&mut authorization_nonce);
        if authorization_nonce.iter().all(|byte| *byte == 0) {
            return Err(ReceiptSessionManagerError::Signature(
                "secure nonce generation returned an invalid value".to_string(),
            ));
        }
        let policy = ReceiptSessionPolicy::new(
            self.config.chain_id,
            self.config.settlement_contract,
            self.config.payer_wallet_address,
            payer_cryptde.public_key().clone(),
            max_total_charge_wei,
            now_unix_s,
            expires_at_unix_s,
            authorization_nonce,
        );
        let eip712_typed_data = policy
            .eip712_typed_data()
            .map_err(ReceiptSessionManagerError::Verification)?;
        let proposal_id = format!("0x{}", authorization_nonce.to_hex::<String>());
        self.pending_opt = Some(PendingReceiptSession {
            proposal_id: proposal_id.clone(),
            policy: policy.clone(),
            payer_cryptde,
        });
        Ok(ReceiptSessionProposal {
            proposal_id,
            chain_name: self.config.chain.rec().literal_identifier.to_string(),
            masq_token_contract: self.config.chain.rec().contract,
            authorization_id: receipt_session_contract_id(
                policy.payer_wallet_address,
                &policy.authorization_nonce,
            ),
            policy,
            eip712_typed_data,
        })
    }

    pub fn activate(
        &mut self,
        proposal_id: &str,
        wallet_signature: &str,
        now_unix_s: u64,
    ) -> Result<ReceiptSessionStatus, ReceiptSessionManagerError> {
        let pending = self
            .pending_opt
            .as_ref()
            .ok_or(ReceiptSessionManagerError::NoPendingProposal)?;
        if pending.proposal_id != proposal_id {
            return Err(ReceiptSessionManagerError::ProposalMismatch);
        }
        if now_unix_s > pending.policy.expires_at_unix_s {
            self.pending_opt = None;
            return Err(ReceiptSessionManagerError::Expired);
        }
        let signature = Self::signature_from_hex(wallet_signature)?;
        let authorization = AuthorizedReceiptSession {
            policy: pending.policy.clone(),
            wallet_signature: signature,
        };
        authorization
            .verify(
                self.config.chain_id,
                self.config.settlement_contract,
                &pending.policy.payer_session_public_key,
                now_unix_s,
                0,
            )
            .map_err(ReceiptSessionManagerError::Verification)?;
        match self.recovery_store_opt.as_mut() {
            Some(store) => store
                .clear()
                .map_err(ReceiptSessionManagerError::Persistence)?,
            None if self.recovery_required => {
                return Err(ReceiptSessionManagerError::Persistence(
                    "a database password is required for recoverable sessions".to_string(),
                ))
            }
            None => (),
        }
        let pending = self
            .pending_opt
            .take()
            .expect("pending proposal disappeared");
        self.active_opt = Some(ActiveReceiptSession {
            authorization,
            payer_cryptde: pending.payer_cryptde,
            routes: vec![],
            spent_charge_wei: 0,
        });
        self.persist_header()?;
        Ok(self.status(now_unix_s))
    }

    pub fn status(&mut self, now_unix_s: u64) -> ReceiptSessionStatus {
        if self
            .active_opt
            .as_ref()
            .map(|active| now_unix_s > active.authorization.policy.expires_at_unix_s)
            .unwrap_or(false)
        {
            self.active_opt = None;
            if let Some(store) = self.recovery_store_opt.as_mut() {
                let _ = store.clear();
            }
        }
        match self.active_opt.as_ref() {
            Some(active) if now_unix_s <= active.authorization.policy.expires_at_unix_s => {
                let policy = &active.authorization.policy;
                ReceiptSessionStatus {
                    protocol_version_opt: Some(policy.protocol_version),
                    chain_name_opt: Some(self.config.chain.rec().literal_identifier.to_string()),
                    chain_id_opt: Some(policy.chain_id),
                    masq_token_contract_opt: Some(self.config.chain.rec().contract),
                    settlement_contract_opt: Some(policy.settlement_contract),
                    payer_wallet_address_opt: Some(policy.payer_wallet_address),
                    payer_session_public_key_opt: Some(active.payer_cryptde.public_key().clone()),
                    authorization_id_opt: Some(receipt_session_contract_id(
                        policy.payer_wallet_address,
                        &policy.authorization_nonce,
                    )),
                    max_total_charge_wei_opt: Some(policy.max_total_charge_wei),
                    spent_charge_wei_opt: Some(active.spent_charge_wei),
                    valid_from_unix_s_opt: Some(policy.valid_from_unix_s),
                    expires_at_unix_s_opt: Some(policy.expires_at_unix_s),
                }
            }
            _ => ReceiptSessionStatus::inactive(),
        }
    }

    pub fn requires_service_receipt_capability(&mut self, now_unix_s: u64) -> bool {
        self.status(now_unix_s).is_active()
    }

    pub fn request_for_stream(
        &mut self,
        stream_key: StreamKey,
        now_unix_s: u64,
    ) -> Result<Option<ReceiptSessionRequest>, ReceiptSessionManagerError> {
        self.status(now_unix_s);
        let active = match self.active_opt.as_mut() {
            Some(active) => active,
            None => return Ok(None),
        };
        if let Some(route) = active
            .routes
            .iter()
            .find(|route| route.stream_key == stream_key)
        {
            return Ok(Some(route.request.clone()));
        }
        if active.routes.len() >= MAX_RECEIPT_SESSION_ROUTES {
            return Err(ReceiptSessionManagerError::Signature(
                "receipt session reached its route limit".to_string(),
            ));
        }
        let request = Self::fresh_route_request(active)?;
        active
            .routes
            .push(Self::route_state(stream_key, request.clone()));
        self.persist_route(&stream_key)?;
        Ok(Some(request))
    }

    /// Replaces the per-route authorization after an exit failure while preserving the wallet's
    /// aggregate session budget. A late offer from the failed exit can no longer match the active
    /// route epoch.
    pub fn rotate_route_for_stream(
        &mut self,
        stream_key: StreamKey,
        now_unix_s: u64,
    ) -> Result<Option<ReceiptSessionRequest>, ReceiptSessionManagerError> {
        self.status(now_unix_s);
        let active = match self.active_opt.as_mut() {
            Some(active) => active,
            None => return Ok(None),
        };
        let replacement = Self::fresh_route_request(active)?;
        match active
            .routes
            .iter()
            .position(|route| route.stream_key == stream_key)
        {
            Some(position) => {
                active.routes[position] = Self::route_state(stream_key, replacement.clone())
            }
            None if active.routes.len() < MAX_RECEIPT_SESSION_ROUTES => active
                .routes
                .push(Self::route_state(stream_key, replacement.clone())),
            None => {
                return Err(ReceiptSessionManagerError::Signature(
                    "receipt session reached its route limit".to_string(),
                ))
            }
        }
        self.persist_route(&stream_key)?;
        Ok(Some(replacement))
    }

    pub fn route_epoch_for_stream(&self, stream_key: &StreamKey) -> Option<[u8; 32]> {
        self.active_opt.as_ref().and_then(|active| {
            active
                .routes
                .iter()
                .find(|route| &route.stream_key == stream_key)
                .map(|route| route.request.route_epoch)
        })
    }

    pub fn bind_exit_quote(
        &mut self,
        stream_key: &StreamKey,
        provider_public_key: PublicKey,
        exit_service_rate: u64,
        exit_byte_rate: u64,
        now_unix_s: u64,
    ) -> Result<(), ReceiptSessionManagerError> {
        self.status(now_unix_s);
        let active = self
            .active_opt
            .as_mut()
            .ok_or(ReceiptSessionManagerError::NoActiveSession)?;
        let route = active
            .routes
            .iter_mut()
            .find(|route| &route.stream_key == stream_key)
            .ok_or(ReceiptSessionManagerError::ReceiptRouteNotFound)?;
        match (
            route.expected_provider_public_key_opt.as_ref(),
            route.exit_service_rate_opt,
            route.exit_byte_rate_opt,
        ) {
            (None, None, None) => {
                route.expected_provider_public_key_opt = Some(provider_public_key);
                route.exit_service_rate_opt = Some(exit_service_rate);
                route.exit_byte_rate_opt = Some(exit_byte_rate);
                self.persist_route(stream_key)
            }
            (Some(existing_provider), Some(existing_service_rate), Some(existing_byte_rate))
                if existing_provider == &provider_public_key
                    && existing_service_rate == exit_service_rate
                    && existing_byte_rate == exit_byte_rate =>
            {
                Ok(())
            }
            _ => Err(ReceiptSessionManagerError::Verification(
                ServiceReceiptError::ProviderPublicKeyMismatch,
            )),
        }
    }

    pub fn bind_routing_quotes(
        &mut self,
        stream_key: &StreamKey,
        request_quotes: Vec<RoutingReceiptQuote>,
        response_quotes: Vec<RoutingReceiptQuote>,
        now_unix_s: u64,
    ) -> Result<(), ReceiptSessionManagerError> {
        self.status(now_unix_s);
        let active = self
            .active_opt
            .as_mut()
            .ok_or(ReceiptSessionManagerError::NoActiveSession)?;
        let route = active
            .routes
            .iter_mut()
            .find(|route| &route.stream_key == stream_key)
            .ok_or(ReceiptSessionManagerError::ReceiptRouteNotFound)?;
        let request_providers = request_quotes
            .iter()
            .map(|quote| quote.provider_public_key.clone())
            .collect::<Vec<_>>();
        let response_providers = response_quotes
            .iter()
            .map(|quote| quote.provider_public_key.clone())
            .collect::<Vec<_>>();
        if !route.routing_provider_states.is_empty() {
            if route.routing_request_providers != request_providers
                || route.routing_response_providers != response_providers
                || request_quotes
                    .iter()
                    .chain(response_quotes.iter())
                    .any(|quote| {
                        route
                            .routing_provider_states
                            .get(&quote.provider_public_key)
                            .map(|state| {
                                state.service_rate != quote.service_rate
                                    || state.byte_rate != quote.byte_rate
                            })
                            .unwrap_or(true)
                    })
            {
                return Err(ReceiptSessionManagerError::ReceiptQuoteUnavailable);
            }
            return Ok(());
        }
        let mut provider_states: HashMap<PublicKey, RoutingProviderReceiptState> = HashMap::new();
        for quote in request_quotes.iter().chain(response_quotes.iter()) {
            match provider_states.get(&quote.provider_public_key) {
                Some(existing)
                    if existing.service_rate != quote.service_rate
                        || existing.byte_rate != quote.byte_rate =>
                {
                    return Err(ReceiptSessionManagerError::ReceiptQuoteUnavailable)
                }
                Some(_) => (),
                None => {
                    provider_states.insert(
                        quote.provider_public_key.clone(),
                        RoutingProviderReceiptState {
                            service_rate: quote.service_rate,
                            byte_rate: quote.byte_rate,
                            observed_payloads: VecDeque::new(),
                            checkpoint_opt: None,
                            acknowledged_payload_opt: None,
                        },
                    );
                }
            }
        }
        route.routing_request_providers = request_providers;
        route.routing_response_providers = response_providers;
        route.routing_provider_states = provider_states;
        self.persist_route(stream_key)
    }

    fn record_routing_payload(
        route: &mut ReceiptRouteState,
        providers: Vec<PublicKey>,
        payload_size: u64,
    ) -> Result<(), ReceiptSessionManagerError> {
        if providers.iter().any(|provider| {
            route
                .routing_provider_states
                .get(provider)
                .map(|state| {
                    state.observed_payloads.len() >= MAX_PENDING_RECEIPT_RESPONSES_PER_ROUTE
                })
                .unwrap_or(true)
        }) {
            return Err(ReceiptSessionManagerError::ReceiptObservationMismatch);
        }
        for provider in providers {
            route
                .routing_provider_states
                .get_mut(&provider)
                .expect("routing provider disappeared after validation")
                .observed_payloads
                .push_back(payload_size);
        }
        Ok(())
    }

    pub fn record_routing_request(
        &mut self,
        stream_key: &StreamKey,
        payload_size: u64,
        now_unix_s: u64,
    ) -> Result<(), ReceiptSessionManagerError> {
        self.status(now_unix_s);
        let route = self
            .active_opt
            .as_mut()
            .ok_or(ReceiptSessionManagerError::NoActiveSession)?
            .routes
            .iter_mut()
            .find(|route| &route.stream_key == stream_key)
            .ok_or(ReceiptSessionManagerError::ReceiptRouteNotFound)?;
        Self::record_routing_payload(route, route.routing_request_providers.clone(), payload_size)?;
        self.persist_route(stream_key)
    }

    pub fn record_routing_response(
        &mut self,
        stream_key: &StreamKey,
        payload_size: u64,
        now_unix_s: u64,
    ) -> Result<(), ReceiptSessionManagerError> {
        self.status(now_unix_s);
        let route = self
            .active_opt
            .as_mut()
            .ok_or(ReceiptSessionManagerError::NoActiveSession)?
            .routes
            .iter_mut()
            .find(|route| &route.stream_key == stream_key)
            .ok_or(ReceiptSessionManagerError::ReceiptRouteNotFound)?;
        Self::record_routing_payload(
            route,
            route.routing_response_providers.clone(),
            payload_size,
        )?;
        self.persist_route(stream_key)
    }

    pub fn decrypt_routing_offer(
        &mut self,
        encrypted_offer: &CryptData,
        now_unix_s: u64,
    ) -> Result<ServiceReceiptOfferPayload_0v1, ReceiptSessionManagerError> {
        self.status(now_unix_s);
        let active = self
            .active_opt
            .as_ref()
            .ok_or(ReceiptSessionManagerError::NoActiveSession)?;
        decodex::<ServiceReceiptOfferPayload_0v1>(active.payer_cryptde.as_ref(), encrypted_offer)
            .map_err(|error| {
                ReceiptSessionManagerError::Signature(format!(
                    "cannot decrypt routing receipt: {:?}",
                    error
                ))
            })
    }

    pub fn record_exit_response(
        &mut self,
        stream_key: &StreamKey,
        sequence: u64,
        payload_size: u64,
        now_unix_s: u64,
    ) -> Result<(), ReceiptSessionManagerError> {
        self.status(now_unix_s);
        let active = self
            .active_opt
            .as_mut()
            .ok_or(ReceiptSessionManagerError::NoActiveSession)?;
        let route = active
            .routes
            .iter_mut()
            .find(|route| &route.stream_key == stream_key)
            .ok_or(ReceiptSessionManagerError::ReceiptRouteNotFound)?;
        if route
            .checkpoint_opt
            .as_ref()
            .map(|checkpoint| sequence <= checkpoint.last_sequence)
            .unwrap_or(false)
            || route.observed_responses.len() >= MAX_PENDING_RECEIPT_RESPONSES_PER_ROUTE
        {
            return Err(ReceiptSessionManagerError::ReceiptObservationMismatch);
        }
        if route
            .observed_responses
            .iter()
            .any(|(observed_sequence, _, _)| *observed_sequence == sequence)
        {
            return Err(ReceiptSessionManagerError::ReceiptObservationMismatch);
        }
        let aggregate_payload_size = route
            .pending_request_payload_size
            .checked_add(payload_size)
            .ok_or(ReceiptSessionManagerError::ReceiptObservationMismatch)?;
        let aggregate_service_units = route
            .pending_request_service_units
            .checked_add(1)
            .ok_or(ReceiptSessionManagerError::ReceiptObservationMismatch)?;
        route.observed_responses.push_back((
            sequence,
            aggregate_payload_size,
            aggregate_service_units,
        ));
        route.pending_request_payload_size = 0;
        route.pending_request_service_units = 0;
        self.persist_route(stream_key)
    }

    pub fn record_exit_request(
        &mut self,
        stream_key: &StreamKey,
        payload_size: u64,
        now_unix_s: u64,
    ) -> Result<(), ReceiptSessionManagerError> {
        self.status(now_unix_s);
        let active = self
            .active_opt
            .as_mut()
            .ok_or(ReceiptSessionManagerError::NoActiveSession)?;
        let route = active
            .routes
            .iter_mut()
            .find(|route| &route.stream_key == stream_key)
            .ok_or(ReceiptSessionManagerError::ReceiptRouteNotFound)?;
        if payload_size == 0 {
            return Ok(());
        }
        route.pending_request_payload_size = route
            .pending_request_payload_size
            .checked_add(payload_size)
            .ok_or(ReceiptSessionManagerError::ReceiptObservationMismatch)?;
        route.pending_request_service_units = route
            .pending_request_service_units
            .checked_add(1)
            .ok_or(ReceiptSessionManagerError::ReceiptObservationMismatch)?;
        self.persist_route(stream_key)
    }

    pub fn acknowledge_offer(
        &mut self,
        signed_receipt: SignedServiceReceipt,
        now_unix_s: u64,
    ) -> Result<ServiceReceiptPayload_0v1, ReceiptSessionManagerError> {
        if signed_receipt.receipt.service_kind == ServiceKind::Routing {
            return self.acknowledge_routing_offer(signed_receipt, now_unix_s);
        }
        self.status(now_unix_s);
        let active = self
            .active_opt
            .as_mut()
            .ok_or(ReceiptSessionManagerError::NoActiveSession)?;
        let offered_receipt = &signed_receipt.receipt;
        let route = active
            .routes
            .iter_mut()
            .find(|route| route.request.route_epoch == offered_receipt.route_epoch)
            .ok_or(ReceiptSessionManagerError::ReceiptRouteNotFound)?;
        if let Some(payload) = route.acknowledged_payload_opt.as_ref() {
            if payload.acknowledged_receipt.signed_receipt == signed_receipt {
                return Ok(payload.clone());
            }
        }
        let expected_provider = route
            .expected_provider_public_key_opt
            .as_ref()
            .ok_or(ReceiptSessionManagerError::ReceiptQuoteUnavailable)?;
        let expected_service_rate = route
            .exit_service_rate_opt
            .ok_or(ReceiptSessionManagerError::ReceiptQuoteUnavailable)?;
        let expected_byte_rate = route
            .exit_byte_rate_opt
            .ok_or(ReceiptSessionManagerError::ReceiptQuoteUnavailable)?;
        let expected_response = route
            .observed_responses
            .front()
            .ok_or(ReceiptSessionManagerError::ReceiptObservationMismatch)?;
        if &offered_receipt.provider_public_key != expected_provider {
            return Err(ReceiptSessionManagerError::Verification(
                ServiceReceiptError::ProviderPublicKeyMismatch,
            ));
        }
        if offered_receipt.service_kind != crate::sub_lib::service_receipt::ServiceKind::Exit
            || offered_receipt.service_rate != expected_service_rate
            || offered_receipt.byte_rate != expected_byte_rate
            || offered_receipt.sequence != expected_response.0
            || offered_receipt.payload_size != expected_response.1
            || offered_receipt.service_units != expected_response.2
        {
            return Err(ReceiptSessionManagerError::ReceiptObservationMismatch);
        }
        let acknowledged_receipt = signed_receipt
            .acknowledge(active.payer_cryptde.as_ref())
            .map_err(ReceiptSessionManagerError::Verification)?;
        active
            .authorization
            .verify_for_receipt(
                &acknowledged_receipt,
                active.payer_cryptde.as_ref(),
                self.config.chain_id,
                self.config.settlement_contract,
                now_unix_s,
            )
            .map_err(ReceiptSessionManagerError::Verification)?;
        let (checkpoint, previous_charge) = match route.checkpoint_opt.as_ref() {
            Some(existing_checkpoint) => {
                let mut checkpoint = existing_checkpoint.clone();
                let previous_charge = checkpoint.cumulative_charge_wei;
                checkpoint
                    .advance(&acknowledged_receipt, active.payer_cryptde.as_ref())
                    .map_err(ReceiptSessionManagerError::Verification)?;
                (checkpoint, previous_charge)
            }
            None => (
                ReceiptSequenceCheckpoint::begin(
                    &acknowledged_receipt,
                    active.payer_cryptde.as_ref(),
                )
                .map_err(ReceiptSessionManagerError::Verification)?,
                0,
            ),
        };
        let delta = checkpoint
            .cumulative_charge_wei
            .checked_sub(previous_charge)
            .ok_or(ReceiptSessionManagerError::Verification(
                ServiceReceiptError::CumulativeChargeMismatch,
            ))?;
        let spent_charge_wei = active.spent_charge_wei.checked_add(delta).ok_or(
            ReceiptSessionManagerError::Verification(ServiceReceiptError::AmountLimitExceeded),
        )?;
        if spent_charge_wei > active.authorization.policy.max_total_charge_wei {
            return Err(ReceiptSessionManagerError::Verification(
                ServiceReceiptError::AmountLimitExceeded,
            ));
        }
        let payload = ServiceReceiptPayload_0v1 {
            authorization: active.authorization.clone(),
            acknowledged_receipt,
        };
        route.checkpoint_opt = Some(checkpoint);
        route.acknowledged_payload_opt = Some(payload.clone());
        let _ = route.observed_responses.pop_front();
        active.spent_charge_wei = spent_charge_wei;
        let stream_key = route.stream_key;
        self.persist_route(&stream_key)?;
        Ok(payload)
    }

    fn acknowledge_routing_offer(
        &mut self,
        signed_receipt: SignedServiceReceipt,
        now_unix_s: u64,
    ) -> Result<ServiceReceiptPayload_0v1, ReceiptSessionManagerError> {
        self.status(now_unix_s);
        let active = self
            .active_opt
            .as_mut()
            .ok_or(ReceiptSessionManagerError::NoActiveSession)?;
        let offered_receipt = &signed_receipt.receipt;
        let route = active
            .routes
            .iter_mut()
            .find(|route| route.request.route_epoch == offered_receipt.route_epoch)
            .ok_or(ReceiptSessionManagerError::ReceiptRouteNotFound)?;
        let provider_state = route
            .routing_provider_states
            .get_mut(&offered_receipt.provider_public_key)
            .ok_or(ReceiptSessionManagerError::ReceiptQuoteUnavailable)?;
        if let Some(payload) = provider_state.acknowledged_payload_opt.as_ref() {
            if payload.acknowledged_receipt.signed_receipt == signed_receipt {
                return Ok(payload.clone());
            }
        }
        let expected_payload_size = *provider_state
            .observed_payloads
            .front()
            .ok_or(ReceiptSessionManagerError::ReceiptObservationMismatch)?;
        let expected_sequence = match provider_state.checkpoint_opt.as_ref() {
            Some(checkpoint) => checkpoint
                .last_sequence
                .checked_add(1)
                .ok_or(ReceiptSessionManagerError::ReceiptObservationMismatch)?,
            None => 1,
        };
        if offered_receipt.service_kind != ServiceKind::Routing
            || offered_receipt.service_rate != provider_state.service_rate
            || offered_receipt.byte_rate != provider_state.byte_rate
            || offered_receipt.sequence != expected_sequence
            || offered_receipt.payload_size != expected_payload_size
            || offered_receipt.service_units != 1
        {
            return Err(ReceiptSessionManagerError::ReceiptObservationMismatch);
        }
        let acknowledged_receipt = signed_receipt
            .acknowledge(active.payer_cryptde.as_ref())
            .map_err(ReceiptSessionManagerError::Verification)?;
        active
            .authorization
            .verify_for_receipt(
                &acknowledged_receipt,
                active.payer_cryptde.as_ref(),
                self.config.chain_id,
                self.config.settlement_contract,
                now_unix_s,
            )
            .map_err(ReceiptSessionManagerError::Verification)?;
        let (checkpoint, previous_charge) = match provider_state.checkpoint_opt.as_ref() {
            Some(existing_checkpoint) => {
                let mut checkpoint = existing_checkpoint.clone();
                let previous_charge = checkpoint.cumulative_charge_wei;
                checkpoint
                    .advance(&acknowledged_receipt, active.payer_cryptde.as_ref())
                    .map_err(ReceiptSessionManagerError::Verification)?;
                (checkpoint, previous_charge)
            }
            None => (
                ReceiptSequenceCheckpoint::begin(
                    &acknowledged_receipt,
                    active.payer_cryptde.as_ref(),
                )
                .map_err(ReceiptSessionManagerError::Verification)?,
                0,
            ),
        };
        let delta = checkpoint
            .cumulative_charge_wei
            .checked_sub(previous_charge)
            .ok_or(ReceiptSessionManagerError::Verification(
                ServiceReceiptError::CumulativeChargeMismatch,
            ))?;
        let spent_charge_wei = active.spent_charge_wei.checked_add(delta).ok_or(
            ReceiptSessionManagerError::Verification(ServiceReceiptError::AmountLimitExceeded),
        )?;
        if spent_charge_wei > active.authorization.policy.max_total_charge_wei {
            return Err(ReceiptSessionManagerError::Verification(
                ServiceReceiptError::AmountLimitExceeded,
            ));
        }
        let payload = ServiceReceiptPayload_0v1 {
            authorization: active.authorization.clone(),
            acknowledged_receipt,
        };
        provider_state.checkpoint_opt = Some(checkpoint);
        provider_state.acknowledged_payload_opt = Some(payload.clone());
        let _ = provider_state.observed_payloads.pop_front();
        active.spent_charge_wei = spent_charge_wei;
        let stream_key = route.stream_key;
        self.persist_route(&stream_key)?;
        Ok(payload)
    }

    pub fn stop(&mut self) -> ReceiptSessionStatus {
        self.pending_opt = None;
        self.active_opt = None;
        if let Some(store) = self.recovery_store_opt.as_mut() {
            let _ = store.clear();
        }
        ReceiptSessionStatus::inactive()
    }

    fn signature_from_hex(input: &str) -> Result<Signature, ReceiptSessionManagerError> {
        let unprefixed = input.strip_prefix("0x").unwrap_or(input);
        let bytes: Vec<u8> = unprefixed
            .from_hex()
            .map_err(|error| ReceiptSessionManagerError::Signature(format!("{:?}", error)))?;
        if bytes.len() != 65 {
            return Err(ReceiptSessionManagerError::Signature(format!(
                "expected 65 bytes, received {}",
                bytes.len()
            )));
        }
        let mut r = [0u8; 32];
        r.copy_from_slice(&bytes[..32]);
        let mut s = [0u8; 32];
        s.copy_from_slice(&bytes[32..64]);
        if r.iter().all(|byte| *byte == 0)
            || s.iter().all(|byte| *byte == 0)
            || s > SECP256K1_HALF_ORDER
        {
            return Err(ReceiptSessionManagerError::Signature(
                "r and s must be non-zero and s must use canonical low-s form".to_string(),
            ));
        }
        let v = match bytes[64] {
            0 | 1 => bytes[64],
            27 | 28 => bytes[64] - 27,
            other => {
                return Err(ReceiptSessionManagerError::Signature(format!(
                    "recovery ID {} is not 0, 1, 27 or 28",
                    other
                )))
            }
        };
        Ok(Signature { v, r, s })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accountant::db_access_objects::utils::DaoFactoryReal;
    use crate::blockchain::bip39::Bip39;
    use crate::database::db_initializer::{
        DbInitializationConfig, DbInitializerReal, DATABASE_FILE,
    };
    use crate::db_config::config_dao::{ConfigDao, ConfigDaoReal};
    use crate::db_config::secure_config_layer::SecureConfigLayer;
    use crate::proxy_server::receipt_session_recovery::ReceiptSessionRecoveryStoreReal;
    use crate::sub_lib::service_receipt::{
        make_accounting_commitment, ServiceKind, ServiceReceipt,
    };
    use crate::sub_lib::wallet::Wallet;
    use crate::test_utils::make_paying_wallet;
    use masq_lib::constants::{CURRENT_SCHEMA_VERSION, RECEIPT_SESSION_RECOVERY_KEY};
    use masq_lib::test_utils::utils::ensure_node_home_directory_does_not_exist;
    use masq_lib::test_utils::utils::TEST_DEFAULT_CHAIN;
    use rusqlite::Connection;
    use std::fs::create_dir_all;

    fn config(wallet: &Wallet) -> ReceiptSessionConfig {
        ReceiptSessionConfig {
            chain: TEST_DEFAULT_CHAIN,
            chain_id: TEST_DEFAULT_CHAIN.rec().num_chain_id,
            settlement_contract: TEST_DEFAULT_CHAIN.rec().contract,
            payer_wallet_address: wallet.address(),
        }
    }

    fn signature_hex(signature: &Signature, ethereum_v: bool) -> String {
        let v = if ethereum_v {
            signature.v + 27
        } else {
            signature.v
        };
        format!(
            "0x{}{}{:02x}",
            signature.r.to_hex::<String>(),
            signature.s.to_hex::<String>(),
            v
        )
    }

    fn recovery_factory(test_name: &str, password: &str) -> DaoFactoryReal {
        let directory = ensure_node_home_directory_does_not_exist("receipt_session", test_name);
        create_dir_all(&directory).unwrap();
        let connection = Connection::open(directory.join(DATABASE_FILE)).unwrap();
        connection
            .execute(
                "create table config (
                    name text primary key,
                    value text,
                    encrypted integer not null
                )",
                [],
            )
            .unwrap();
        let example = Bip39::encrypt_bytes(b"receipt recovery password check", password).unwrap();
        connection
            .execute(
                "insert into config (name, value, encrypted) values (?1, ?2, 1)",
                rusqlite::params![
                    crate::db_config::secure_config_layer::EXAMPLE_ENCRYPTED,
                    example
                ],
            )
            .unwrap();
        connection
            .execute(
                "insert into config (name, value, encrypted) values (?1, null, 1)",
                rusqlite::params![RECEIPT_SESSION_RECOVERY_KEY],
            )
            .unwrap();
        connection
            .execute(
                "insert into config (name, value, encrypted) values ('schema_version', ?1, 0)",
                rusqlite::params![CURRENT_SCHEMA_VERSION.to_string()],
            )
            .unwrap();
        DbInitializerReal::create_receipt_session_recovery_tables(&connection);
        drop(connection);
        DaoFactoryReal::new(&directory, DbInitializationConfig::panic_on_migration())
    }

    #[test]
    fn proposal_is_wallet_readable_bounded_and_inactive_until_signed() {
        let wallet = make_paying_wallet(b"receipt session wallet");
        let settlement_contract = Address::from([0x77; 20]);
        let mut session_config = config(&wallet);
        session_config.settlement_contract = settlement_contract;
        let mut subject = ReceiptSessionManager::new(session_config);

        let proposal = subject.propose("123456789", 600, 1000).unwrap();

        assert_eq!(
            format!("{:?}", proposal),
            "ReceiptSessionProposal { proposal_data: [REDACTED] }"
        );
        assert_eq!(
            format!(
                "{:?}",
                ReceiptSessionManagerError::Signature(
                    "private signature parser marker".to_string()
                )
            ),
            "Signature([REDACTED])"
        );
        assert_eq!(
            ReceiptSessionManagerError::Persistence("private recovery database marker".to_string())
                .to_string(),
            "receipt-session recovery failed"
        );
        assert_eq!(
            proposal.chain_name,
            TEST_DEFAULT_CHAIN.rec().literal_identifier
        );
        assert_eq!(
            proposal.masq_token_contract,
            TEST_DEFAULT_CHAIN.rec().contract
        );
        assert_eq!(proposal.policy.protocol_version, 1);
        assert_eq!(
            proposal.policy.chain_id,
            TEST_DEFAULT_CHAIN.rec().num_chain_id
        );
        assert_eq!(proposal.policy.settlement_contract, settlement_contract);
        assert_eq!(
            proposal.authorization_id,
            receipt_session_contract_id(
                proposal.policy.payer_wallet_address,
                &proposal.policy.authorization_nonce,
            )
        );
        assert_eq!(proposal.policy.max_total_charge_wei, 123456789);
        assert_eq!(proposal.policy.valid_from_unix_s, 1000);
        assert_eq!(proposal.policy.expires_at_unix_s, 1600);
        assert_eq!(
            proposal.eip712_typed_data["message"]["maxTotalChargeWei"],
            "123456789"
        );
        assert_eq!(
            proposal.eip712_typed_data["domain"]["verifyingContract"],
            format!("{:#x}", settlement_contract)
        );
        assert!(!subject.requires_service_receipt_capability(1000));
    }

    #[test]
    fn matching_external_signature_activates_and_stop_revokes_session() {
        let wallet = make_paying_wallet(b"receipt session wallet");
        let settlement_contract = Address::from([0x77; 20]);
        let mut session_config = config(&wallet);
        session_config.settlement_contract = settlement_contract;
        let mut subject = ReceiptSessionManager::new(session_config);
        let proposal = subject.propose("42", 600, 1000).unwrap();
        let signature = wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();

        let status = subject
            .activate(
                &proposal.proposal_id,
                &signature_hex(&signature, true),
                1001,
            )
            .unwrap();

        assert!(status.is_active());
        assert_eq!(
            format!("{:?}", status),
            "ReceiptSessionStatus { active: true, session_data: [REDACTED] }"
        );
        assert_eq!(status.protocol_version_opt, Some(1));
        assert_eq!(
            status.chain_name_opt,
            Some(TEST_DEFAULT_CHAIN.rec().literal_identifier.to_string())
        );
        assert_eq!(
            status.chain_id_opt,
            Some(TEST_DEFAULT_CHAIN.rec().num_chain_id)
        );
        assert_eq!(
            status.masq_token_contract_opt,
            Some(TEST_DEFAULT_CHAIN.rec().contract)
        );
        assert_eq!(status.settlement_contract_opt, Some(settlement_contract));
        assert_eq!(status.payer_wallet_address_opt, Some(wallet.address()));
        assert_eq!(
            status.payer_session_public_key_opt,
            Some(proposal.policy.payer_session_public_key.clone())
        );
        assert_eq!(
            status.authorization_id_opt,
            Some(receipt_session_contract_id(
                wallet.address(),
                &proposal.policy.authorization_nonce,
            ))
        );
        assert_eq!(status.max_total_charge_wei_opt, Some(42));
        assert_eq!(status.spent_charge_wei_opt, Some(0));
        assert_eq!(status.valid_from_unix_s_opt, Some(1000));
        assert_eq!(status.expires_at_unix_s_opt, Some(1600));
        let active = subject.active_opt.as_ref().unwrap();
        let persisted_header = ReceiptSessionManager::persisted_header(active);
        let request = ReceiptSessionRequest {
            authorization: active.authorization.clone(),
            route_epoch: [0x71; 32],
            accounting_commitment: [0x72; 32],
        };
        let route_state = ReceiptSessionManager::route_state(
            StreamKey::make_meaningful_stream_key("private receipt route marker"),
            request,
        );
        let mut observed_payloads = VecDeque::new();
        observed_payloads.push_back(123_456);
        let provider_state = RoutingProviderReceiptState {
            service_rate: 789,
            byte_rate: 456,
            observed_payloads,
            checkpoint_opt: None,
            acknowledged_payload_opt: None,
        };
        let quote = RoutingReceiptQuote {
            provider_public_key: PublicKey::new(b"private routing provider marker"),
            service_rate: 321,
            byte_rate: 654,
        };

        assert_eq!(
            format!("{:?}", persisted_header),
            "PersistedReceiptSessionHeader { recovery_header: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", route_state),
            "ReceiptRouteState { route_state: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", provider_state),
            "RoutingProviderReceiptState { provider_state: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", quote),
            "RoutingReceiptQuote { quote_data: [REDACTED] }"
        );
        assert!(subject.requires_service_receipt_capability(1600));
        assert!(!subject.requires_service_receipt_capability(1601));
        assert!(!subject.stop().is_active());
    }

    #[test]
    fn encrypted_consumer_session_recovers_key_routes_and_observations_after_restart() {
        let password = "correct horse battery staple";
        let factory = recovery_factory(
            "encrypted_consumer_session_recovers_key_routes_and_observations_after_restart",
            password,
        );
        let wallet = make_paying_wallet(b"recoverable receipt session wallet");
        let session_config = config(&wallet);
        let store =
            ReceiptSessionRecoveryStoreReal::new(&factory, password, TEST_DEFAULT_CHAIN).unwrap();
        let mut subject = ReceiptSessionManager::new_recovery_required(
            session_config.clone(),
            Some(Box::new(store)),
            1000,
        )
        .unwrap();
        let proposal = subject.propose("100000", 600, 1000).unwrap();
        let signature = wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        subject
            .activate(
                &proposal.proposal_id,
                &signature_hex(&signature, true),
                1001,
            )
            .unwrap();
        let stream_key = StreamKey::make_meaningful_stream_key("recoverable receipt route");
        let request_before_restart = subject
            .request_for_stream(stream_key, 1001)
            .unwrap()
            .unwrap();
        let provider = PublicKey::new(b"recoverable routing provider");
        subject
            .bind_routing_quotes(
                &stream_key,
                vec![RoutingReceiptQuote {
                    provider_public_key: provider,
                    service_rate: 7,
                    byte_rate: 3,
                }],
                vec![],
                1001,
            )
            .unwrap();
        subject
            .record_routing_request(&stream_key, 123, 1001)
            .unwrap();
        let status_before_restart = subject.status(1001);
        let public_key_before_restart = status_before_restart
            .payer_session_public_key_opt
            .clone()
            .unwrap();
        drop(subject);

        let raw = Connection::open(factory.data_directory.join(DATABASE_FILE)).unwrap();
        let encrypted_header: Vec<u8> = raw
            .query_row(
                "select encrypted_header from receipt_session_recovery where singleton_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let encrypted_route: Vec<u8> = raw
            .query_row(
                "select encrypted_route from receipt_session_route_recovery",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            serde_cbor::from_slice::<PersistedReceiptSessionHeader>(&encrypted_header).is_err()
        );
        assert!(serde_cbor::from_slice::<ReceiptRouteState>(&encrypted_route).is_err());
        assert!(!encrypted_header
            .windows(wallet.address().as_bytes().len())
            .any(|window| window == wallet.address().as_bytes()));
        drop(raw);

        assert!(ReceiptSessionRecoveryStoreReal::new(
            &factory,
            "wrong password",
            TEST_DEFAULT_CHAIN,
        )
        .is_err());
        let restored_store =
            ReceiptSessionRecoveryStoreReal::new(&factory, password, TEST_DEFAULT_CHAIN).unwrap();
        let mut restored = ReceiptSessionManager::new_recovery_required(
            session_config,
            Some(Box::new(restored_store)),
            1002,
        )
        .unwrap();
        assert_eq!(restored.status(1002), status_before_restart);
        assert_eq!(
            restored.status(1002).payer_session_public_key_opt,
            Some(public_key_before_restart)
        );
        assert_eq!(
            restored
                .request_for_stream(stream_key, 1002)
                .unwrap()
                .unwrap(),
            request_before_restart
        );
        let route = restored
            .active_opt
            .as_ref()
            .unwrap()
            .routes
            .iter()
            .find(|route| route.stream_key == stream_key)
            .unwrap();
        assert_eq!(
            route
                .routing_provider_states
                .values()
                .next()
                .unwrap()
                .observed_payloads,
            VecDeque::from(vec![123])
        );
        let new_password = "new correct horse battery staple";
        let mut config_dao: Box<dyn ConfigDao> =
            Box::new(ConfigDaoReal::new(factory.make_connection()));
        SecureConfigLayer::new()
            .change_password(Some(password.to_string()), new_password, &mut config_dao)
            .unwrap();
        restored
            .record_routing_request(&stream_key, 456, 1002)
            .unwrap();
        drop(restored);
        assert!(
            ReceiptSessionRecoveryStoreReal::new(&factory, password, TEST_DEFAULT_CHAIN,).is_err()
        );

        let rotated_store =
            ReceiptSessionRecoveryStoreReal::new(&factory, new_password, TEST_DEFAULT_CHAIN)
                .unwrap();
        let mut rotated = ReceiptSessionManager::new_recovery_required(
            config(&wallet),
            Some(Box::new(rotated_store)),
            1003,
        )
        .unwrap();
        assert_eq!(
            rotated
                .active_opt
                .as_ref()
                .unwrap()
                .routes
                .first()
                .unwrap()
                .routing_provider_states
                .values()
                .next()
                .unwrap()
                .observed_payloads,
            VecDeque::from(vec![123, 456])
        );
        assert!(!rotated.stop().is_active());

        let cleared_store =
            ReceiptSessionRecoveryStoreReal::new(&factory, new_password, TEST_DEFAULT_CHAIN)
                .unwrap();
        let mut cleared = ReceiptSessionManager::new_recovery_required(
            config(&wallet),
            Some(Box::new(cleared_store)),
            1004,
        )
        .unwrap();
        assert!(!cleared.status(1004).is_active());
    }

    #[test]
    fn production_session_activation_requires_encrypted_recovery_storage() {
        let wallet = make_paying_wallet(b"recovery-required receipt wallet");
        let mut subject =
            ReceiptSessionManager::new_recovery_required(config(&wallet), None, 1000).unwrap();
        let proposal = subject.propose("1000", 600, 1000).unwrap();
        let signature = wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();

        assert!(matches!(
            subject.activate(
                &proposal.proposal_id,
                &signature_hex(&signature, true),
                1001,
            ),
            Err(ReceiptSessionManagerError::Persistence(_))
        ));
        assert!(!subject.status(1001).is_active());
    }

    #[test]
    fn route_rotation_replaces_provider_binding_without_resetting_the_session_budget() {
        let wallet = make_paying_wallet(b"rotating receipt session wallet");
        let mut subject = ReceiptSessionManager::new(config(&wallet));
        let proposal = subject.propose("1000", 600, 1000).unwrap();
        let signature = wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        subject
            .activate(
                &proposal.proposal_id,
                &signature_hex(&signature, false),
                1001,
            )
            .unwrap();
        let old_provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let new_provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let stream_key = StreamKey::make_meaningful_stream_key("rotating receipt route");
        let old_request = subject
            .request_for_stream(stream_key, 1001)
            .unwrap()
            .unwrap();
        subject
            .bind_exit_quote(&stream_key, old_provider.public_key().clone(), 5, 2, 1001)
            .unwrap();
        subject
            .record_exit_response(&stream_key, 1, 10, 1001)
            .unwrap();

        let new_request = subject
            .rotate_route_for_stream(stream_key, 1002)
            .unwrap()
            .unwrap();

        assert_ne!(new_request.route_epoch, old_request.route_epoch);
        assert_eq!(
            subject.route_epoch_for_stream(&stream_key),
            Some(new_request.route_epoch)
        );
        assert_eq!(subject.status(1002).spent_charge_wei_opt, Some(0));
        let late_old_offer = ServiceReceipt::new(
            old_request.route_epoch,
            1,
            ServiceKind::Exit,
            old_provider.public_key().clone(),
            old_request.accounting_commitment,
            10,
            5,
            2,
        )
        .sign(&old_provider)
        .unwrap();
        assert_eq!(
            subject.acknowledge_offer(late_old_offer, 1002),
            Err(ReceiptSessionManagerError::ReceiptRouteNotFound)
        );
        subject
            .bind_exit_quote(&stream_key, new_provider.public_key().clone(), 7, 3, 1002)
            .unwrap();
        subject
            .record_exit_response(&stream_key, 1, 11, 1002)
            .unwrap();
        let new_offer = ServiceReceipt::new(
            new_request.route_epoch,
            1,
            ServiceKind::Exit,
            new_provider.public_key().clone(),
            new_request.accounting_commitment,
            11,
            7,
            3,
        )
        .sign(&new_provider)
        .unwrap();
        subject.acknowledge_offer(new_offer, 1002).unwrap();
        assert_eq!(subject.status(1002).spent_charge_wei_opt, Some(40));
    }

    #[test]
    fn invalid_limits_mismatched_proposal_and_wrong_wallet_fail_closed() {
        let wallet = make_paying_wallet(b"receipt session wallet");
        let wrong_wallet = make_paying_wallet(b"wrong receipt session wallet");
        let mut subject = ReceiptSessionManager::new(config(&wallet));

        assert!(matches!(
            subject.propose("0", 600, 1000),
            Err(ReceiptSessionManagerError::Amount(_))
        ));
        assert!(matches!(
            subject.propose("1", 59, 1000),
            Err(ReceiptSessionManagerError::Duration(59))
        ));
        let proposal = subject.propose("42", 600, 1000).unwrap();
        let wrong_signature = wrong_wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        assert_eq!(
            subject.activate(
                "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                &signature_hex(&wrong_signature, false),
                1001,
            ),
            Err(ReceiptSessionManagerError::ProposalMismatch)
        );
        assert!(matches!(
            subject.activate(
                &proposal.proposal_id,
                &signature_hex(&wrong_signature, false),
                1001,
            ),
            Err(ReceiptSessionManagerError::Verification(
                ServiceReceiptError::InvalidWalletSignature
            ))
        ));
        assert!(!subject.requires_service_receipt_capability(1001));
    }

    #[test]
    fn active_session_acknowledges_a_bounded_provider_offer() {
        let wallet = make_paying_wallet(b"receipt session wallet");
        let mut subject = ReceiptSessionManager::new(config(&wallet));
        let proposal = subject.propose("100", 600, 1000).unwrap();
        let signature = wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        subject
            .activate(
                &proposal.proposal_id,
                &signature_hex(&signature, false),
                1001,
            )
            .unwrap();
        let provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let stream_key = StreamKey::make_meaningful_stream_key("bounded receipt stream");
        let route_epoch = subject
            .request_for_stream(stream_key, 1001)
            .unwrap()
            .unwrap()
            .route_epoch;
        subject
            .bind_exit_quote(&stream_key, provider.public_key().clone(), 5, 2, 1001)
            .unwrap();
        subject
            .record_exit_response(&stream_key, 1, 10, 1002)
            .unwrap();
        let receipt = ServiceReceipt::new(
            route_epoch,
            1,
            ServiceKind::Exit,
            provider.public_key().clone(),
            make_accounting_commitment(&route_epoch, &proposal.policy.payer_session_public_key),
            10,
            5,
            2,
        )
        .sign(&provider)
        .unwrap();

        let tampered_quote = ServiceReceipt::new(
            route_epoch,
            1,
            ServiceKind::Exit,
            provider.public_key().clone(),
            make_accounting_commitment(&route_epoch, &proposal.policy.payer_session_public_key),
            10,
            5,
            3,
        )
        .sign(&provider)
        .unwrap();
        assert_eq!(
            subject.acknowledge_offer(tampered_quote, 1002),
            Err(ReceiptSessionManagerError::ReceiptObservationMismatch)
        );
        assert_eq!(subject.status(1002).spent_charge_wei_opt, Some(0));

        let payload = subject.acknowledge_offer(receipt.clone(), 1002).unwrap();
        assert_eq!(
            subject.record_exit_response(&stream_key, 1, 10, 1003),
            Err(ReceiptSessionManagerError::ReceiptObservationMismatch)
        );
        let replay_payload = subject.acknowledge_offer(receipt, 1003).unwrap();

        assert_eq!(payload.authorization.policy, proposal.policy);
        assert_eq!(replay_payload, payload);
        assert_eq!(
            payload
                .acknowledged_receipt
                .signed_receipt
                .receipt
                .cumulative_charge_wei,
            25
        );
        payload
            .authorization
            .verify_for_receipt(
                &payload.acknowledged_receipt,
                &provider,
                TEST_DEFAULT_CHAIN.rec().num_chain_id,
                TEST_DEFAULT_CHAIN.rec().contract,
                1002,
            )
            .unwrap();
        assert_eq!(subject.status(1002).spent_charge_wei_opt, Some(25));
    }

    #[test]
    fn active_session_correlates_request_and_response_bytes_and_service_units() {
        let wallet = make_paying_wallet(b"aggregate request response wallet");
        let mut subject = ReceiptSessionManager::new(config(&wallet));
        let proposal = subject.propose("100", 600, 1000).unwrap();
        let signature = wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        subject
            .activate(
                &proposal.proposal_id,
                &signature_hex(&signature, false),
                1001,
            )
            .unwrap();
        let provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let stream_key = StreamKey::make_meaningful_stream_key("aggregate receipt stream");
        let request = subject
            .request_for_stream(stream_key, 1001)
            .unwrap()
            .unwrap();
        subject
            .bind_exit_quote(&stream_key, provider.public_key().clone(), 5, 2, 1001)
            .unwrap();
        subject.record_exit_request(&stream_key, 7, 1001).unwrap();
        subject.record_exit_request(&stream_key, 11, 1001).unwrap();
        subject
            .record_exit_response(&stream_key, 1, 13, 1002)
            .unwrap();

        let wrong_units = ServiceReceipt::new_with_service_units(
            request.route_epoch,
            1,
            ServiceKind::Exit,
            provider.public_key().clone(),
            request.accounting_commitment,
            31,
            2,
            5,
            2,
        )
        .sign(&provider)
        .unwrap();
        assert_eq!(
            subject.acknowledge_offer(wrong_units, 1002),
            Err(ReceiptSessionManagerError::ReceiptObservationMismatch)
        );

        let aggregate = ServiceReceipt::new_with_service_units(
            request.route_epoch,
            1,
            ServiceKind::Exit,
            provider.public_key().clone(),
            request.accounting_commitment,
            31,
            3,
            5,
            2,
        )
        .sign(&provider)
        .unwrap();
        let acknowledged = subject.acknowledge_offer(aggregate, 1002).unwrap();

        assert_eq!(
            acknowledged
                .acknowledged_receipt
                .signed_receipt
                .receipt
                .total_charge_wei(),
            77
        );
        assert_eq!(subject.status(1002).spent_charge_wei_opt, Some(77));
    }

    #[test]
    fn signer_enforces_one_aggregate_cap_across_parallel_routes() {
        let wallet = make_paying_wallet(b"receipt session wallet");
        let mut subject = ReceiptSessionManager::new(config(&wallet));
        let proposal = subject.propose("30", 600, 1000).unwrap();
        let signature = wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        subject
            .activate(
                &proposal.proposal_id,
                &signature_hex(&signature, false),
                1001,
            )
            .unwrap();
        let provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let first_stream = StreamKey::make_meaningful_stream_key("first receipt stream");
        let second_stream = StreamKey::make_meaningful_stream_key("second receipt stream");
        let first_route_epoch = subject
            .request_for_stream(first_stream, 1001)
            .unwrap()
            .unwrap()
            .route_epoch;
        let second_route_epoch = subject
            .request_for_stream(second_stream, 1001)
            .unwrap()
            .unwrap()
            .route_epoch;
        for stream_key in [&first_stream, &second_stream] {
            subject
                .bind_exit_quote(stream_key, provider.public_key().clone(), 5, 2, 1001)
                .unwrap();
            subject
                .record_exit_response(stream_key, 1, 10, 1002)
                .unwrap();
        }
        let make_offer = |route_epoch: [u8; 32]| {
            ServiceReceipt::new(
                route_epoch,
                1,
                ServiceKind::Exit,
                provider.public_key().clone(),
                make_accounting_commitment(&route_epoch, &proposal.policy.payer_session_public_key),
                10,
                5,
                2,
            )
            .sign(&provider)
            .unwrap()
        };

        subject
            .acknowledge_offer(make_offer(first_route_epoch), 1002)
            .unwrap();
        let result = subject.acknowledge_offer(make_offer(second_route_epoch), 1003);

        assert!(matches!(
            result,
            Err(ReceiptSessionManagerError::Verification(
                ServiceReceiptError::AmountLimitExceeded
            ))
        ));
        assert_eq!(subject.status(1003).spent_charge_wei_opt, Some(25));
    }

    #[test]
    fn routing_offers_are_private_exact_and_bilateral_in_both_directions() {
        use crate::sub_lib::cryptde::{encodex, CryptDE};

        let wallet = make_paying_wallet(b"bilateral routing receipt wallet");
        let mut subject = ReceiptSessionManager::new(config(&wallet));
        let proposal = subject.propose("1000000", 600, 1000).unwrap();
        let signature = wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        subject
            .activate(
                &proposal.proposal_id,
                &signature_hex(&signature, false),
                1001,
            )
            .unwrap();
        let stream_key = StreamKey::make_meaningful_stream_key("bilateral routing stream");
        let request = subject
            .request_for_stream(stream_key, 1001)
            .unwrap()
            .unwrap();
        let provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let quote = RoutingReceiptQuote {
            provider_public_key: provider.public_key().clone(),
            service_rate: 5,
            byte_rate: 2,
        };
        subject
            .bind_routing_quotes(&stream_key, vec![quote.clone()], vec![quote], 1001)
            .unwrap();

        subject
            .record_routing_request(&stream_key, 321, 1001)
            .unwrap();
        let first_receipt = ServiceReceipt::new(
            request.route_epoch,
            1,
            ServiceKind::Routing,
            provider.public_key().clone(),
            request.accounting_commitment,
            321,
            5,
            2,
        );
        let first_signed = first_receipt.clone().sign(&provider).unwrap();
        let first_encrypted = encodex(
            &provider,
            &proposal.policy.payer_session_public_key,
            &ServiceReceiptOfferPayload_0v1 {
                signed_receipt: first_signed.clone(),
            },
        )
        .unwrap();
        let unrelated_peer = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        assert!(
            decodex::<ServiceReceiptOfferPayload_0v1>(&unrelated_peer, &first_encrypted).is_err()
        );
        let decoded = subject
            .decrypt_routing_offer(&first_encrypted, 1002)
            .unwrap();
        let first_acknowledgement = subject
            .acknowledge_offer(decoded.signed_receipt, 1002)
            .unwrap();
        assert_eq!(
            first_acknowledgement
                .acknowledged_receipt
                .signed_receipt
                .receipt
                .total_charge_wei(),
            647
        );
        assert_eq!(subject.status(1002).spent_charge_wei_opt, Some(647));
        assert_eq!(
            subject.acknowledge_offer(first_signed.clone(), 1002),
            Ok(first_acknowledgement)
        );

        subject
            .record_routing_response(&stream_key, 111, 1002)
            .unwrap();
        let second_signed = first_receipt
            .next_for_same_route(2, ServiceKind::Routing, 111, 5, 2)
            .unwrap()
            .sign(&provider)
            .unwrap();
        let second_encrypted = encodex(
            &provider,
            &proposal.policy.payer_session_public_key,
            &ServiceReceiptOfferPayload_0v1 {
                signed_receipt: second_signed,
            },
        )
        .unwrap();
        let decoded = subject
            .decrypt_routing_offer(&second_encrypted, 1003)
            .unwrap();
        subject
            .acknowledge_offer(decoded.signed_receipt, 1003)
            .unwrap();
        assert_eq!(subject.status(1003).spent_charge_wei_opt, Some(874));
    }
}
