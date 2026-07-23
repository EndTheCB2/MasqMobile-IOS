// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::accountant::db_access_objects::receipt_checkpoint_dao::{
    ReceiptCheckpointDao, ReceiptCheckpointDaoError, SettlementChainObservation,
    SettlementReconciliationCandidate, SettlementReconciliationOutcome,
};
use crate::sub_lib::cryptde::CryptDE;
use crate::sub_lib::receipt_settlement::{
    receipt_settlement_claim_id, ReceiptSettlementBatch, ReceiptSettlementClaim,
    ReceiptSettlementError,
};
use crate::sub_lib::service_receipt::{
    AuthorizedProviderSettlement, ProviderSettlementPolicy, ServiceReceiptError,
    MAX_PROVIDER_SETTLEMENT_DURATION_SECONDS, MIN_PROVIDER_SETTLEMENT_DURATION_SECONDS,
};
use ethereum_types::Address;
use ethsign::Signature;
use masq_lib::blockchains::chains::Chain;
use rustc_hex::{FromHex, ToHex};
use serde_json::Value;
use std::fmt::{Debug, Formatter};

pub const MAX_PROVIDER_SETTLEMENT_EXPORT_CLAIMS: usize = 128;
pub const MIN_PROVIDER_SETTLEMENT_CONFIRMATION_DEPTH: u64 = 12;
pub const MAX_PROVIDER_SETTLEMENT_CONFIRMATION_DEPTH: u64 = 100_000;
const SECP256K1_HALF_ORDER: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b, 0x20, 0xa0,
];

#[derive(Clone)]
pub struct ProviderSettlementConfig {
    pub chain: Chain,
    pub chain_id: u64,
    pub settlement_contract: Address,
    pub payout_wallet_address: Address,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProviderSettlementProposal {
    pub proposal_id: String,
    pub chain_name: String,
    pub masq_token_contract: Address,
    pub policy: ProviderSettlementPolicy,
    pub eip712_typed_data: Value,
}

impl Debug for ProviderSettlementProposal {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderSettlementProposal { proposal_data: [REDACTED] }")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProviderSettlementStatus {
    pub authorization_opt: Option<AuthorizedProviderSettlement>,
    pub chain_name_opt: Option<String>,
    pub masq_token_contract_opt: Option<Address>,
    pub pending_claim_count: usize,
}

impl Debug for ProviderSettlementStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ProviderSettlementStatus {{ active: {}, pending_claim_count: {}, settlement_data: [REDACTED] }}",
            self.authorization_opt.is_some(),
            self.pending_claim_count
        )
    }
}

impl ProviderSettlementStatus {
    pub fn inactive(pending_claim_count: usize) -> Self {
        Self {
            authorization_opt: None,
            chain_name_opt: None,
            masq_token_contract_opt: None,
            pending_claim_count,
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProviderSettlementBatchExport {
    pub batch: ReceiptSettlementBatch,
    pub total_pending_claims: usize,
    pub start_after_claim_id_opt: Option<[u8; 32]>,
    pub next_cursor: [u8; 32],
}

impl Debug for ProviderSettlementBatchExport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ProviderSettlementBatchExport {{ batch_claim_count: {}, total_pending_claims: {}, export_data: [REDACTED] }}",
            self.batch.claims.len(),
            self.total_pending_claims
        )
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProviderSettlementReconciliationPage {
    pub settlement_contract: Address,
    pub candidates: Vec<SettlementReconciliationCandidate>,
    pub start_after_claim_id_opt: Option<[u8; 32]>,
    pub next_cursor: [u8; 32],
}

impl Debug for ProviderSettlementReconciliationPage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ProviderSettlementReconciliationPage {{ candidate_count: {}, reconciliation_data: [REDACTED] }}",
            self.candidates.len()
        )
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum ProviderSettlementManagerError {
    Dao(ReceiptCheckpointDaoError),
    ConfirmationDepth(u64),
    Duration(u64),
    EmptyPage,
    Expired,
    ExportLimit(usize),
    NoActiveAuthorization,
    NoPendingProposal,
    ProposalMismatch,
    Settlement(ReceiptSettlementError),
    Signature(String),
    TimeOverflow,
    Verification(ServiceReceiptError),
}

impl Debug for ProviderSettlementManagerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dao(_) => f.write_str("Dao([REDACTED])"),
            Self::ConfirmationDepth(value) => {
                f.debug_tuple("ConfirmationDepth").field(value).finish()
            }
            Self::Duration(value) => f.debug_tuple("Duration").field(value).finish(),
            Self::EmptyPage => f.write_str("EmptyPage"),
            Self::Expired => f.write_str("Expired"),
            Self::ExportLimit(value) => f.debug_tuple("ExportLimit").field(value).finish(),
            Self::NoActiveAuthorization => f.write_str("NoActiveAuthorization"),
            Self::NoPendingProposal => f.write_str("NoPendingProposal"),
            Self::ProposalMismatch => f.write_str("ProposalMismatch"),
            Self::Settlement(_) => f.write_str("Settlement([REDACTED])"),
            Self::Signature(_) => f.write_str("Signature([REDACTED])"),
            Self::TimeOverflow => f.write_str("TimeOverflow"),
            Self::Verification(_) => f.write_str("Verification([REDACTED])"),
        }
    }
}

