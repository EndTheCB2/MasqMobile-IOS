// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::accountant::db_access_objects::utils::to_unix_timestamp;
use crate::accountant::db_access_objects::utils::DaoFactoryReal;
use crate::accountant::db_big_integer::big_int_divider::BigIntDivider;
use crate::database::rusqlite_wrappers::ConnectionWrapper;
use crate::sub_lib::cryptde::PublicKey;
use crate::sub_lib::receipt_settlement::receipt_settlement_claim_id;
use crate::sub_lib::service_receipt::{
    make_accounting_commitment, AuthorizedProviderSettlement, AuthorizedReceiptSession,
    ReceiptSequenceCheckpoint, ServiceKind, ServiceReceiptPayload_0v1, SignedServiceReceipt,
};
use crate::sub_lib::wallet::Wallet;
use ethereum_types::Address;
use rusqlite::{Error, ErrorCode, OptionalExtension};
use serde_derive::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use std::time::SystemTime;

/// Provider-side cumulative routing state. It contains no wallet address or destination data; the
/// authorization nonce, route epoch and payer-session key are opaque protocol identifiers.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct RoutingReceiptOfferState {
    pub authorization_nonce: [u8; 32],
    pub payer_session_public_key: PublicKey,
    pub expires_at_unix_s: u64,
    pub last_signed_receipt: SignedServiceReceipt,
}

impl Debug for RoutingReceiptOfferState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("RoutingReceiptOfferState { offer_state: [REDACTED] }")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ConfirmedSettlementClaim {
    pub claim_id: [u8; 32],
    pub cumulative_charge_wei: u128,
}

impl Debug for ConfirmedSettlementClaim {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConfirmedSettlementClaim { claim_data: [REDACTED] }")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SettlementReconciliationCandidate {
    pub claim_id: [u8; 32],
    pub cumulative_charge_wei: u128,
}

impl Debug for SettlementReconciliationCandidate {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("SettlementReconciliationCandidate { claim_data: [REDACTED] }")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SettlementChainObservation {
    pub chain_id: u64,
    pub settlement_contract: Address,
    pub confirmation_depth: u64,
    pub latest_block_number: u64,
    pub observed_block_number: u64,
    pub observed_block_hash: [u8; 32],
    pub claims: Vec<ConfirmedSettlementClaim>,
}

impl Debug for SettlementChainObservation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SettlementChainObservation {{ confirmation_depth: {}, claim_count: {}, chain_evidence: [REDACTED] }}",
            self.confirmation_depth,
            self.claims.len()
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettlementReconciliationOutcome {
    pub archived_claim_count: usize,
    pub restored_claim_count: usize,
    pub still_pending_claim_count: usize,
    pub revalidated_archive_count: usize,
    pub unknown_claim_count: usize,
}

#[derive(Clone, Eq, PartialEq)]
pub struct PendingSettlementClaimRecord {
    pub receipt_payload: ServiceReceiptPayload_0v1,
    pub accepted_at_unix_s: u64,
}

impl Debug for PendingSettlementClaimRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("PendingSettlementClaimRecord { claim_record: [REDACTED] }")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum ReceiptCheckpointDaoError {
    AmountLimitExceeded,
    AuthorizationIdentityMismatch,
    AuthorizationNonceAlreadyUsed,
    BalanceOverflow,
    CheckpointIdentityMismatch,
    Database(String),
    Deserialization(String),
    OfferStateCapacityExceeded,
    OfferStateIdentityMismatch,
    SettlementClaimIdentityMismatch,
    SettlementConfirmationDepthReduced,
    SettlementObservationIdentityMismatch,
    SettlementObservationInvalid,
    SettlementObservationRegressed,
    Serialization(String),
    StaleCheckpoint,
    StaleOfferState,
    StoredAmountInvalid(String),
}

impl Debug for ReceiptCheckpointDaoError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let category = match self {
            Self::AmountLimitExceeded => "AmountLimitExceeded",
            Self::AuthorizationIdentityMismatch => "AuthorizationIdentityMismatch",
            Self::AuthorizationNonceAlreadyUsed => "AuthorizationNonceAlreadyUsed",
            Self::BalanceOverflow => "BalanceOverflow",
            Self::CheckpointIdentityMismatch => "CheckpointIdentityMismatch",
            Self::OfferStateCapacityExceeded => "OfferStateCapacityExceeded",
            Self::OfferStateIdentityMismatch => "OfferStateIdentityMismatch",
            Self::SettlementClaimIdentityMismatch => "SettlementClaimIdentityMismatch",
            Self::SettlementConfirmationDepthReduced => "SettlementConfirmationDepthReduced",
            Self::SettlementObservationIdentityMismatch => "SettlementObservationIdentityMismatch",
            Self::SettlementObservationInvalid => "SettlementObservationInvalid",
            Self::SettlementObservationRegressed => "SettlementObservationRegressed",
            Self::StaleCheckpoint => "StaleCheckpoint",
            Self::StaleOfferState => "StaleOfferState",
            Self::Database(_) => "Database([REDACTED])",
            Self::Deserialization(_) => "Deserialization([REDACTED])",
            Self::Serialization(_) => "Serialization([REDACTED])",
            Self::StoredAmountInvalid(_) => "StoredAmountInvalid([REDACTED])",
        };
        f.write_str(category)
    }
}

pub trait ReceiptCheckpointDao: Send {
    fn checkpoint(
        &self,
        route_epoch: &[u8; 32],
        provider_public_key: &PublicKey,
        payer_session_public_key: &PublicKey,
    ) -> Result<Option<ReceiptSequenceCheckpoint>, ReceiptCheckpointDaoError>;

    fn save_checkpoint(
        &mut self,
        checkpoint: &ReceiptSequenceCheckpoint,
    ) -> Result<(), ReceiptCheckpointDaoError>;

    fn authorization(
        &self,
        authorization_nonce: &[u8; 32],
    ) -> Result<Option<AuthorizedReceiptSession>, ReceiptCheckpointDaoError>;

    fn save_authorization(
        &mut self,
        authorization: &AuthorizedReceiptSession,
    ) -> Result<(), ReceiptCheckpointDaoError>;

    fn routing_offer_state(
        &self,
        authorization_nonce: &[u8; 32],
        route_epoch: &[u8; 32],
        provider_public_key: &PublicKey,
        payer_session_public_key: &PublicKey,
    ) -> Result<Option<RoutingReceiptOfferState>, ReceiptCheckpointDaoError>;

    /// Prunes expired states and durably inserts or advances one cumulative offer in a single
    /// transaction. A caller must persist before putting the corresponding offer on the wire.
    fn save_routing_offer_state(
        &mut self,
        state: &RoutingReceiptOfferState,
        now_unix_s: u64,
        maximum_states: usize,
    ) -> Result<(), ReceiptCheckpointDaoError>;

    /// Atomically records a previously cryptographically verified checkpoint and its exact
    /// receivable delta. The authorization's aggregate cap spans every route using the same
    /// nonce, preventing a payer from multiplying the cap with parallel route epochs.
    fn accept_verified_receipt(
        &mut self,
        payload: &ServiceReceiptPayload_0v1,
        checkpoint: &ReceiptSequenceCheckpoint,
        timestamp: SystemTime,
    ) -> Result<u128, ReceiptCheckpointDaoError>;

    /// Returns the latest fully signed cumulative receipt for every unsettled economic route.
    /// These payloads are written in the same transaction as the checkpoint and receivable.
    fn pending_settlement_claims(
        &self,
    ) -> Result<Vec<ServiceReceiptPayload_0v1>, ReceiptCheckpointDaoError>;

    fn pending_settlement_claims_page(
        &self,
        start_after_claim_id_opt: Option<[u8; 32]>,
        limit: usize,
    ) -> Result<Vec<ServiceReceiptPayload_0v1>, ReceiptCheckpointDaoError>;

    fn pending_settlement_claim_records_page(
        &self,
        start_after_claim_id_opt: Option<[u8; 32]>,
        limit: usize,
    ) -> Result<Vec<PendingSettlementClaimRecord>, ReceiptCheckpointDaoError>;

    /// Pages over both pending and previously confirmed claims. Archived claims remain candidates
    /// so a later deep reorg can be detected and reversed.
    fn settlement_reconciliation_candidates_page(
        &self,
        start_after_claim_id_opt: Option<[u8; 32]>,
        limit: usize,
    ) -> Result<Vec<SettlementReconciliationCandidate>, ReceiptCheckpointDaoError>;

    /// Applies contract state read at one immutable, confirmation-deep block. Confirmed pending
    /// rows move to a proof-preserving archive; archived rows whose state disappeared move back to
    /// the outbox. Regressing block observations and confirmation-depth downgrades fail closed.
    fn reconcile_settlement_claims(
        &mut self,
        observation: &SettlementChainObservation,
    ) -> Result<SettlementReconciliationOutcome, ReceiptCheckpointDaoError>;

    fn provider_settlement_authorization(
        &self,
    ) -> Result<Option<AuthorizedProviderSettlement>, ReceiptCheckpointDaoError>;

    fn save_provider_settlement_authorization(
        &mut self,
        authorization: &AuthorizedProviderSettlement,
    ) -> Result<(), ReceiptCheckpointDaoError>;

    fn clear_provider_settlement_authorization(&mut self) -> Result<(), ReceiptCheckpointDaoError>;
}

pub trait ReceiptCheckpointDaoFactory {
    fn make(&self) -> Box<dyn ReceiptCheckpointDao>;
}

impl ReceiptCheckpointDaoFactory for DaoFactoryReal {
    fn make(&self) -> Box<dyn ReceiptCheckpointDao> {
        Box::new(ReceiptCheckpointDaoReal::new(self.make_connection()))
    }
}

pub struct ReceiptCheckpointDaoReal {
    conn: Box<dyn ConnectionWrapper>,
}

impl ReceiptCheckpointDaoReal {
    pub fn new(conn: Box<dyn ConnectionWrapper>) -> Self {
        Self { conn }
    }

    fn database_error(error: Error) -> ReceiptCheckpointDaoError {
        ReceiptCheckpointDaoError::Database(error.to_string())
    }