impl std::fmt::Display for ProviderSettlementManagerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dao(error) => write!(
                formatter,
                "provider settlement persistence failed: {:?}",
                error
            ),
            Self::ConfirmationDepth(value) => write!(
                formatter,
                "confirmationDepth {} is outside the allowed {}..={} block range",
                value,
                MIN_PROVIDER_SETTLEMENT_CONFIRMATION_DEPTH,
                MAX_PROVIDER_SETTLEMENT_CONFIRMATION_DEPTH
            ),
            Self::Duration(value) => write!(
                formatter,
                "durationSeconds {} is outside the allowed {}..={} second range",
                value,
                MIN_PROVIDER_SETTLEMENT_DURATION_SECONDS,
                MAX_PROVIDER_SETTLEMENT_DURATION_SECONDS
            ),
            Self::EmptyPage => write!(formatter, "the requested pending-claim page is empty"),
            Self::Expired => write!(formatter, "provider-settlement proposal has expired"),
            Self::ExportLimit(value) => write!(
                formatter,
                "maxClaims {} is outside the contract-compatible 1..={} range",
                value, MAX_PROVIDER_SETTLEMENT_EXPORT_CLAIMS
            ),
            Self::NoActiveAuthorization => {
                write!(formatter, "no provider payout authorization is active")
            }
            Self::NoPendingProposal => {
                write!(formatter, "no provider-settlement proposal is pending")
            }
            Self::ProposalMismatch => {
                write!(formatter, "provider-settlement proposal ID does not match")
            }
            Self::Settlement(error) => {
                write!(formatter, "cannot build settlement batch: {:?}", error)
            }
            Self::Signature(_) => write!(formatter, "invalid wallet signature"),
            Self::TimeOverflow => write!(
                formatter,
                "provider-settlement expiry exceeds the supported time range"
            ),
            Self::Verification(error) => write!(
                formatter,
                "provider payout authorization failed: {:?}",
                error
            ),
        }
    }
}

struct PendingProviderSettlement {
    proposal_id: String,
    policy: ProviderSettlementPolicy,
}

pub struct ProviderSettlementManager {
    config: ProviderSettlementConfig,
    provider_cryptde: Box<dyn CryptDE>,
    dao: Box<dyn ReceiptCheckpointDao>,
    pending_opt: Option<PendingProviderSettlement>,
    active_opt: Option<AuthorizedProviderSettlement>,
}

impl ProviderSettlementManager {
    pub fn new(
        config: ProviderSettlementConfig,
        provider_cryptde: Box<dyn CryptDE>,
        mut dao: Box<dyn ReceiptCheckpointDao>,
        now_unix_s: u64,
    ) -> Result<Self, ProviderSettlementManagerError> {
        let stored_opt = dao
            .provider_settlement_authorization()
            .map_err(ProviderSettlementManagerError::Dao)?;
        let active_opt = match stored_opt {
            Some(stored) if now_unix_s > stored.policy.expires_at_unix_s => {
                dao.clear_provider_settlement_authorization()
                    .map_err(ProviderSettlementManagerError::Dao)?;
                None
            }
            Some(stored)
                if stored.policy.chain_id != config.chain_id
                    || stored.policy.settlement_contract != config.settlement_contract
                    || stored.policy.payout_wallet_address != config.payout_wallet_address
                    || &stored.policy.provider_public_key != provider_cryptde.public_key() =>
            {
                dao.clear_provider_settlement_authorization()
                    .map_err(ProviderSettlementManagerError::Dao)?;
                None
            }
            Some(stored) => {
                stored
                    .verify(
                        config.chain_id,
                        config.settlement_contract,
                        provider_cryptde.public_key(),
                        now_unix_s,
                        provider_cryptde.as_ref(),
                    )
                    .map_err(ProviderSettlementManagerError::Verification)?;
                Some(stored)
            }
            None => None,
        };
        Ok(Self {
            config,
            provider_cryptde,
            dao,
            pending_opt: None,
            active_opt,
        })
    }

    pub fn propose(
        &mut self,
        duration_seconds: u64,
        now_unix_s: u64,
    ) -> Result<ProviderSettlementProposal, ProviderSettlementManagerError> {
        if !(MIN_PROVIDER_SETTLEMENT_DURATION_SECONDS..=MAX_PROVIDER_SETTLEMENT_DURATION_SECONDS)
            .contains(&duration_seconds)
        {
            return Err(ProviderSettlementManagerError::Duration(duration_seconds));
        }
        let expires_at_unix_s = now_unix_s
            .checked_add(duration_seconds)
            .ok_or(ProviderSettlementManagerError::TimeOverflow)?;
        let mut authorization_nonce = [0u8; 32];
        self.provider_cryptde.random(&mut authorization_nonce);
        if authorization_nonce.iter().all(|byte| *byte == 0) {
            return Err(ProviderSettlementManagerError::Signature(
                "secure nonce generation returned an invalid value".to_string(),
            ));
        }
        let policy = ProviderSettlementPolicy::new(
            self.config.chain_id,
            self.config.settlement_contract,
            self.config.payout_wallet_address,
            self.provider_cryptde.public_key().clone(),
            now_unix_s,
            expires_at_unix_s,
            authorization_nonce,
        );
        let eip712_typed_data = policy
            .eip712_typed_data()
            .map_err(ProviderSettlementManagerError::Verification)?;
        let proposal_id = format!("0x{}", authorization_nonce.to_hex::<String>());
        self.pending_opt = Some(PendingProviderSettlement {
            proposal_id: proposal_id.clone(),
            policy: policy.clone(),
        });
        Ok(ProviderSettlementProposal {
            proposal_id,
            chain_name: self.config.chain.rec().literal_identifier.to_string(),
            masq_token_contract: self.config.chain.rec().contract,
            policy,
            eip712_typed_data,
        })
    }

    pub fn activate(
        &mut self,
        proposal_id: &str,
        wallet_signature: &str,
        now_unix_s: u64,
    ) -> Result<ProviderSettlementStatus, ProviderSettlementManagerError> {
        let pending = self
            .pending_opt
            .as_ref()
            .ok_or(ProviderSettlementManagerError::NoPendingProposal)?;
        if pending.proposal_id != proposal_id {
            return Err(ProviderSettlementManagerError::ProposalMismatch);
        }
        if now_unix_s > pending.policy.expires_at_unix_s {
            self.pending_opt = None;
            return Err(ProviderSettlementManagerError::Expired);
        }
        let signature = Self::signature_from_hex(wallet_signature)?;
        let authorization = pending
            .policy
            .clone()
            .authorize_with_wallet_signature(signature, self.provider_cryptde.as_ref())
            .map_err(ProviderSettlementManagerError::Verification)?;
        authorization
            .verify(
                self.config.chain_id,
                self.config.settlement_contract,
                self.provider_cryptde.public_key(),
                now_unix_s,
                self.provider_cryptde.as_ref(),
            )
            .map_err(ProviderSettlementManagerError::Verification)?;
        self.dao
            .save_provider_settlement_authorization(&authorization)
            .map_err(ProviderSettlementManagerError::Dao)?;
        self.pending_opt = None;
        self.active_opt = Some(authorization);
        self.status(now_unix_s)
    }