    fn fixed_32(bytes: Vec<u8>, field: &str) -> Result<[u8; 32], ReceiptCheckpointDaoError> {
        if bytes.len() != 32 {
            return Err(ReceiptCheckpointDaoError::Deserialization(format!(
                "{} must contain 32 bytes, received {}",
                field,
                bytes.len()
            )));
        }
        let mut value = [0u8; 32];
        value.copy_from_slice(&bytes);
        Ok(value)
    }

    fn stored_u64(value: &str) -> Result<u64, ReceiptCheckpointDaoError> {
        value
            .parse::<u64>()
            .map_err(|_| ReceiptCheckpointDaoError::StoredAmountInvalid(value.to_string()))
    }

    fn stored_u128(value: &str) -> Result<u128, ReceiptCheckpointDaoError> {
        value
            .parse::<u128>()
            .map_err(|_| ReceiptCheckpointDaoError::StoredAmountInvalid(value.to_string()))
    }

    fn deserialize_checkpoint(
        serialized: &[u8],
    ) -> Result<ReceiptSequenceCheckpoint, ReceiptCheckpointDaoError> {
        serde_cbor::from_slice(serialized)
            .map_err(|error| ReceiptCheckpointDaoError::Deserialization(error.to_string()))
    }

    fn deserialize_authorization(
        serialized: &[u8],
    ) -> Result<AuthorizedReceiptSession, ReceiptCheckpointDaoError> {
        serde_cbor::from_slice(serialized)
            .map_err(|error| ReceiptCheckpointDaoError::Deserialization(error.to_string()))
    }

    fn deserialize_routing_offer_state(
        authorization_nonce: &[u8],
        route_epoch: &[u8],
        provider_public_key: &[u8],
        payer_session_public_key: &[u8],
        expires_at_unix_s: &str,
        serialized: &[u8],
    ) -> Result<RoutingReceiptOfferState, ReceiptCheckpointDaoError> {
        let state: RoutingReceiptOfferState = serde_cbor::from_slice(serialized)
            .map_err(|error| ReceiptCheckpointDaoError::Deserialization(error.to_string()))?;
        let receipt = &state.last_signed_receipt.receipt;
        let stored_expiry = expires_at_unix_s
            .parse::<u64>()
            .map_err(|error| ReceiptCheckpointDaoError::StoredAmountInvalid(error.to_string()))?;
        if state.authorization_nonce.as_slice() != authorization_nonce
            || receipt.route_epoch.as_slice() != route_epoch
            || receipt.provider_public_key.as_slice() != provider_public_key
            || state.payer_session_public_key.as_slice() != payer_session_public_key
            || state.expires_at_unix_s != stored_expiry
            || receipt.service_kind != ServiceKind::Routing
        {
            return Err(ReceiptCheckpointDaoError::OfferStateIdentityMismatch);
        }
        Ok(state)
    }

    fn deserialize_settlement_claim(
        claim_id: &[u8],
        authorization_nonce: &[u8],
        route_epoch: &[u8],
        provider_public_key: &[u8],
        payer_session_public_key: &[u8],
        cumulative_charge_wei: &str,
        serialized: &[u8],
    ) -> Result<ServiceReceiptPayload_0v1, ReceiptCheckpointDaoError> {
        let payload: ServiceReceiptPayload_0v1 = serde_cbor::from_slice(serialized)
            .map_err(|error| ReceiptCheckpointDaoError::Deserialization(error.to_string()))?;
        let authorization = &payload.authorization.policy;
        let acknowledgement = &payload.acknowledged_receipt;
        let receipt = &acknowledgement.signed_receipt.receipt;
        let stored_charge = cumulative_charge_wei.parse::<u128>().map_err(|_| {
            ReceiptCheckpointDaoError::StoredAmountInvalid(cumulative_charge_wei.to_string())
        })?;
        if authorization.authorization_nonce.as_slice() != authorization_nonce
            || receipt.route_epoch.as_slice() != route_epoch
            || receipt.provider_public_key.as_slice() != provider_public_key
            || acknowledgement.payer_session_public_key.as_slice() != payer_session_public_key
            || authorization.payer_session_public_key != acknowledgement.payer_session_public_key
            || receipt.cumulative_charge_wei != stored_charge
        {
            return Err(ReceiptCheckpointDaoError::SettlementClaimIdentityMismatch);
        }
        let computed_claim_id = receipt_settlement_claim_id(&payload)
            .map_err(|error| ReceiptCheckpointDaoError::Deserialization(format!("{:?}", error)))?;
        if computed_claim_id.as_slice() != claim_id {
            return Err(ReceiptCheckpointDaoError::SettlementClaimIdentityMismatch);
        }
        Ok(payload)
    }

    fn validate_settlement_claim_for_checkpoint(
        payload: &ServiceReceiptPayload_0v1,
        checkpoint: &ReceiptSequenceCheckpoint,
    ) -> Result<(), ReceiptCheckpointDaoError> {
        let authorization = &payload.authorization.policy;
        let acknowledgement = &payload.acknowledged_receipt;
        let receipt = &acknowledgement.signed_receipt.receipt;
        if authorization.payer_session_public_key != checkpoint.payer_session_public_key
            || acknowledgement.payer_session_public_key != checkpoint.payer_session_public_key
            || receipt.route_epoch != checkpoint.route_epoch
            || receipt.provider_public_key != checkpoint.provider_public_key
            || receipt.accounting_commitment != checkpoint.accounting_commitment
            || receipt.sequence != checkpoint.last_sequence
            || receipt.cumulative_charge_wei != checkpoint.cumulative_charge_wei
        {
            return Err(ReceiptCheckpointDaoError::SettlementClaimIdentityMismatch);
        }
        Ok(())
    }
}

impl ReceiptCheckpointDao for ReceiptCheckpointDaoReal {
    fn checkpoint(
        &self,
        route_epoch: &[u8; 32],
        provider_public_key: &PublicKey,
        payer_session_public_key: &PublicKey,
    ) -> Result<Option<ReceiptSequenceCheckpoint>, ReceiptCheckpointDaoError> {
        let mut statement = self
            .conn
            .prepare(
                "select checkpoint_cbor from receipt_sequence_checkpoint
                 where route_epoch = ?1 and provider_public_key = ?2
                   and payer_session_public_key = ?3",
            )
            .map_err(Self::database_error)?;
        let serialized_opt = statement
            .query_row(
                rusqlite::params![
                    &route_epoch[..],
                    provider_public_key.as_slice(),
                    payer_session_public_key.as_slice()
                ],
                |row| row.get::<usize, Vec<u8>>(0),
            )
            .optional()
            .map_err(Self::database_error)?;
        serialized_opt
            .map(|serialized| Self::deserialize_checkpoint(&serialized))
            .transpose()
    }

    fn save_checkpoint(
        &mut self,
        checkpoint: &ReceiptSequenceCheckpoint,
    ) -> Result<(), ReceiptCheckpointDaoError> {
        let serialized = serde_cbor::to_vec(checkpoint)
            .map_err(|error| ReceiptCheckpointDaoError::Serialization(error.to_string()))?;
        let transaction = self.conn.transaction().map_err(Self::database_error)?;
        let existing_serialized_opt = {
            let mut statement = transaction
                .prepare(
                    "select checkpoint_cbor from receipt_sequence_checkpoint
                     where route_epoch = ?1 and provider_public_key = ?2
                       and payer_session_public_key = ?3",
                )
                .map_err(Self::database_error)?;
            statement
                .query_row(
                    rusqlite::params![
                        &checkpoint.route_epoch[..],
                        checkpoint.provider_public_key.as_slice(),
                        checkpoint.payer_session_public_key.as_slice()
                    ],
                    |row| row.get::<usize, Vec<u8>>(0),
                )
                .optional()
                .map_err(Self::database_error)?
        };

        match existing_serialized_opt {
            Some(existing_serialized) => {
                let existing = Self::deserialize_checkpoint(&existing_serialized)?;
                if checkpoint.route_epoch != existing.route_epoch
                    || checkpoint.provider_public_key != existing.provider_public_key
                    || checkpoint.payer_session_public_key != existing.payer_session_public_key
                    || checkpoint.accounting_commitment != existing.accounting_commitment
                {
                    return Err(ReceiptCheckpointDaoError::CheckpointIdentityMismatch);
                }
                if checkpoint.last_sequence <= existing.last_sequence
                    || checkpoint.cumulative_charge_wei <= existing.cumulative_charge_wei
                {
                    return Err(ReceiptCheckpointDaoError::StaleCheckpoint);
                }
                transaction
                    .execute(
                        "update receipt_sequence_checkpoint
                         set last_sequence = ?1, cumulative_charge_wei = ?2, checkpoint_cbor = ?3
                         where route_epoch = ?4 and provider_public_key = ?5
                           and payer_session_public_key = ?6",
                        &[
                            &checkpoint.last_sequence.to_string(),
                            &checkpoint.cumulative_charge_wei.to_string(),
                            &serialized,
                            &&checkpoint.route_epoch[..],
                            &checkpoint.provider_public_key.as_slice(),
                            &checkpoint.payer_session_public_key.as_slice(),
                        ],
                    )
                    .map_err(Self::database_error)?;
            }
            None => {
                transaction
                    .execute(
                        "insert into receipt_sequence_checkpoint
                         (route_epoch, provider_public_key, payer_session_public_key,
                          last_sequence, cumulative_charge_wei, checkpoint_cbor)
                         values (?1, ?2, ?3, ?4, ?5, ?6)",
                        &[
                            &&checkpoint.route_epoch[..],
                            &checkpoint.provider_public_key.as_slice(),
                            &checkpoint.payer_session_public_key.as_slice(),
                            &checkpoint.last_sequence.to_string(),
                            &checkpoint.cumulative_charge_wei.to_string(),
                            &serialized,
                        ],
                    )
                    .map_err(Self::database_error)?;
            }
        }
        transaction.commit().map_err(Self::database_error)
    }