    pub fn status(
        &mut self,
        now_unix_s: u64,
    ) -> Result<ProviderSettlementStatus, ProviderSettlementManagerError> {
        if self
            .active_opt
            .as_ref()
            .map(|authorization| now_unix_s > authorization.policy.expires_at_unix_s)
            .unwrap_or(false)
        {
            self.dao
                .clear_provider_settlement_authorization()
                .map_err(ProviderSettlementManagerError::Dao)?;
            self.active_opt = None;
        }
        let pending_claim_count = self
            .dao
            .pending_settlement_claims()
            .map_err(ProviderSettlementManagerError::Dao)?
            .len();
        match self.active_opt.as_ref() {
            Some(authorization) => Ok(ProviderSettlementStatus {
                authorization_opt: Some(authorization.clone()),
                chain_name_opt: Some(self.config.chain.rec().literal_identifier.to_string()),
                masq_token_contract_opt: Some(self.config.chain.rec().contract),
                pending_claim_count,
            }),
            None => Ok(ProviderSettlementStatus::inactive(pending_claim_count)),
        }
    }

    pub fn stop(&mut self) -> Result<ProviderSettlementStatus, ProviderSettlementManagerError> {
        self.dao
            .clear_provider_settlement_authorization()
            .map_err(ProviderSettlementManagerError::Dao)?;
        self.pending_opt = None;
        self.active_opt = None;
        let pending_claim_count = self
            .dao
            .pending_settlement_claims()
            .map_err(ProviderSettlementManagerError::Dao)?
            .len();
        Ok(ProviderSettlementStatus::inactive(pending_claim_count))
    }

    pub fn export(
        &mut self,
        start_after_claim_id_opt: Option<[u8; 32]>,
        max_claims: usize,
        now_unix_s: u64,
    ) -> Result<ProviderSettlementBatchExport, ProviderSettlementManagerError> {
        if !(1..=MAX_PROVIDER_SETTLEMENT_EXPORT_CLAIMS).contains(&max_claims) {
            return Err(ProviderSettlementManagerError::ExportLimit(max_claims));
        }
        self.status(now_unix_s)?;
        let authorization = self
            .active_opt
            .clone()
            .ok_or(ProviderSettlementManagerError::NoActiveAuthorization)?;
        let total_pending_claims = self
            .dao
            .pending_settlement_claims()
            .map_err(ProviderSettlementManagerError::Dao)?;
        let selected_records = self
            .dao
            .pending_settlement_claim_records_page(start_after_claim_id_opt, max_claims)
            .map_err(ProviderSettlementManagerError::Dao)?;
        let next_cursor = selected_records
            .last()
            .map(|record| receipt_settlement_claim_id(&record.receipt_payload))
            .transpose()
            .map_err(ProviderSettlementManagerError::Settlement)?
            .ok_or(ProviderSettlementManagerError::EmptyPage)?;
        let selected = selected_records
            .into_iter()
            .map(|record| {
                (
                    ReceiptSettlementClaim::new(record.receipt_payload, authorization.clone()),
                    record.accepted_at_unix_s,
                )
            })
            .collect::<Vec<_>>();
        let batch = ReceiptSettlementBatch::build_from_accepted(
            selected,
            self.config.chain_id,
            self.config.settlement_contract,
            now_unix_s,
            self.provider_cryptde.as_ref(),
        )
        .map_err(ProviderSettlementManagerError::Settlement)?;
        Ok(ProviderSettlementBatchExport {
            batch,
            total_pending_claims: total_pending_claims.len(),
            start_after_claim_id_opt,
            next_cursor,
        })
    }

    pub fn reconciliation_page(
        &self,
        start_after_claim_id_opt: Option<[u8; 32]>,
        max_claims: usize,
        confirmation_depth: u64,
    ) -> Result<ProviderSettlementReconciliationPage, ProviderSettlementManagerError> {
        if !(1..=MAX_PROVIDER_SETTLEMENT_EXPORT_CLAIMS).contains(&max_claims) {
            return Err(ProviderSettlementManagerError::ExportLimit(max_claims));
        }
        if !(MIN_PROVIDER_SETTLEMENT_CONFIRMATION_DEPTH
            ..=MAX_PROVIDER_SETTLEMENT_CONFIRMATION_DEPTH)
            .contains(&confirmation_depth)
        {
            return Err(ProviderSettlementManagerError::ConfirmationDepth(
                confirmation_depth,
            ));
        }
        let candidates = self
            .dao
            .settlement_reconciliation_candidates_page(start_after_claim_id_opt, max_claims)
            .map_err(ProviderSettlementManagerError::Dao)?;
        let next_cursor = candidates
            .last()
            .map(|candidate| candidate.claim_id)
            .ok_or(ProviderSettlementManagerError::EmptyPage)?;
        Ok(ProviderSettlementReconciliationPage {
            settlement_contract: self.config.settlement_contract,
            candidates,
            start_after_claim_id_opt,
            next_cursor,
        })
    }