    fn routing_offer_state(
        &self,
        authorization_nonce: &[u8; 32],
        route_epoch: &[u8; 32],
        provider_public_key: &PublicKey,
        payer_session_public_key: &PublicKey,
    ) -> Result<Option<RoutingReceiptOfferState>, ReceiptCheckpointDaoError> {
        let mut statement = self
            .conn
            .prepare(
                "select expires_at_unix_s, offer_state_cbor
                 from routing_receipt_offer_state
                 where authorization_nonce = ?1 and route_epoch = ?2
                   and provider_public_key = ?3 and payer_session_public_key = ?4",
            )
            .map_err(Self::database_error)?;
        let row_opt = statement
            .query_row(
                rusqlite::params![
                    authorization_nonce.as_slice(),
                    route_epoch.as_slice(),
                    provider_public_key.as_slice(),
                    payer_session_public_key.as_slice(),
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()
            .map_err(Self::database_error)?;
        row_opt
            .map(|(expiry, serialized)| {
                Self::deserialize_routing_offer_state(
                    authorization_nonce.as_slice(),
                    route_epoch.as_slice(),
                    provider_public_key.as_slice(),
                    payer_session_public_key.as_slice(),
                    &expiry,
                    &serialized,
                )
            })
            .transpose()
    }

    fn save_routing_offer_state(
        &mut self,
        state: &RoutingReceiptOfferState,
        now_unix_s: u64,
        maximum_states: usize,
    ) -> Result<(), ReceiptCheckpointDaoError> {
        let receipt = &state.last_signed_receipt.receipt;
        if receipt.service_kind != ServiceKind::Routing
            || state.authorization_nonce == [0u8; 32]
            || state.payer_session_public_key.as_slice().is_empty()
            || receipt.route_epoch == [0u8; 32]
            || receipt.provider_public_key.as_slice().is_empty()
            || state.expires_at_unix_s < now_unix_s
        {
            return Err(ReceiptCheckpointDaoError::OfferStateIdentityMismatch);
        }
        let serialized = serde_cbor::to_vec(state)
            .map_err(|error| ReceiptCheckpointDaoError::Serialization(error.to_string()))?;
        let transaction = self.conn.transaction().map_err(Self::database_error)?;
        transaction
            .execute(
                "delete from routing_receipt_offer_state
                 where cast(expires_at_unix_s as integer) < ?1",
                rusqlite::params![now_unix_s.to_string()],
            )
            .map_err(Self::database_error)?;
        let existing_opt: Option<(String, Vec<u8>)> = {
            let mut statement = transaction
                .prepare(
                    "select expires_at_unix_s, offer_state_cbor
                     from routing_receipt_offer_state
                     where authorization_nonce = ?1 and route_epoch = ?2
                       and provider_public_key = ?3 and payer_session_public_key = ?4",
                )
                .map_err(Self::database_error)?;
            statement
                .query_row(
                    rusqlite::params![
                        state.authorization_nonce.as_slice(),
                        receipt.route_epoch.as_slice(),
                        receipt.provider_public_key.as_slice(),
                        state.payer_session_public_key.as_slice(),
                    ],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )
                .optional()
                .map_err(Self::database_error)?
        };
        match existing_opt {
            Some((existing_expiry, existing_serialized)) => {
                let existing = Self::deserialize_routing_offer_state(
                    state.authorization_nonce.as_slice(),
                    receipt.route_epoch.as_slice(),
                    receipt.provider_public_key.as_slice(),
                    state.payer_session_public_key.as_slice(),
                    &existing_expiry,
                    &existing_serialized,
                )?;
                if existing == *state {
                    transaction.commit().map_err(Self::database_error)?;
                    return Ok(());
                }
                let previous = &existing.last_signed_receipt.receipt;
                if state.expires_at_unix_s != existing.expires_at_unix_s
                    || receipt.accounting_commitment != previous.accounting_commitment
                    || receipt.service_rate != previous.service_rate
                    || receipt.byte_rate != previous.byte_rate
                    || previous.sequence.checked_add(1) != Some(receipt.sequence)
                    || receipt.cumulative_charge_wei <= previous.cumulative_charge_wei
                {
                    return Err(ReceiptCheckpointDaoError::StaleOfferState);
                }
                let changed = transaction
                    .execute(
                        "update routing_receipt_offer_state
                         set expires_at_unix_s = ?1, offer_state_cbor = ?2
                         where authorization_nonce = ?3 and route_epoch = ?4
                           and provider_public_key = ?5 and payer_session_public_key = ?6
                           and offer_state_cbor = ?7",
                        rusqlite::params![
                            state.expires_at_unix_s.to_string(),
                            &serialized,
                            state.authorization_nonce.as_slice(),
                            receipt.route_epoch.as_slice(),
                            receipt.provider_public_key.as_slice(),
                            state.payer_session_public_key.as_slice(),
                            &existing_serialized,
                        ],
                    )
                    .map_err(Self::database_error)?;
                if changed != 1 {
                    return Err(ReceiptCheckpointDaoError::Database(
                        "routing offer state changed during replacement".to_string(),
                    ));
                }
            }
            None => {
                if receipt.sequence != 1 {
                    return Err(ReceiptCheckpointDaoError::StaleOfferState);
                }
                let state_count: i64 = transaction
                    .prepare("select count(*) from routing_receipt_offer_state")
                    .map_err(Self::database_error)?
                    .query_row([], |row| row.get(0))
                    .map_err(Self::database_error)?;
                if usize::try_from(state_count).unwrap_or(usize::MAX) >= maximum_states {
                    return Err(ReceiptCheckpointDaoError::OfferStateCapacityExceeded);
                }
                transaction
                    .execute(
                        "insert into routing_receipt_offer_state
                         (authorization_nonce, route_epoch, provider_public_key,
                          payer_session_public_key, expires_at_unix_s, offer_state_cbor)
                         values (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            state.authorization_nonce.as_slice(),
                            receipt.route_epoch.as_slice(),
                            receipt.provider_public_key.as_slice(),
                            state.payer_session_public_key.as_slice(),
                            state.expires_at_unix_s.to_string(),
                            &serialized,
                        ],
                    )
                    .map_err(Self::database_error)?;
            }
        }
        transaction.commit().map_err(Self::database_error)
    }

    fn authorization(
        &self,
        authorization_nonce: &[u8; 32],
    ) -> Result<Option<AuthorizedReceiptSession>, ReceiptCheckpointDaoError> {
        let mut statement = self
            .conn
            .prepare(
                "select authorization_cbor from receipt_session_authorization
                 where authorization_nonce = ?1",
            )
            .map_err(Self::database_error)?;
        let serialized_opt = statement
            .query_row(rusqlite::params![&authorization_nonce[..]], |row| {
                row.get::<usize, Vec<u8>>(0)
            })
            .optional()
            .map_err(Self::database_error)?;
        serialized_opt
            .map(|serialized| Self::deserialize_authorization(&serialized))
            .transpose()
    }

    fn save_authorization(
        &mut self,
        authorization: &AuthorizedReceiptSession,
    ) -> Result<(), ReceiptCheckpointDaoError> {
        let serialized = serde_cbor::to_vec(authorization)
            .map_err(|error| ReceiptCheckpointDaoError::Serialization(error.to_string()))?;
        let transaction = self.conn.transaction().map_err(Self::database_error)?;
        match transaction.execute(
            "insert into receipt_session_authorization
             (authorization_nonce, expires_at_unix_s, authorization_cbor)
             values (?1, ?2, ?3)",
            &[
                &&authorization.policy.authorization_nonce[..],
                &authorization.policy.expires_at_unix_s.to_string(),
                &serialized,
            ],
        ) {
            Ok(_) => transaction.commit().map_err(Self::database_error),
            Err(Error::SqliteFailure(error, _)) if error.code == ErrorCode::ConstraintViolation => {
                Err(ReceiptCheckpointDaoError::AuthorizationNonceAlreadyUsed)
            }
            Err(error) => Err(Self::database_error(error)),
        }
    }

    fn accept_verified_receipt(
        &mut self,
        payload: &ServiceReceiptPayload_0v1,
        checkpoint: &ReceiptSequenceCheckpoint,
        timestamp: SystemTime,
    ) -> Result<u128, ReceiptCheckpointDaoError> {
        const MAX_DB_BALANCE: i128 = (1i128 << 126) - 1;

        Self::validate_settlement_claim_for_checkpoint(payload, checkpoint)?;
        let authorization = &payload.authorization;

        if authorization.policy.payer_session_public_key != checkpoint.payer_session_public_key
            || make_accounting_commitment(
                &checkpoint.route_epoch,
                &checkpoint.payer_session_public_key,
            ) != checkpoint.accounting_commitment
        {
            return Err(ReceiptCheckpointDaoError::AuthorizationIdentityMismatch);
        }

        let authorization_serialized = serde_cbor::to_vec(authorization)
            .map_err(|error| ReceiptCheckpointDaoError::Serialization(error.to_string()))?;
        let checkpoint_serialized = serde_cbor::to_vec(checkpoint)
            .map_err(|error| ReceiptCheckpointDaoError::Serialization(error.to_string()))?;
        let claim_serialized = serde_cbor::to_vec(payload)
            .map_err(|error| ReceiptCheckpointDaoError::Serialization(error.to_string()))?;
        let claim_id = receipt_settlement_claim_id(payload)
            .map_err(|error| ReceiptCheckpointDaoError::Serialization(format!("{:?}", error)))?;
        let transaction = self.conn.transaction().map_err(Self::database_error)?;

        let stored_claim_opt: Option<(Vec<u8>, Vec<u8>, String, Vec<u8>)> = {
            let mut statement = transaction
                .prepare(
                    "select claim_id, authorization_nonce, cumulative_charge_wei,
                            receipt_payload_cbor
                     from receipt_settlement_claim_outbox
                     where route_epoch = ?1 and provider_public_key = ?2
                       and payer_session_public_key = ?3",
                )
                .map_err(Self::database_error)?;
            statement
                .query_row(
                    rusqlite::params![
                        &checkpoint.route_epoch[..],
                        checkpoint.provider_public_key.as_slice(),
                        checkpoint.payer_session_public_key.as_slice()
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(Self::database_error)?
        };
        if let Some((stored_claim_id, stored_nonce, stored_charge, stored_serialized)) =
            stored_claim_opt.as_ref()
        {
            let stored_payload = Self::deserialize_settlement_claim(
                stored_claim_id,
                stored_nonce,
                &checkpoint.route_epoch,
                checkpoint.provider_public_key.as_slice(),
                checkpoint.payer_session_public_key.as_slice(),
                stored_charge,
                stored_serialized,
            )?;
            if stored_payload.authorization != *authorization {
                return Err(ReceiptCheckpointDaoError::SettlementClaimIdentityMismatch);
            }
        }

        let stored_authorization_opt = {
            let mut statement = transaction
                .prepare(
                    "select authorization_cbor, spent_charge_wei
                     from receipt_session_authorization where authorization_nonce = ?1",
                )
                .map_err(Self::database_error)?;
            statement
                .query_row(
                    rusqlite::params![&authorization.policy.authorization_nonce[..]],
                    |row| Ok((row.get::<usize, Vec<u8>>(0)?, row.get::<usize, String>(1)?)),
                )
                .optional()
                .map_err(Self::database_error)?
        };
        let (authorization_already_stored, spent_charge_wei) = match stored_authorization_opt {
            Some((serialized, spent)) => {
                let stored = Self::deserialize_authorization(&serialized)?;
                if stored != *authorization {
                    return Err(ReceiptCheckpointDaoError::AuthorizationIdentityMismatch);
                }
                let spent = spent
                    .parse::<u128>()
                    .map_err(|_| ReceiptCheckpointDaoError::StoredAmountInvalid(spent.clone()))?;
                (true, spent)
            }
            None => (false, 0),
        };

        let stored_checkpoint_opt = {
            let mut statement = transaction
                .prepare(
                    "select checkpoint_cbor from receipt_sequence_checkpoint
                     where route_epoch = ?1 and provider_public_key = ?2
                       and payer_session_public_key = ?3",
                )
                .map_err(Self::database_error)?;
            statement
                .query_row(
                    rusqlite::params![
                        &checkpoint.route_epoch[..],
                        checkpoint.provider_public_key.as_slice(),
                        checkpoint.payer_session_public_key.as_slice()
                    ],
                    |row| row.get::<usize, Vec<u8>>(0),
                )
                .optional()
                .map_err(Self::database_error)?
        };

        let checkpoint_already_stored = stored_checkpoint_opt.is_some();
        let charge_delta = match stored_checkpoint_opt {
            Some(serialized) => {
                let stored = Self::deserialize_checkpoint(&serialized)?;
                if checkpoint.route_epoch != stored.route_epoch
                    || checkpoint.provider_public_key != stored.provider_public_key
                    || checkpoint.payer_session_public_key != stored.payer_session_public_key
                    || checkpoint.accounting_commitment != stored.accounting_commitment
                {
                    return Err(ReceiptCheckpointDaoError::CheckpointIdentityMismatch);
                }
                if checkpoint.last_sequence <= stored.last_sequence
                    || checkpoint.cumulative_charge_wei <= stored.cumulative_charge_wei
                {
                    return Err(ReceiptCheckpointDaoError::StaleCheckpoint);
                }
                checkpoint
                    .cumulative_charge_wei
                    .checked_sub(stored.cumulative_charge_wei)
                    .ok_or(ReceiptCheckpointDaoError::StaleCheckpoint)?
            }
            None => checkpoint.cumulative_charge_wei,
        };
        let new_spent_charge_wei = spent_charge_wei
            .checked_add(charge_delta)
            .ok_or(ReceiptCheckpointDaoError::AmountLimitExceeded)?;
        if new_spent_charge_wei > authorization.policy.max_total_charge_wei {
            return Err(ReceiptCheckpointDaoError::AmountLimitExceeded);
        }

        if authorization_already_stored {
            transaction
                .execute(
                    "update receipt_session_authorization set spent_charge_wei = ?1
                     where authorization_nonce = ?2",
                    rusqlite::params![
                        new_spent_charge_wei.to_string(),
                        &authorization.policy.authorization_nonce[..]
                    ],
                )
                .map_err(Self::database_error)?;
        } else {
            transaction
                .execute(
                    "insert into receipt_session_authorization
                     (authorization_nonce, expires_at_unix_s, spent_charge_wei, authorization_cbor)
                     values (?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        &authorization.policy.authorization_nonce[..],
                        authorization.policy.expires_at_unix_s.to_string(),
                        new_spent_charge_wei.to_string(),
                        &authorization_serialized
                    ],
                )
                .map_err(Self::database_error)?;
        }

        if checkpoint_already_stored {
            transaction
                .execute(
                    "update receipt_sequence_checkpoint
                     set last_sequence = ?1, cumulative_charge_wei = ?2, checkpoint_cbor = ?3
                     where route_epoch = ?4 and provider_public_key = ?5
                       and payer_session_public_key = ?6",
                    rusqlite::params![
                        checkpoint.last_sequence.to_string(),
                        checkpoint.cumulative_charge_wei.to_string(),
                        &checkpoint_serialized,
                        &checkpoint.route_epoch[..],
                        checkpoint.provider_public_key.as_slice(),
                        checkpoint.payer_session_public_key.as_slice()
                    ],
                )
                .map_err(Self::database_error)?;
        } else {
            transaction
                .execute(
                    "insert into receipt_sequence_checkpoint
                     (route_epoch, provider_public_key, payer_session_public_key,
                      last_sequence, cumulative_charge_wei, checkpoint_cbor)
                     values (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        &checkpoint.route_epoch[..],
                        checkpoint.provider_public_key.as_slice(),
                        checkpoint.payer_session_public_key.as_slice(),
                        checkpoint.last_sequence.to_string(),
                        checkpoint.cumulative_charge_wei.to_string(),
                        &checkpoint_serialized
                    ],
                )
                .map_err(Self::database_error)?;
        }

        let payer_wallet = Wallet::from(authorization.policy.payer_wallet_address);
        let existing_balance_parts_opt = {
            let mut statement = transaction
                .prepare(
                    "select balance_high_b, balance_low_b from receivable
                     where wallet_address = ?1",
                )
                .map_err(Self::database_error)?;
            statement
                .query_row(rusqlite::params![&payer_wallet], |row| {
                    Ok((row.get::<usize, i64>(0)?, row.get::<usize, i64>(1)?))
                })
                .optional()
                .map_err(Self::database_error)?
        };
        let charge_delta_i128 =
            i128::try_from(charge_delta).map_err(|_| ReceiptCheckpointDaoError::BalanceOverflow)?;
        let existing_balance = existing_balance_parts_opt
            .map(|parts| BigIntDivider::reconstitute(parts.0, parts.1))
            .unwrap_or(0);
        let new_balance = existing_balance
            .checked_add(charge_delta_i128)
            .filter(|balance| *balance <= MAX_DB_BALANCE)
            .ok_or(ReceiptCheckpointDaoError::BalanceOverflow)?;
        let (new_balance_high_b, new_balance_low_b) = BigIntDivider::deconstruct(new_balance);
        match existing_balance_parts_opt {
            Some(_) => transaction
                .execute(
                    "update receivable set balance_high_b = ?1, balance_low_b = ?2
                     where wallet_address = ?3",
                    rusqlite::params![new_balance_high_b, new_balance_low_b, &payer_wallet],
                )
                .map_err(Self::database_error)?,
            None => transaction
                .execute(
                    "insert into receivable
                     (wallet_address, balance_high_b, balance_low_b, last_received_timestamp)
                     values (?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        &payer_wallet,
                        new_balance_high_b,
                        new_balance_low_b,
                        to_unix_timestamp(timestamp)
                    ],
                )
                .map_err(Self::database_error)?,
        };

        // A later cumulative receipt reopens an archived stream. Keeping both copies would make
        // reconciliation ambiguous after a concurrent settlement/export cycle.
        transaction
            .execute(
                "delete from receipt_settlement_claim_archive where claim_id = ?1",
                rusqlite::params![&claim_id[..]],
            )
            .map_err(Self::database_error)?;

        match stored_claim_opt {
            Some(_) => transaction
                .execute(
                    "update receipt_settlement_claim_outbox
                     set cumulative_charge_wei = ?1, receipt_payload_cbor = ?2,
                         accepted_at_unix_s = ?3
                     where route_epoch = ?4 and provider_public_key = ?5
                       and payer_session_public_key = ?6",
                    rusqlite::params![
                        checkpoint.cumulative_charge_wei.to_string(),
                        &claim_serialized,
                        to_unix_timestamp(timestamp).to_string(),
                        &checkpoint.route_epoch[..],
                        checkpoint.provider_public_key.as_slice(),
                        checkpoint.payer_session_public_key.as_slice()
                    ],
                )
                .map_err(Self::database_error)?,
            None => transaction
                .execute(
                    "insert into receipt_settlement_claim_outbox
                     (claim_id, authorization_nonce, route_epoch, provider_public_key,
                      payer_session_public_key, cumulative_charge_wei,
                      receipt_payload_cbor, accepted_at_unix_s)
                     values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        &claim_id[..],
                        &authorization.policy.authorization_nonce[..],
                        &checkpoint.route_epoch[..],
                        checkpoint.provider_public_key.as_slice(),
                        checkpoint.payer_session_public_key.as_slice(),
                        checkpoint.cumulative_charge_wei.to_string(),
                        &claim_serialized,
                        to_unix_timestamp(timestamp).to_string()
                    ],
                )
                .map_err(Self::database_error)?,
        };

        transaction.commit().map_err(Self::database_error)?;
        Ok(charge_delta)
    }

    fn pending_settlement_claims(
        &self,
    ) -> Result<Vec<ServiceReceiptPayload_0v1>, ReceiptCheckpointDaoError> {
        self.pending_settlement_claims_page(None, usize::MAX)
    }

    fn pending_settlement_claims_page(
        &self,
        start_after_claim_id_opt: Option<[u8; 32]>,
        limit: usize,
    ) -> Result<Vec<ServiceReceiptPayload_0v1>, ReceiptCheckpointDaoError> {
        self.pending_settlement_claim_records_page(start_after_claim_id_opt, limit)
            .map(|records| {
                records
                    .into_iter()
                    .map(|record| record.receipt_payload)
                    .collect()
            })
    }

    fn pending_settlement_claim_records_page(
        &self,
        start_after_claim_id_opt: Option<[u8; 32]>,
        limit: usize,
    ) -> Result<Vec<PendingSettlementClaimRecord>, ReceiptCheckpointDaoError> {
        if limit == 0 {
            return Ok(vec![]);
        }
        let mut statement = self
            .conn
            .prepare(
                "select claim_id, authorization_nonce, route_epoch, provider_public_key,
                        payer_session_public_key, cumulative_charge_wei, receipt_payload_cbor,
                        accepted_at_unix_s
                 from receipt_settlement_claim_outbox
                 where (?1 is null or claim_id > ?1)
                 order by claim_id limit ?2",
            )
            .map_err(Self::database_error)?;
        let start_after_bytes_opt = start_after_claim_id_opt.map(|claim_id| claim_id.to_vec());
        let sql_limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = statement
            .query_map(rusqlite::params![start_after_bytes_opt, sql_limit], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Vec<u8>>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })
            .map_err(Self::database_error)?;
        rows.map(|row| {
            let (claim_id, nonce, route, provider, payer_session, charge, serialized, accepted_at) =
                row.map_err(Self::database_error)?;
            let receipt_payload = Self::deserialize_settlement_claim(
                &claim_id,
                &nonce,
                &route,
                &provider,
                &payer_session,
                &charge,
                &serialized,
            )?;
            let accepted_at_unix_s = accepted_at
                .parse::<u64>()
                .map_err(|_| ReceiptCheckpointDaoError::StoredAmountInvalid(accepted_at.clone()))?;
            Ok(PendingSettlementClaimRecord {
                receipt_payload,
                accepted_at_unix_s,
            })
        })
        .collect()
    }

    fn settlement_reconciliation_candidates_page(
        &self,
        start_after_claim_id_opt: Option<[u8; 32]>,
        limit: usize,
    ) -> Result<Vec<SettlementReconciliationCandidate>, ReceiptCheckpointDaoError> {
        if limit == 0 {
            return Ok(vec![]);
        }
        let mut statement = self
            .conn
            .prepare(
                "select claim_id, cumulative_charge_wei from (
                    select claim_id, cumulative_charge_wei
                    from receipt_settlement_claim_outbox
                    union all
                    select archive.claim_id, archive.cumulative_charge_wei
                    from receipt_settlement_claim_archive archive
                    where not exists (
                        select 1 from receipt_settlement_claim_outbox pending
                        where pending.claim_id = archive.claim_id
                    )
                 ) candidates
                 where (?1 is null or claim_id > ?1)
                 order by claim_id limit ?2",
            )
            .map_err(Self::database_error)?;
        let start_after_bytes_opt = start_after_claim_id_opt.map(|claim_id| claim_id.to_vec());
        let sql_limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = statement
            .query_map(rusqlite::params![start_after_bytes_opt, sql_limit], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(Self::database_error)?;
        rows.map(|row| {
            let (claim_id, cumulative_charge_wei) = row.map_err(Self::database_error)?;
            Ok(SettlementReconciliationCandidate {
                claim_id: Self::fixed_32(claim_id, "settlement claim ID")?,
                cumulative_charge_wei: Self::stored_u128(&cumulative_charge_wei)?,
            })
        })
        .collect()
    }

    fn reconcile_settlement_claims(
        &mut self,
        observation: &SettlementChainObservation,
    ) -> Result<SettlementReconciliationOutcome, ReceiptCheckpointDaoError> {
        if observation.confirmation_depth == 0
            || observation.observed_block_number > observation.latest_block_number
            || observation
                .latest_block_number
                .checked_sub(observation.observed_block_number)
                != Some(observation.confirmation_depth)
            || observation
                .observed_block_hash
                .iter()
                .all(|byte| *byte == 0)
        {
            return Err(ReceiptCheckpointDaoError::SettlementObservationInvalid);
        }
        let mut unique_claim_ids = HashSet::new();
        if observation
            .claims
            .iter()
            .any(|claim| !unique_claim_ids.insert(claim.claim_id))
        {
            return Err(ReceiptCheckpointDaoError::SettlementObservationInvalid);
        }

        let transaction = self.conn.transaction().map_err(Self::database_error)?;
        let checkpoint_opt: Option<(String, Vec<u8>, String, String, Vec<u8>)> = transaction
            .prepare(
                "select chain_id, settlement_contract, confirmation_depth,
                        observed_block_number, observed_block_hash
                 from receipt_settlement_reconciliation_checkpoint where singleton_id = 1",
            )
            .map_err(Self::database_error)?
            .query_row([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .optional()
            .map_err(Self::database_error)?;
        if let Some((chain_id, contract, depth, block_number, block_hash)) = checkpoint_opt {
            let stored_chain_id = Self::stored_u64(&chain_id)?;
            let stored_depth = Self::stored_u64(&depth)?;
            let stored_block_number = Self::stored_u64(&block_number)?;
            let _stored_block_hash = Self::fixed_32(block_hash, "settlement block hash")?;
            if contract.as_slice() != observation.settlement_contract.as_bytes()
                || stored_chain_id != observation.chain_id
            {
                return Err(ReceiptCheckpointDaoError::SettlementObservationIdentityMismatch);
            }
            if observation.confirmation_depth < stored_depth {
                return Err(ReceiptCheckpointDaoError::SettlementConfirmationDepthReduced);
            }
            if observation.observed_block_number < stored_block_number {
                return Err(ReceiptCheckpointDaoError::SettlementObservationRegressed);
            }
        }

        let mut outcome = SettlementReconciliationOutcome {
            archived_claim_count: 0,
            restored_claim_count: 0,
            still_pending_claim_count: 0,
            revalidated_archive_count: 0,
            unknown_claim_count: 0,
        };
        for confirmation in &observation.claims {
            let pending_opt: Option<(Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, String, Vec<u8>, String)> =
                transaction
                    .prepare(
                        "select authorization_nonce, route_epoch, provider_public_key,
                            payer_session_public_key, cumulative_charge_wei,
                            receipt_payload_cbor, accepted_at_unix_s
                     from receipt_settlement_claim_outbox where claim_id = ?1",
                    )
                    .map_err(Self::database_error)?
                    .query_row(rusqlite::params![&confirmation.claim_id[..]], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                        ))
                    })
                    .optional()
                    .map_err(Self::database_error)?;
            if let Some((nonce, route, provider, payer_session, charge, serialized, accepted_at)) =
                pending_opt
            {
                let payload = Self::deserialize_settlement_claim(
                    &confirmation.claim_id,
                    &nonce,
                    &route,
                    &provider,
                    &payer_session,
                    &charge,
                    &serialized,
                )?;
                Self::stored_u64(&accepted_at)?;
                if payload.authorization.policy.chain_id != observation.chain_id
                    || payload.authorization.policy.settlement_contract
                        != observation.settlement_contract
                {
                    return Err(ReceiptCheckpointDaoError::SettlementObservationIdentityMismatch);
                }
                let local_charge = payload
                    .acknowledged_receipt
                    .signed_receipt
                    .receipt
                    .cumulative_charge_wei;
                if confirmation.cumulative_charge_wei < local_charge {
                    outcome.still_pending_claim_count += 1;
                    continue;
                }
                transaction
                    .execute(
                        "insert into receipt_settlement_claim_archive
                         (claim_id, authorization_nonce, route_epoch, provider_public_key,
                          payer_session_public_key, cumulative_charge_wei, receipt_payload_cbor,
                          accepted_at_unix_s, confirmed_cumulative_charge_wei,
                          observed_block_number, observed_block_hash)
                         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                         on conflict(claim_id) do update set
                          authorization_nonce = excluded.authorization_nonce,
                          route_epoch = excluded.route_epoch,
                          provider_public_key = excluded.provider_public_key,
                          payer_session_public_key = excluded.payer_session_public_key,
                          cumulative_charge_wei = excluded.cumulative_charge_wei,
                          receipt_payload_cbor = excluded.receipt_payload_cbor,
                          accepted_at_unix_s = excluded.accepted_at_unix_s,
                          confirmed_cumulative_charge_wei = excluded.confirmed_cumulative_charge_wei,
                          observed_block_number = excluded.observed_block_number,
                          observed_block_hash = excluded.observed_block_hash",
                        rusqlite::params![
                            &confirmation.claim_id[..],
                            &nonce,
                            &route,
                            &provider,
                            &payer_session,
                            &charge,
                            &serialized,
                            &accepted_at,
                            confirmation.cumulative_charge_wei.to_string(),
                            observation.observed_block_number.to_string(),
                            &observation.observed_block_hash[..]
                        ],
                    )
                    .map_err(Self::database_error)?;
                let removed = transaction
                    .execute(
                        "delete from receipt_settlement_claim_outbox
                         where claim_id = ?1 and receipt_payload_cbor = ?2",
                        rusqlite::params![&confirmation.claim_id[..], &serialized],
                    )
                    .map_err(Self::database_error)?;
                outcome.archived_claim_count += removed;
                continue;
            }

            let archived_opt: Option<(
                Vec<u8>,
                Vec<u8>,
                Vec<u8>,
                Vec<u8>,
                String,
                Vec<u8>,
                String,
                String,
                String,
                Vec<u8>,
            )> = transaction
                .prepare(
                    "select authorization_nonce, route_epoch, provider_public_key,
                            payer_session_public_key, cumulative_charge_wei,
                            receipt_payload_cbor, accepted_at_unix_s,
                            confirmed_cumulative_charge_wei, observed_block_number,
                            observed_block_hash
                     from receipt_settlement_claim_archive where claim_id = ?1",
                )
                .map_err(Self::database_error)?
                .query_row(rusqlite::params![&confirmation.claim_id[..]], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                    ))
                })
                .optional()
                .map_err(Self::database_error)?;
            let (
                nonce,
                route,
                provider,
                payer_session,
                charge,
                serialized,
                accepted_at,
                archived_confirmation,
                archived_block_number,
                archived_block_hash,
            ) = match archived_opt {
                Some(stored) => stored,
                None => {
                    outcome.unknown_claim_count += 1;
                    continue;
                }
            };
            let payload = Self::deserialize_settlement_claim(
                &confirmation.claim_id,
                &nonce,
                &route,
                &provider,
                &payer_session,
                &charge,
                &serialized,
            )?;
            Self::stored_u64(&accepted_at)?;
            Self::stored_u128(&archived_confirmation)?;
            Self::stored_u64(&archived_block_number)?;
            Self::fixed_32(archived_block_hash, "archived settlement block hash")?;
            if payload.authorization.policy.chain_id != observation.chain_id
                || payload.authorization.policy.settlement_contract
                    != observation.settlement_contract
            {
                return Err(ReceiptCheckpointDaoError::SettlementObservationIdentityMismatch);
            }
            let local_charge = payload
                .acknowledged_receipt
                .signed_receipt
                .receipt
                .cumulative_charge_wei;
            if confirmation.cumulative_charge_wei < local_charge {
                transaction
                    .execute(
                        "insert into receipt_settlement_claim_outbox
                         (claim_id, authorization_nonce, route_epoch, provider_public_key,
                          payer_session_public_key, cumulative_charge_wei,
                          receipt_payload_cbor, accepted_at_unix_s)
                         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        rusqlite::params![
                            &confirmation.claim_id[..],
                            &nonce,
                            &route,
                            &provider,
                            &payer_session,
                            &charge,
                            &serialized,
                            &accepted_at
                        ],
                    )
                    .map_err(Self::database_error)?;
                outcome.restored_claim_count += transaction
                    .execute(
                        "delete from receipt_settlement_claim_archive where claim_id = ?1",
                        rusqlite::params![&confirmation.claim_id[..]],
                    )
                    .map_err(Self::database_error)?;
            } else {
                outcome.revalidated_archive_count += transaction
                    .execute(
                        "update receipt_settlement_claim_archive
                         set confirmed_cumulative_charge_wei = ?1,
                             observed_block_number = ?2, observed_block_hash = ?3
                         where claim_id = ?4",
                        rusqlite::params![
                            confirmation.cumulative_charge_wei.to_string(),
                            observation.observed_block_number.to_string(),
                            &observation.observed_block_hash[..],
                            &confirmation.claim_id[..]
                        ],
                    )
                    .map_err(Self::database_error)?;
            }
        }

        transaction
            .execute(
                "insert into receipt_settlement_reconciliation_checkpoint
                 (singleton_id, chain_id, settlement_contract, confirmation_depth,
                  observed_block_number, observed_block_hash)
                 values (1, ?1, ?2, ?3, ?4, ?5)
                 on conflict(singleton_id) do update set
                  chain_id = excluded.chain_id,
                  settlement_contract = excluded.settlement_contract,
                  confirmation_depth = excluded.confirmation_depth,
                  observed_block_number = excluded.observed_block_number,
                  observed_block_hash = excluded.observed_block_hash",
                rusqlite::params![
                    observation.chain_id.to_string(),
                    observation.settlement_contract.as_bytes(),
                    observation.confirmation_depth.to_string(),
                    observation.observed_block_number.to_string(),
                    &observation.observed_block_hash[..]
                ],
            )
            .map_err(Self::database_error)?;
        transaction.commit().map_err(Self::database_error)?;
        Ok(outcome)
    }

    fn provider_settlement_authorization(
        &self,
    ) -> Result<Option<AuthorizedProviderSettlement>, ReceiptCheckpointDaoError> {
        let stored_opt: Option<(String, Vec<u8>)> = self
            .conn
            .prepare(
                "select expires_at_unix_s, authorization_cbor
                 from provider_settlement_authorization where singleton_id = 1",
            )
            .map_err(Self::database_error)?
            .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .map_err(Self::database_error)?;
        stored_opt
            .map(|(stored_expiry, serialized)| {
                let authorization: AuthorizedProviderSettlement =
                    serde_cbor::from_slice(&serialized).map_err(|error| {
                        ReceiptCheckpointDaoError::Deserialization(error.to_string())
                    })?;
                let expiry = stored_expiry.parse::<u64>().map_err(|_| {
                    ReceiptCheckpointDaoError::StoredAmountInvalid(stored_expiry.clone())
                })?;
                if authorization.policy.expires_at_unix_s != expiry {
                    return Err(ReceiptCheckpointDaoError::SettlementClaimIdentityMismatch);
                }
                Ok(authorization)
            })
            .transpose()
    }

    fn save_provider_settlement_authorization(
        &mut self,
        authorization: &AuthorizedProviderSettlement,
    ) -> Result<(), ReceiptCheckpointDaoError> {
        let serialized = serde_cbor::to_vec(authorization)
            .map_err(|error| ReceiptCheckpointDaoError::Serialization(error.to_string()))?;
        self.conn
            .prepare(
                "insert into provider_settlement_authorization
                 (singleton_id, expires_at_unix_s, authorization_cbor)
                 values (1, ?1, ?2)
                 on conflict(singleton_id) do update set
                  expires_at_unix_s = excluded.expires_at_unix_s,
                  authorization_cbor = excluded.authorization_cbor",
            )
            .map_err(Self::database_error)?
            .execute(rusqlite::params![
                authorization.policy.expires_at_unix_s.to_string(),
                serialized
            ])
            .map(|_| ())
            .map_err(Self::database_error)
    }

    fn clear_provider_settlement_authorization(&mut self) -> Result<(), ReceiptCheckpointDaoError> {
        self.conn
            .prepare("delete from provider_settlement_authorization where singleton_id = 1")
            .map_err(Self::database_error)?
            .execute([])
            .map(|_| ())
            .map_err(Self::database_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::db_initializer::{
        DbInitializationConfig, DbInitializer, DbInitializerReal,
    };
    use crate::database::rusqlite_wrappers::ConnectionWrapperReal;
    use crate::sub_lib::cryptde::{CryptData, PlainData};
    use crate::sub_lib::cryptde_null::CryptDENull;
    use crate::sub_lib::service_receipt::{
        AcknowledgedServiceReceipt, ReceiptSessionPolicy, ServiceKind, ServiceReceipt,
        SignedServiceReceipt, SERVICE_RECEIPT_PROTOCOL_VERSION,
    };
    use crate::test_utils::make_paying_wallet;
    use masq_lib::test_utils::utils::{
        ensure_node_home_directory_does_not_exist, TEST_DEFAULT_CHAIN,
    };
    use rusqlite::Connection;
    use std::fs::create_dir_all;
    use std::path::Path;

    fn make_checkpoint(sequence: u64, cumulative_charge_wei: u128) -> ReceiptSequenceCheckpoint {
        ReceiptSequenceCheckpoint {
            route_epoch: [0x11; 32],
            provider_public_key: PublicKey::new(b"provider"),
            accounting_commitment: [0x22; 32],
            payer_session_public_key: PublicKey::new(b"payer session"),
            last_sequence: sequence,
            cumulative_charge_wei,
        }
    }

    fn make_authorization_with_cap(max_total_charge_wei: u128) -> AuthorizedReceiptSession {
        let wallet = make_paying_wallet(b"receipt authorization wallet");
        ReceiptSessionPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
            wallet.address(),
            PublicKey::new(b"payer session"),
            max_total_charge_wei,
            100,
            200,
            [0x33; 32],
        )
        .authorize(&wallet)
        .unwrap()
    }

    fn make_authorization() -> AuthorizedReceiptSession {
        make_authorization_with_cap(1_000_000)
    }

    fn make_atomic_checkpoint(
        authorization: &AuthorizedReceiptSession,
        route_epoch: [u8; 32],
        sequence: u64,
        cumulative_charge_wei: u128,
    ) -> ReceiptSequenceCheckpoint {
        ReceiptSequenceCheckpoint {
            route_epoch,
            provider_public_key: PublicKey::new(b"atomic receipt provider"),
            accounting_commitment: make_accounting_commitment(
                &route_epoch,
                &authorization.policy.payer_session_public_key,
            ),
            payer_session_public_key: authorization.policy.payer_session_public_key.clone(),
            last_sequence: sequence,
            cumulative_charge_wei,
        }
    }

    fn make_atomic_payload(
        authorization: &AuthorizedReceiptSession,
        checkpoint: &ReceiptSequenceCheckpoint,
    ) -> ServiceReceiptPayload_0v1 {
        ServiceReceiptPayload_0v1 {
            authorization: authorization.clone(),
            acknowledged_receipt: AcknowledgedServiceReceipt {
                signed_receipt: SignedServiceReceipt {
                    receipt: ServiceReceipt {
                        protocol_version: SERVICE_RECEIPT_PROTOCOL_VERSION,
                        route_epoch: checkpoint.route_epoch,
                        sequence: checkpoint.last_sequence,
                        service_kind: ServiceKind::Routing,
                        provider_public_key: checkpoint.provider_public_key.clone(),
                        accounting_commitment: checkpoint.accounting_commitment,
                        payload_size: 0,
                        service_units: 1,
                        service_rate: 0,
                        byte_rate: 0,
                        cumulative_charge_wei: checkpoint.cumulative_charge_wei,
                    },
                    provider_signature: CryptData::new(PlainData::new(b"provider").as_slice()),
                },
                payer_session_public_key: checkpoint.payer_session_public_key.clone(),
                payer_signature: CryptData::new(PlainData::new(b"payer").as_slice()),
            },
        }
    }

    fn receivable_balance(subject: &ReceiptCheckpointDaoReal, wallet: &Wallet) -> i128 {
        let mut statement = subject
            .conn
            .prepare(
                "select balance_high_b, balance_low_b from receivable where wallet_address = ?1",
            )
            .unwrap();
        let parts = statement
            .query_row(rusqlite::params![wallet], |row| {
                Ok((row.get::<usize, i64>(0)?, row.get::<usize, i64>(1)?))
            })
            .unwrap();
        BigIntDivider::reconstitute(parts.0, parts.1)
    }

    fn offer_state_dao(path: &Path, create_table: bool) -> ReceiptCheckpointDaoReal {
        let connection = Connection::open(path).unwrap();
        if create_table {
            DbInitializerReal::create_routing_receipt_offer_state_table(&connection);
        }
        ReceiptCheckpointDaoReal::new(Box::new(ConnectionWrapperReal::new(connection)))
    }

    fn atomic_receipt_dao() -> ReceiptCheckpointDaoReal {
        let connection = Connection::open_in_memory().unwrap();
        DbInitializerReal::create_receivable_table(&connection);
        DbInitializerReal::create_receipt_sequence_checkpoint_table(&connection);
        DbInitializerReal::create_receipt_session_authorization_table(&connection);
        DbInitializerReal::create_receipt_settlement_claim_outbox_table(&connection);
        DbInitializerReal::create_receipt_settlement_reconciliation_tables(&connection);
        ReceiptCheckpointDaoReal::new(Box::new(ConnectionWrapperReal::new(connection)))
    }

    fn make_routing_offer_state(tag: u8, expires_at_unix_s: u64) -> RoutingReceiptOfferState {
        let provider_public_key = PublicKey::new(&[tag; 8]);
        let provider = CryptDENull::from(&provider_public_key, TEST_DEFAULT_CHAIN);
        let payer_session_public_key = PublicKey::new(&[tag.wrapping_add(1); 8]);
        let receipt = ServiceReceipt::new(
            [tag; 32],
            1,
            ServiceKind::Routing,
            provider_public_key,
            [tag.wrapping_add(2); 32],
            10,
            5,
            2,
        )
        .sign(&provider)
        .unwrap();
        RoutingReceiptOfferState {
            authorization_nonce: [tag.wrapping_add(3); 32],
            payer_session_public_key,
            expires_at_unix_s,
            last_signed_receipt: receipt,
        }
    }

    #[test]
    fn checkpoint_is_persisted_and_can_only_move_forward() {
        let home_dir = ensure_node_home_directory_does_not_exist(
            "receipt_checkpoint_dao",
            "checkpoint_is_persisted_and_can_only_move_forward",
        );
        let initializer = DbInitializerReal::default();
        let conn = initializer
            .initialize(&home_dir, DbInitializationConfig::test_default())
            .unwrap();
        let mut subject = ReceiptCheckpointDaoReal::new(conn);
        let initial = make_checkpoint(7, 100);
        let offer_state = make_routing_offer_state(0x61, 1_234_567);
        let confirmed_claim = ConfirmedSettlementClaim {
            claim_id: [0x62; 32],
            cumulative_charge_wei: 987_654,
        };
        let candidate = SettlementReconciliationCandidate {
            claim_id: [0x63; 32],
            cumulative_charge_wei: 876_543,
        };
        let observation = SettlementChainObservation {
            chain_id: 8453,
            settlement_contract: Address::from_slice(&[0x64; 20]),
            confirmation_depth: 64,
            latest_block_number: 999_999,
            observed_block_number: 999_935,
            observed_block_hash: [0x65; 32],
            claims: vec![confirmed_claim.clone()],
        };
        let authorization = make_authorization();
        let pending_checkpoint = make_atomic_checkpoint(&authorization, [0x66; 32], 9, 765_432);
        let pending_record = PendingSettlementClaimRecord {
            receipt_payload: make_atomic_payload(&authorization, &pending_checkpoint),
            accepted_at_unix_s: 1_987_654_321,
        };

        assert_eq!(
            format!("{:?}", offer_state),
            "RoutingReceiptOfferState { offer_state: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", confirmed_claim),
            "ConfirmedSettlementClaim { claim_data: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", candidate),
            "SettlementReconciliationCandidate { claim_data: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", observation),
            "SettlementChainObservation { confirmation_depth: 64, claim_count: 1, chain_evidence: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", pending_record),
            "PendingSettlementClaimRecord { claim_record: [REDACTED] }"
        );
        assert_eq!(
            format!(
                "{:?}",
                ReceiptCheckpointDaoError::Database("private database marker".to_string())
            ),
            "Database([REDACTED])"
        );
        subject.save_checkpoint(&initial).unwrap();

        let stale = make_checkpoint(7, 101);
        assert_eq!(
            subject.save_checkpoint(&stale),
            Err(ReceiptCheckpointDaoError::StaleCheckpoint)
        );

        let mut conflicting = make_checkpoint(8, 150);
        conflicting.accounting_commitment = [0x44; 32];
        assert_eq!(
            subject.save_checkpoint(&conflicting),
            Err(ReceiptCheckpointDaoError::CheckpointIdentityMismatch)
        );

        let advanced = make_checkpoint(8, 150);
        subject.save_checkpoint(&advanced).unwrap();
        drop(subject);

        let conn = initializer
            .initialize(&home_dir, DbInitializationConfig::test_default())
            .unwrap();
        let subject = ReceiptCheckpointDaoReal::new(conn);
        assert_eq!(
            subject
                .checkpoint(
                    &advanced.route_epoch,
                    &advanced.provider_public_key,
                    &advanced.payer_session_public_key,
                )
                .unwrap(),
            Some(advanced)
        );
    }

    #[test]
    fn authorization_nonce_is_write_once_and_persistent() {
        let home_dir = ensure_node_home_directory_does_not_exist(
            "receipt_checkpoint_dao",
            "authorization_nonce_is_write_once_and_persistent",
        );
        let initializer = DbInitializerReal::default();
        let conn = initializer
            .initialize(&home_dir, DbInitializationConfig::test_default())
            .unwrap();
        let mut subject = ReceiptCheckpointDaoReal::new(conn);
        let authorization = make_authorization();
        subject.save_authorization(&authorization).unwrap();
        assert_eq!(
            subject.save_authorization(&authorization),
            Err(ReceiptCheckpointDaoError::AuthorizationNonceAlreadyUsed)
        );
        drop(subject);

        let conn = initializer
            .initialize(&home_dir, DbInitializationConfig::test_default())
            .unwrap();
        let subject = ReceiptCheckpointDaoReal::new(conn);
        assert_eq!(
            subject
                .authorization(&authorization.policy.authorization_nonce)
                .unwrap(),
            Some(authorization)
        );
    }

    #[test]
    fn malformed_persisted_checkpoint_is_a_typed_error() {
        let home_dir = ensure_node_home_directory_does_not_exist(
            "receipt_checkpoint_dao",
            "malformed_persisted_checkpoint_is_a_typed_error",
        );
        let initializer = DbInitializerReal::default();
        let conn = initializer
            .initialize(&home_dir, DbInitializationConfig::test_default())
            .unwrap();
        let checkpoint = make_checkpoint(7, 100);
        conn.prepare(
            "insert into receipt_sequence_checkpoint
             (route_epoch, provider_public_key, payer_session_public_key,
              last_sequence, cumulative_charge_wei, checkpoint_cbor)
             values (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .unwrap()
        .execute(rusqlite::params![
            &checkpoint.route_epoch[..],
            checkpoint.provider_public_key.as_slice(),
            checkpoint.payer_session_public_key.as_slice(),
            checkpoint.last_sequence.to_string(),
            checkpoint.cumulative_charge_wei.to_string(),
            b"not cbor"
        ])
        .unwrap();
        let subject = ReceiptCheckpointDaoReal::new(conn);

        assert!(matches!(
            subject.checkpoint(
                &checkpoint.route_epoch,
                &checkpoint.provider_public_key,
                &checkpoint.payer_session_public_key,
            ),
            Err(ReceiptCheckpointDaoError::Deserialization(_))
        ));
    }

    #[test]
    fn verified_receipt_checkpoint_and_receivable_are_committed_exactly_once() {
        let mut subject = atomic_receipt_dao();
        let authorization = make_authorization_with_cap(200);
        let wallet = Wallet::from(authorization.policy.payer_wallet_address);
        let first = make_atomic_checkpoint(&authorization, [0x81; 32], 1, 100);
        let first_payload = make_atomic_payload(&authorization, &first);

        assert_eq!(
            subject.accept_verified_receipt(&first_payload, &first, SystemTime::UNIX_EPOCH),
            Ok(100)
        );
        assert_eq!(receivable_balance(&subject, &wallet), 100);
        assert_eq!(
            subject.pending_settlement_claims().unwrap(),
            vec![first_payload]
        );

        let advanced = make_atomic_checkpoint(&authorization, [0x81; 32], 2, 150);
        let advanced_payload = make_atomic_payload(&authorization, &advanced);
        assert_eq!(
            subject.accept_verified_receipt(&advanced_payload, &advanced, SystemTime::UNIX_EPOCH),
            Ok(50)
        );
        assert_eq!(receivable_balance(&subject, &wallet), 150);
        assert_eq!(
            subject.accept_verified_receipt(&advanced_payload, &advanced, SystemTime::UNIX_EPOCH),
            Err(ReceiptCheckpointDaoError::StaleCheckpoint)
        );
        assert_eq!(receivable_balance(&subject, &wallet), 150);
        assert_eq!(
            subject.pending_settlement_claims().unwrap(),
            vec![advanced_payload]
        );

        let parallel_route = make_atomic_checkpoint(&authorization, [0x82; 32], 1, 60);
        let parallel_payload = make_atomic_payload(&authorization, &parallel_route);
        assert_eq!(
            subject.accept_verified_receipt(
                &parallel_payload,
                &parallel_route,
                SystemTime::UNIX_EPOCH
            ),
            Err(ReceiptCheckpointDaoError::AmountLimitExceeded)
        );
        assert_eq!(receivable_balance(&subject, &wallet), 150);
        assert_eq!(
            subject
                .checkpoint(
                    &parallel_route.route_epoch,
                    &parallel_route.provider_public_key,
                    &parallel_route.payer_session_public_key,
                )
                .unwrap(),
            None
        );
    }

    #[test]
    fn late_balance_overflow_rolls_back_authorization_checkpoint_and_receivable() {
        let mut subject = atomic_receipt_dao();
        let authorization = make_authorization_with_cap(1);
        let wallet = Wallet::from(authorization.policy.payer_wallet_address);
        let maximum_balance = (1i128 << 126) - 1;
        let (balance_high_b, balance_low_b) = BigIntDivider::deconstruct(maximum_balance);
        subject
            .conn
            .prepare(
                "insert into receivable
                 (wallet_address, balance_high_b, balance_low_b, last_received_timestamp)
                 values (?1, ?2, ?3, 0)",
            )
            .unwrap()
            .execute(rusqlite::params![&wallet, balance_high_b, balance_low_b])
            .unwrap();
        let checkpoint = make_atomic_checkpoint(&authorization, [0x83; 32], 1, 1);
        let payload = make_atomic_payload(&authorization, &checkpoint);

        assert_eq!(
            subject.accept_verified_receipt(&payload, &checkpoint, SystemTime::UNIX_EPOCH),
            Err(ReceiptCheckpointDaoError::BalanceOverflow)
        );
        assert_eq!(
            subject
                .authorization(&authorization.policy.authorization_nonce)
                .unwrap(),
            None
        );
        assert_eq!(
            subject
                .checkpoint(
                    &checkpoint.route_epoch,
                    &checkpoint.provider_public_key,
                    &checkpoint.payer_session_public_key,
                )
                .unwrap(),
            None
        );
        assert_eq!(receivable_balance(&subject, &wallet), maximum_balance);
        assert_eq!(subject.pending_settlement_claims().unwrap(), vec![]);
    }

    #[test]
    fn settlement_claim_binds_a_checkpoint_to_one_authorization() {
        let mut subject = atomic_receipt_dao();
        let first_authorization = make_authorization_with_cap(1_000);
        let first = make_atomic_checkpoint(&first_authorization, [0x84; 32], 1, 100);
        let first_payload = make_atomic_payload(&first_authorization, &first);
        subject
            .accept_verified_receipt(&first_payload, &first, SystemTime::UNIX_EPOCH)
            .unwrap();

        let mut replacement_authorization = first_authorization.clone();
        replacement_authorization.policy.authorization_nonce = [0x55; 32];
        let advanced = make_atomic_checkpoint(&replacement_authorization, [0x84; 32], 2, 150);
        let replacement_payload = make_atomic_payload(&replacement_authorization, &advanced);
        assert_eq!(
            subject.accept_verified_receipt(
                &replacement_payload,
                &advanced,
                SystemTime::UNIX_EPOCH
            ),
            Err(ReceiptCheckpointDaoError::SettlementClaimIdentityMismatch)
        );
        assert_eq!(
            subject.pending_settlement_claims().unwrap(),
            vec![first_payload]
        );
        assert_eq!(
            subject
                .checkpoint(
                    &first.route_epoch,
                    &first.provider_public_key,
                    &first.payer_session_public_key,
                )
                .unwrap(),
            Some(first)
        );
    }

    #[test]
    fn settlement_claim_cursor_is_stable_and_reconciliation_recovers_from_a_reorg() {
        let mut subject = atomic_receipt_dao();
        let authorization = make_authorization_with_cap(1_000);
        let first = make_atomic_checkpoint(&authorization, [0x85; 32], 1, 100);
        let first_payload = make_atomic_payload(&authorization, &first);
        let second = make_atomic_checkpoint(&authorization, [0x86; 32], 1, 60);
        let second_payload = make_atomic_payload(&authorization, &second);
        subject
            .accept_verified_receipt(&first_payload, &first, SystemTime::UNIX_EPOCH)
            .unwrap();
        subject
            .accept_verified_receipt(&second_payload, &second, SystemTime::UNIX_EPOCH)
            .unwrap();

        let mut ordered = vec![first_payload.clone(), second_payload.clone()];
        ordered.sort_by_key(|payload| receipt_settlement_claim_id(payload).unwrap());
        let first_cursor = receipt_settlement_claim_id(&ordered[0]).unwrap();
        assert_eq!(
            subject.pending_settlement_claims_page(None, 1).unwrap(),
            vec![ordered[0].clone()]
        );
        assert_eq!(
            subject
                .pending_settlement_claims_page(Some(first_cursor), 1)
                .unwrap(),
            vec![ordered[1].clone()]
        );

        let first_claim_id = receipt_settlement_claim_id(&first_payload).unwrap();
        let second_claim_id = receipt_settlement_claim_id(&second_payload).unwrap();
        let advanced = make_atomic_checkpoint(&authorization, [0x85; 32], 2, 150);
        let advanced_payload = make_atomic_payload(&authorization, &advanced);
        subject
            .accept_verified_receipt(&advanced_payload, &advanced, SystemTime::UNIX_EPOCH)
            .unwrap();
        let candidates = subject
            .settlement_reconciliation_candidates_page(None, 8)
            .unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(
            candidates
                .iter()
                .find(|candidate| candidate.claim_id == first_claim_id)
                .unwrap()
                .cumulative_charge_wei,
            150
        );

        let first_observation = SettlementChainObservation {
            chain_id: authorization.policy.chain_id,
            settlement_contract: authorization.policy.settlement_contract,
            confirmation_depth: 12,
            latest_block_number: 112,
            observed_block_number: 100,
            observed_block_hash: [0xa1; 32],
            claims: vec![
                ConfirmedSettlementClaim {
                    claim_id: first_claim_id,
                    cumulative_charge_wei: 100,
                },
                ConfirmedSettlementClaim {
                    claim_id: second_claim_id,
                    cumulative_charge_wei: 60,
                },
            ],
        };
        assert_eq!(
            subject
                .reconcile_settlement_claims(&first_observation)
                .unwrap(),
            SettlementReconciliationOutcome {
                archived_claim_count: 1,
                restored_claim_count: 0,
                still_pending_claim_count: 1,
                revalidated_archive_count: 0,
                unknown_claim_count: 0,
            }
        );
        assert_eq!(
            subject.pending_settlement_claims().unwrap(),
            vec![advanced_payload.clone()]
        );

        let fully_confirmed = SettlementChainObservation {
            latest_block_number: 113,
            observed_block_number: 101,
            observed_block_hash: [0xa2; 32],
            claims: vec![
                ConfirmedSettlementClaim {
                    claim_id: first_claim_id,
                    cumulative_charge_wei: 150,
                },
                ConfirmedSettlementClaim {
                    claim_id: second_claim_id,
                    cumulative_charge_wei: 60,
                },
            ],
            ..first_observation.clone()
        };
        assert_eq!(
            subject
                .reconcile_settlement_claims(&fully_confirmed)
                .unwrap(),
            SettlementReconciliationOutcome {
                archived_claim_count: 1,
                restored_claim_count: 0,
                still_pending_claim_count: 0,
                revalidated_archive_count: 1,
                unknown_claim_count: 0,
            }
        );
        assert!(subject.pending_settlement_claims().unwrap().is_empty());

        let reorged = SettlementChainObservation {
            latest_block_number: 114,
            observed_block_number: 102,
            observed_block_hash: [0xb1; 32],
            claims: vec![
                ConfirmedSettlementClaim {
                    claim_id: first_claim_id,
                    cumulative_charge_wei: 149,
                },
                ConfirmedSettlementClaim {
                    claim_id: second_claim_id,
                    cumulative_charge_wei: 0,
                },
            ],
            ..fully_confirmed
        };
        assert_eq!(
            subject.reconcile_settlement_claims(&reorged).unwrap(),
            SettlementReconciliationOutcome {
                archived_claim_count: 0,
                restored_claim_count: 2,
                still_pending_claim_count: 0,
                revalidated_archive_count: 0,
                unknown_claim_count: 0,
            }
        );
        let mut restored = subject.pending_settlement_claims().unwrap();
        restored.sort_by_key(|payload| receipt_settlement_claim_id(payload).unwrap());
        let mut expected = vec![advanced_payload, second_payload];
        expected.sort_by_key(|payload| receipt_settlement_claim_id(payload).unwrap());
        assert_eq!(restored, expected);

        let downgraded = SettlementChainObservation {
            confirmation_depth: 11,
            latest_block_number: 114,
            observed_block_number: 103,
            observed_block_hash: [0xc1; 32],
            claims: vec![],
            ..reorged
        };
        assert_eq!(
            subject.reconcile_settlement_claims(&downgraded),
            Err(ReceiptCheckpointDaoError::SettlementConfirmationDepthReduced)
        );
    }

    #[test]
    fn routing_offer_state_survives_reopen_and_only_advances_monotonically() {
        let directory = ensure_node_home_directory_does_not_exist(
            "receipt_checkpoint_dao",
            "routing_offer_state_survives_reopen_and_only_advances_monotonically",
        );
        create_dir_all(&directory).unwrap();
        let database_path = directory.join("routing-offer-state.db");
        let mut subject = offer_state_dao(&database_path, true);
        let first = make_routing_offer_state(0x61, 200);
        subject.save_routing_offer_state(&first, 100, 8).unwrap();
        drop(subject);

        let mut subject = offer_state_dao(&database_path, false);
        let first_receipt = &first.last_signed_receipt.receipt;
        assert_eq!(
            subject
                .routing_offer_state(
                    &first.authorization_nonce,
                    &first_receipt.route_epoch,
                    &first_receipt.provider_public_key,
                    &first.payer_session_public_key,
                )
                .unwrap(),
            Some(first.clone())
        );
        subject.save_routing_offer_state(&first, 100, 8).unwrap();

        let provider = CryptDENull::from(
            &first.last_signed_receipt.receipt.provider_public_key,
            TEST_DEFAULT_CHAIN,
        );
        let mut advanced = first.clone();
        advanced.last_signed_receipt = first
            .last_signed_receipt
            .receipt
            .next_for_same_route(2, ServiceKind::Routing, 20, 5, 2)
            .unwrap()
            .sign(&provider)
            .unwrap();
        subject.save_routing_offer_state(&advanced, 100, 8).unwrap();
        assert_eq!(
            subject.save_routing_offer_state(&first, 100, 8),
            Err(ReceiptCheckpointDaoError::StaleOfferState)
        );
        assert_eq!(
            subject
                .routing_offer_state(
                    &advanced.authorization_nonce,
                    &advanced.last_signed_receipt.receipt.route_epoch,
                    &advanced.last_signed_receipt.receipt.provider_public_key,
                    &advanced.payer_session_public_key,
                )
                .unwrap(),
            Some(advanced)
        );
    }

    #[test]
    fn expired_routing_offer_state_is_pruned_before_capacity_is_enforced() {
        let directory = ensure_node_home_directory_does_not_exist(
            "receipt_checkpoint_dao",
            "expired_routing_offer_state_is_pruned_before_capacity_is_enforced",
        );
        create_dir_all(&directory).unwrap();
        let database_path = directory.join("routing-offer-capacity.db");
        let mut subject = offer_state_dao(&database_path, true);
        let expired = make_routing_offer_state(0x71, 100);
        let replacement = make_routing_offer_state(0x72, 200);
        subject.save_routing_offer_state(&expired, 100, 1).unwrap();
        assert_eq!(
            subject.save_routing_offer_state(&replacement, 100, 1),
            Err(ReceiptCheckpointDaoError::OfferStateCapacityExceeded)
        );
        subject
            .save_routing_offer_state(&replacement, 101, 1)
            .unwrap();
        assert!(subject
            .routing_offer_state(
                &expired.authorization_nonce,
                &expired.last_signed_receipt.receipt.route_epoch,
                &expired.last_signed_receipt.receipt.provider_public_key,
                &expired.payer_session_public_key,
            )
            .unwrap()
            .is_none());
    }
}