    pub fn reconcile(
        &mut self,
        observation: &SettlementChainObservation,
    ) -> Result<SettlementReconciliationOutcome, ProviderSettlementManagerError> {
        if observation.chain_id != self.config.chain_id
            || observation.settlement_contract != self.config.settlement_contract
        {
            return Err(ProviderSettlementManagerError::Dao(
                ReceiptCheckpointDaoError::SettlementObservationIdentityMismatch,
            ));
        }
        self.dao
            .reconcile_settlement_claims(observation)
            .map_err(ProviderSettlementManagerError::Dao)
    }

    fn signature_from_hex(input: &str) -> Result<Signature, ProviderSettlementManagerError> {
        let unprefixed = input.strip_prefix("0x").unwrap_or(input);
        let bytes: Vec<u8> = unprefixed
            .from_hex()
            .map_err(|error| ProviderSettlementManagerError::Signature(format!("{:?}", error)))?;
        if bytes.len() != 65 {
            return Err(ProviderSettlementManagerError::Signature(format!(
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
            return Err(ProviderSettlementManagerError::Signature(
                "r and s must be non-zero and s must use canonical low-s form".to_string(),
            ));
        }
        let v = match bytes[64] {
            0 | 1 => bytes[64],
            27 | 28 => bytes[64] - 27,
            other => {
                return Err(ProviderSettlementManagerError::Signature(format!(
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
    use crate::accountant::db_access_objects::receipt_checkpoint_dao::{
        ConfirmedSettlementClaim, ReceiptCheckpointDaoReal,
    };
    use crate::database::db_initializer::DbInitializerReal;
    use crate::database::rusqlite_wrappers::ConnectionWrapperReal;
    use crate::sub_lib::cryptde_real::CryptDEReal;
    use crate::sub_lib::service_receipt::{
        make_accounting_commitment, ReceiptSequenceCheckpoint, ReceiptSessionPolicy, ServiceKind,
        ServiceReceipt, ServiceReceiptPayload_0v1,
    };
    use crate::sub_lib::wallet::Wallet;
    use crate::test_utils::make_paying_wallet;
    use masq_lib::test_utils::utils::{
        ensure_node_home_directory_does_not_exist, TEST_DEFAULT_CHAIN,
    };
    use rusqlite::Connection;
    use std::fs::create_dir_all;
    use std::path::Path;

    fn config(wallet: &Wallet) -> ProviderSettlementConfig {
        ProviderSettlementConfig {
            chain: TEST_DEFAULT_CHAIN,
            chain_id: TEST_DEFAULT_CHAIN.rec().num_chain_id,
            settlement_contract: TEST_DEFAULT_CHAIN.rec().contract,
            payout_wallet_address: wallet.address(),
        }
    }

    fn dao(path: &Path, create_tables: bool) -> Box<dyn ReceiptCheckpointDao> {
        let connection = Connection::open(path).unwrap();
        if create_tables {
            DbInitializerReal::create_receivable_table(&connection);
            DbInitializerReal::create_receipt_sequence_checkpoint_table(&connection);
            DbInitializerReal::create_receipt_session_authorization_table(&connection);
            DbInitializerReal::create_receipt_settlement_claim_outbox_table(&connection);
            DbInitializerReal::create_provider_settlement_authorization_table(&connection);
            DbInitializerReal::create_receipt_settlement_reconciliation_tables(&connection);
        }
        Box::new(ReceiptCheckpointDaoReal::new(Box::new(
            ConnectionWrapperReal::new(connection),
        )))
    }

    fn signature_hex(signature: &Signature) -> String {
        format!(
            "0x{}{}{:02x}",
            signature.r.to_hex::<String>(),
            signature.s.to_hex::<String>(),
            signature.v
        )
    }

    #[test]
    fn provider_payout_authorization_is_externally_signed_durable_and_revocable() {
        let directory = ensure_node_home_directory_does_not_exist(
            "provider_settlement",
            "provider_payout_authorization_is_externally_signed_durable_and_revocable",
        );
        create_dir_all(&directory).unwrap();
        let database_path = directory.join("provider-settlement.db");
        let payout_wallet = make_paying_wallet(b"provider payout wallet");
        let provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);

        let mut first = ProviderSettlementManager::new(
            config(&payout_wallet),
            provider.dup(),
            dao(&database_path, true),
            100,
        )
        .unwrap();
        let proposal = first.propose(600, 100).unwrap();
        assert_eq!(
            format!("{:?}", proposal),
            "ProviderSettlementProposal { proposal_data: [REDACTED] }"
        );
        assert_eq!(
            format!(
                "{:?}",
                ProviderSettlementManagerError::Signature(
                    "private provider signature marker".to_string()
                )
            ),
            "Signature([REDACTED])"
        );
        assert_eq!(
            ProviderSettlementManagerError::Signature(
                "private provider signature marker".to_string()
            )
            .to_string(),
            "invalid wallet signature"
        );
        assert_eq!(proposal.policy.provider_public_key, *provider.public_key());
        assert_eq!(
            proposal.policy.payout_wallet_address,
            payout_wallet.address()
        );
        let wallet_signature = payout_wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        let activated = first
            .activate(
                &proposal.proposal_id,
                &signature_hex(&wallet_signature),
                100,
            )
            .unwrap();
        assert!(activated.authorization_opt.is_some());
        assert_eq!(
            format!("{:?}", activated),
            "ProviderSettlementStatus { active: true, pending_claim_count: 0, settlement_data: [REDACTED] }"
        );
        drop(first);

        let mut restored = ProviderSettlementManager::new(
            config(&payout_wallet),
            provider.dup(),
            dao(&database_path, false),
            101,
        )
        .unwrap();
        let restored_status = restored.status(101).unwrap();
        assert!(restored_status.authorization_opt.is_some());
        assert_eq!(restored_status.pending_claim_count, 0);
        assert!(restored.stop().unwrap().authorization_opt.is_none());
        drop(restored);

        let mut after_revoke = ProviderSettlementManager::new(
            config(&payout_wallet),
            provider.dup(),
            dao(&database_path, false),
            102,
        )
        .unwrap();
        assert!(after_revoke
            .status(102)
            .unwrap()
            .authorization_opt
            .is_none());
    }

    #[test]
    fn export_reverifies_pending_receipt_and_builds_contract_compatible_page() {
        let directory = ensure_node_home_directory_does_not_exist(
            "provider_settlement",
            "export_reverifies_pending_receipt_and_builds_contract_compatible_page",
        );
        create_dir_all(&directory).unwrap();
        let database_path = directory.join("provider-settlement-export.db");
        let payout_wallet = make_paying_wallet(b"provider export payout wallet");
        let payer_wallet = make_paying_wallet(b"provider export payer wallet");
        let provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let payer_session = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let route_epoch = [0x71; 32];
        let authorization = ReceiptSessionPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
            payer_wallet.address(),
            payer_session.public_key().clone(),
            1_000,
            100,
            200,
            [0x72; 32],
        )
        .authorize(&payer_wallet)
        .unwrap();
        let acknowledged_receipt = ServiceReceipt::new(
            route_epoch,
            1,
            ServiceKind::Routing,
            provider.public_key().clone(),
            make_accounting_commitment(&route_epoch, payer_session.public_key()),
            10,
            5,
            2,
        )
        .sign(&provider)
        .unwrap()
        .acknowledge(&payer_session)
        .unwrap();
        let checkpoint =
            ReceiptSequenceCheckpoint::begin_for_settlement(&acknowledged_receipt, &provider)
                .unwrap();
        let payload = ServiceReceiptPayload_0v1 {
            authorization,
            acknowledged_receipt,
        };
        let mut store = dao(&database_path, true);
        store
            .accept_verified_receipt(
                &payload,
                &checkpoint,
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(100),
            )
            .unwrap();

        let mut manager =
            ProviderSettlementManager::new(config(&payout_wallet), provider.dup(), store, 100)
                .unwrap();
        let proposal = manager.propose(600, 100).unwrap();
        let wallet_signature = payout_wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        manager
            .activate(
                &proposal.proposal_id,
                &signature_hex(&wallet_signature),
                100,
            )
            .unwrap();

        let export = manager.export(None, 128, 300).unwrap();
        assert_eq!(
            format!("{:?}", export),
            "ProviderSettlementBatchExport { batch_claim_count: 1, total_pending_claims: 1, export_data: [REDACTED] }"
        );
        assert_eq!(export.total_pending_claims, 1);
        assert_eq!(export.start_after_claim_id_opt, None);
        assert_eq!(
            export.next_cursor,
            receipt_settlement_claim_id(&payload).unwrap()
        );
        assert_eq!(export.batch.claims.len(), 1);
        assert_eq!(export.batch.contract_claims.len(), 1);
        assert_eq!(
            export.batch.total_claimed_wei,
            checkpoint.cumulative_charge_wei
        );
        assert_ne!(export.batch.contract_merkle_root, [0u8; 32]);
        assert_eq!(
            serde_cbor::from_slice::<ReceiptSettlementBatch>(
                &serde_cbor::to_vec(&export.batch).unwrap()
            )
            .unwrap(),
            export.batch
        );
        assert_eq!(
            manager.export(Some(export.next_cursor), 128, 300),
            Err(ProviderSettlementManagerError::EmptyPage)
        );
        assert_eq!(
            manager.export(None, 129, 300),
            Err(ProviderSettlementManagerError::ExportLimit(129))
        );

        let reconciliation_page = manager.reconciliation_page(None, 128, 64).unwrap();
        assert_eq!(
            format!("{:?}", reconciliation_page),
            "ProviderSettlementReconciliationPage { candidate_count: 1, reconciliation_data: [REDACTED] }"
        );
        assert_eq!(reconciliation_page.candidates.len(), 1);
        assert_eq!(reconciliation_page.next_cursor, export.next_cursor);
        assert_eq!(
            manager
                .reconcile(&SettlementChainObservation {
                    chain_id: TEST_DEFAULT_CHAIN.rec().num_chain_id,
                    settlement_contract: TEST_DEFAULT_CHAIN.rec().contract,
                    confirmation_depth: 64,
                    latest_block_number: 1_064,
                    observed_block_number: 1_000,
                    observed_block_hash: [0xd1; 32],
                    claims: vec![ConfirmedSettlementClaim {
                        claim_id: export.next_cursor,
                        cumulative_charge_wei: checkpoint.cumulative_charge_wei,
                    }],
                })
                .unwrap(),
            SettlementReconciliationOutcome {
                archived_claim_count: 1,
                restored_claim_count: 0,
                still_pending_claim_count: 0,
                revalidated_archive_count: 0,
                unknown_claim_count: 0,
            }
        );
        assert_eq!(manager.status(300).unwrap().pending_claim_count, 0);
        assert_eq!(
            manager
                .reconciliation_page(None, 128, 64)
                .unwrap()
                .candidates
                .len(),
            1
        );
        assert_eq!(
            manager.reconciliation_page(None, 128, 11),
            Err(ProviderSettlementManagerError::ConfirmationDepth(11))
        );
    }

    #[test]
    fn changed_payout_identity_revokes_stale_durable_authority_without_disabling_manager() {
        let directory = ensure_node_home_directory_does_not_exist(
            "provider_settlement",
            "changed_payout_identity_revokes_stale_durable_authority_without_disabling_manager",
        );
        create_dir_all(&directory).unwrap();
        let database_path = directory.join("provider-settlement-identity-change.db");
        let old_wallet = make_paying_wallet(b"old provider payout wallet");
        let new_wallet = make_paying_wallet(b"new provider payout wallet");
        let provider = CryptDEReal::new(TEST_DEFAULT_CHAIN);
        let mut old_manager = ProviderSettlementManager::new(
            config(&old_wallet),
            provider.dup(),
            dao(&database_path, true),
            100,
        )
        .unwrap();
        let proposal = old_manager.propose(600, 100).unwrap();
        let signature = old_wallet
            .sign(&proposal.policy.eip712_digest().unwrap())
            .unwrap();
        old_manager
            .activate(&proposal.proposal_id, &signature_hex(&signature), 100)
            .unwrap();
        drop(old_manager);

        let mut new_manager = ProviderSettlementManager::new(
            config(&new_wallet),
            provider.dup(),
            dao(&database_path, false),
            101,
        )
        .unwrap();
        assert!(new_manager.status(101).unwrap().authorization_opt.is_none());
        let new_proposal = new_manager.propose(600, 101).unwrap();
        assert_eq!(
            new_proposal.policy.payout_wallet_address,
            new_wallet.address()
        );
    }
}
