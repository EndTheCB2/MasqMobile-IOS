// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::sub_lib::cryptde::CryptDE;
use crate::sub_lib::service_receipt::{
    AuthorizedProviderSettlement, ServiceReceiptError, ServiceReceiptPayload_0v1,
};
use ethereum_types::Address;
use ethsign_crypto::Keccak256;
use serde_derive::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Formatter};

const SETTLEMENT_CLAIM_ID_DOMAIN: &[u8] = b"MASQ_SETTLEMENT_CLAIM_ID_V1\0";
const SETTLEMENT_LEAF_DOMAIN: &[u8] = b"MASQ_SETTLEMENT_LEAF_V1\0";
const SETTLEMENT_NODE_DOMAIN: &[u8] = b"MASQ_SETTLEMENT_NODE_V1\0";
const CONTRACT_SETTLEMENT_LEAF_DOMAIN: &[u8] = b"MASQ_ONCHAIN_SETTLEMENT_LEAF_V1\0";
const CONTRACT_SETTLEMENT_NODE_DOMAIN: &[u8] = b"MASQ_ONCHAIN_SETTLEMENT_NODE_V1\0";

/// Solidity `keccak256(abi.encode(payerWallet, authorizationNonce))` identity used by the escrow.
pub fn receipt_session_contract_id(
    payer_wallet_address: Address,
    authorization_nonce: &[u8; 32],
) -> [u8; 32] {
    let mut session_identity = [0u8; 64];
    session_identity[12..32].copy_from_slice(payer_wallet_address.as_bytes());
    session_identity[32..].copy_from_slice(authorization_nonce);
    session_identity.keccak256()
}

/// Stable privacy-safe identity for one cumulative provider/route stream. It contains the
/// authorization and pseudonymous route identities but no destination or wall-clock data.
pub fn receipt_settlement_claim_id(
    receipt_payload: &ServiceReceiptPayload_0v1,
) -> Result<[u8; 32], ReceiptSettlementError> {
    let authorization = &receipt_payload.authorization.policy;
    let acknowledged_receipt = &receipt_payload.acknowledged_receipt;
    let receipt = &acknowledged_receipt.signed_receipt.receipt;
    let identity = (
        authorization.chain_id,
        authorization.settlement_contract,
        authorization.payer_wallet_address,
        &acknowledged_receipt.payer_session_public_key,
        receipt.route_epoch,
        &receipt.provider_public_key,
        receipt.accounting_commitment,
    );
    let serialized = serde_cbor::to_vec(&identity)
        .map_err(|error| ReceiptSettlementError::Serialization(error.to_string()))?;
    Ok([SETTLEMENT_CLAIM_ID_DOMAIN, &serialized]
        .concat()
        .keccak256())
}

/// Complete off-chain proof needed to attribute one payer-authorized cumulative receipt to the
/// provider's independently authorized payout wallet.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReceiptSettlementClaim {
    pub receipt_payload: ServiceReceiptPayload_0v1,
    pub provider_settlement: AuthorizedProviderSettlement,
}

impl Debug for ReceiptSettlementClaim {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "ReceiptSettlementClaim { receipt_payload: [REDACTED], provider_settlement: [REDACTED] }",
        )
    }
}

/// A verified claim with the privacy-safe identifiers needed by a batcher. The route epoch and
/// claim id remain opaque; neither contains a hostname, URL, IP address, or browse timestamp.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerifiedReceiptSettlementClaim {
    pub claim: ReceiptSettlementClaim,
    pub claim_id: [u8; 32],
    pub leaf_hash: [u8; 32],
    pub payout_wallet_address: Address,
    #[serde(with = "u128_be")]
    pub cumulative_charge_wei: u128,
}

impl Debug for VerifiedReceiptSettlementClaim {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("VerifiedReceiptSettlementClaim { claim_data: [REDACTED] }")
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReceiptSettlementPayout {
    pub payout_wallet_address: Address,
    #[serde(with = "u128_be")]
    pub amount_wei: u128,
}

impl Debug for ReceiptSettlementPayout {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceiptSettlementPayout { payout_data: [REDACTED] }")
    }
}

/// Canonically packed calldata for one cumulative claim accepted by MASQSettlementEscrow.
/// Unlike the portable receipt, on-chain escrow necessarily exposes the payer and payout wallets.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReceiptSettlementContractClaim {
    pub claim_id: [u8; 32],
    pub session_id: [u8; 32],
    pub payer_wallet_address: Address,
    pub payout_wallet_address: Address,
    #[serde(with = "u128_be")]
    pub cumulative_charge_wei: u128,
}

impl Debug for ReceiptSettlementContractClaim {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceiptSettlementContractClaim { contract_claim_data: [REDACTED] }")
    }
}

/// Deterministic off-chain batch material. It is contract-agnostic: an audited settlement
/// contract still has to define proof submission, replay state, challenge rules, and token payout.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReceiptSettlementBatch {
    pub chain_id: u64,
    pub settlement_contract: Address,
    pub claims: Vec<ReceiptSettlementClaim>,
    /// Durable provider-acceptance time for each portable claim, in the same canonical order.
    /// Older artifacts decode with an empty vector but cannot pass independent verification.
    #[serde(default)]
    pub claim_accepted_at_unix_s: Vec<u64>,
    pub leaf_hashes: Vec<[u8; 32]>,
    pub merkle_root: [u8; 32],
    pub payouts: Vec<ReceiptSettlementPayout>,
    #[serde(with = "u128_be")]
    pub total_claimed_wei: u128,
    /// Claims and hashes use the Solidity contract's ABI-packed domains and independent canonical
    /// sort order. Defaults preserve decoding of pre-contract batch artifacts.
    #[serde(default)]
    pub contract_claims: Vec<ReceiptSettlementContractClaim>,
    #[serde(default)]
    pub contract_leaf_hashes: Vec<[u8; 32]>,
    #[serde(default)]
    pub contract_merkle_root: [u8; 32],
}

impl Debug for ReceiptSettlementBatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ReceiptSettlementBatch {{ claim_count: {}, payout_count: {}, contract_claim_count: {}, settlement_data: [REDACTED] }}",
            self.claims.len(),
            self.payouts.len(),
            self.contract_claims.len()
        )
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum ReceiptSettlementError {
    AcceptanceTimeCountMismatch {
        claim_count: usize,
        acceptance_time_count: usize,
    },
    AcceptanceTimeInFuture(u64),
    AmountOverflow,
    ArtifactMismatch,
    DuplicateClaim([u8; 32]),
    EmptyBatch,
    Receipt(ServiceReceiptError),
    Serialization(String),
}

impl Debug for ReceiptSettlementError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AcceptanceTimeCountMismatch {
                claim_count,
                acceptance_time_count,
            } => f
                .debug_struct("AcceptanceTimeCountMismatch")
                .field("claim_count", claim_count)
                .field("acceptance_time_count", acceptance_time_count)
                .finish(),
            Self::AcceptanceTimeInFuture(_) => f.write_str("AcceptanceTimeInFuture([REDACTED])"),
            Self::AmountOverflow => f.write_str("AmountOverflow"),
            Self::ArtifactMismatch => f.write_str("ArtifactMismatch"),
            Self::DuplicateClaim(_) => f.write_str("DuplicateClaim([REDACTED])"),
            Self::EmptyBatch => f.write_str("EmptyBatch"),
            Self::Receipt(error) => f.debug_tuple("Receipt").field(error).finish(),
            Self::Serialization(_) => f.write_str("Serialization([REDACTED])"),
        }
    }
}

impl From<ServiceReceiptError> for ReceiptSettlementError {
    fn from(error: ServiceReceiptError) -> Self {
        Self::Receipt(error)
    }
}

impl ReceiptSettlementClaim {
    pub fn new(
        receipt_payload: ServiceReceiptPayload_0v1,
        provider_settlement: AuthorizedProviderSettlement,
    ) -> Self {
        Self {
            receipt_payload,
            provider_settlement,
        }
    }

    pub fn verify(
        &self,
        expected_chain_id: u64,
        expected_settlement_contract: Address,
        now_unix_s: u64,
        receipt_cryptde: &dyn CryptDE,
    ) -> Result<VerifiedReceiptSettlementClaim, ReceiptSettlementError> {
        self.verify_at(
            expected_chain_id,
            expected_settlement_contract,
            now_unix_s,
            now_unix_s,
            receipt_cryptde,
        )
    }

    /// Revalidates the payer receipt at the durable provider-acceptance time while requiring the
    /// payout authority to be valid now. This preserves crash recovery after a payer session
    /// expires without pretending that the payer signed a wall-clock timestamp in the receipt.
    pub fn verify_at(
        &self,
        expected_chain_id: u64,
        expected_settlement_contract: Address,
        receipt_accepted_at_unix_s: u64,
        provider_authorization_now_unix_s: u64,
        receipt_cryptde: &dyn CryptDE,
    ) -> Result<VerifiedReceiptSettlementClaim, ReceiptSettlementError> {
        let acknowledged_receipt = &self.receipt_payload.acknowledged_receipt;
        self.receipt_payload.authorization.verify_for_receipt(
            acknowledged_receipt,
            receipt_cryptde,
            expected_chain_id,
            expected_settlement_contract,
            receipt_accepted_at_unix_s,
        )?;
        let receipt = &acknowledged_receipt.signed_receipt.receipt;
        let payout_wallet_address = self.provider_settlement.verify(
            expected_chain_id,
            expected_settlement_contract,
            &receipt.provider_public_key,
            provider_authorization_now_unix_s,
            receipt_cryptde,
        )?;
        Ok(VerifiedReceiptSettlementClaim {
            claim: self.clone(),
            claim_id: self.claim_id()?,
            leaf_hash: self.leaf_hash()?,
            payout_wallet_address,
            cumulative_charge_wei: receipt.cumulative_charge_wei,
        })
    }

    fn claim_id(&self) -> Result<[u8; 32], ReceiptSettlementError> {
        receipt_settlement_claim_id(&self.receipt_payload)
    }

    fn leaf_hash(&self) -> Result<[u8; 32], ReceiptSettlementError> {
        let serialized = serde_cbor::to_vec(self)
            .map_err(|error| ReceiptSettlementError::Serialization(error.to_string()))?;
        Ok([SETTLEMENT_LEAF_DOMAIN, &serialized].concat().keccak256())
    }
}

impl VerifiedReceiptSettlementClaim {
    fn contract_claim(&self) -> ReceiptSettlementContractClaim {
        let authorization = &self.claim.receipt_payload.authorization.policy;
        ReceiptSettlementContractClaim {
            claim_id: self.claim_id,
            session_id: receipt_session_contract_id(
                authorization.payer_wallet_address,
                &authorization.authorization_nonce,
            ),
            payer_wallet_address: authorization.payer_wallet_address,
            payout_wallet_address: self.payout_wallet_address,
            cumulative_charge_wei: self.cumulative_charge_wei,
        }
    }
}

impl ReceiptSettlementContractClaim {
    pub fn leaf_hash(&self, chain_id: u64, settlement_contract: Address) -> [u8; 32] {
        let mut chain_word = [0u8; 32];
        chain_word[24..].copy_from_slice(&chain_id.to_be_bytes());
        [
            CONTRACT_SETTLEMENT_LEAF_DOMAIN,
            &chain_word,
            settlement_contract.as_bytes(),
            &self.claim_id,
            &self.session_id,
            self.payer_wallet_address.as_bytes(),
            self.payout_wallet_address.as_bytes(),
            &self.cumulative_charge_wei.to_be_bytes(),
        ]
        .concat()
        .keccak256()
    }
}

impl ReceiptSettlementBatch {
    pub fn build(
        claims: Vec<ReceiptSettlementClaim>,
        expected_chain_id: u64,
        expected_settlement_contract: Address,
        now_unix_s: u64,
        receipt_cryptde: &dyn CryptDE,
    ) -> Result<Self, ReceiptSettlementError> {
        if claims.is_empty() {
            return Err(ReceiptSettlementError::EmptyBatch);
        }
        let verified = claims
            .iter()
            .map(|claim| {
                claim
                    .verify(
                        expected_chain_id,
                        expected_settlement_contract,
                        now_unix_s,
                        receipt_cryptde,
                    )
                    .map(|verified| (verified, now_unix_s))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_verified(verified, expected_chain_id, expected_settlement_contract)
    }

    pub fn build_from_accepted(
        claims: Vec<(ReceiptSettlementClaim, u64)>,
        expected_chain_id: u64,
        expected_settlement_contract: Address,
        provider_authorization_now_unix_s: u64,
        receipt_cryptde: &dyn CryptDE,
    ) -> Result<Self, ReceiptSettlementError> {
        if claims.is_empty() {
            return Err(ReceiptSettlementError::EmptyBatch);
        }
        if let Some((_, accepted_at_unix_s)) = claims
            .iter()
            .find(|(_, accepted_at_unix_s)| *accepted_at_unix_s > provider_authorization_now_unix_s)
        {
            return Err(ReceiptSettlementError::AcceptanceTimeInFuture(
                *accepted_at_unix_s,
            ));
        }
        let verified = claims
            .iter()
            .map(|(claim, accepted_at_unix_s)| {
                claim
                    .verify_at(
                        expected_chain_id,
                        expected_settlement_contract,
                        *accepted_at_unix_s,
                        provider_authorization_now_unix_s,
                        receipt_cryptde,
                    )
                    .map(|verified| (verified, *accepted_at_unix_s))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_verified(verified, expected_chain_id, expected_settlement_contract)
    }

    /// Reconstructs every signature, identity, amount, ordering and root from the portable
    /// claims. A batcher must run this against canonical CBOR instead of trusting the convenient
    /// contract-claim projection printed by MASQNode.
    pub fn verify_exported(
        &self,
        provider_authorization_now_unix_s: u64,
        receipt_cryptde: &dyn CryptDE,
    ) -> Result<(), ReceiptSettlementError> {
        if self.claims.len() != self.claim_accepted_at_unix_s.len() {
            return Err(ReceiptSettlementError::AcceptanceTimeCountMismatch {
                claim_count: self.claims.len(),
                acceptance_time_count: self.claim_accepted_at_unix_s.len(),
            });
        }
        let reconstructed = Self::build_from_accepted(
            self.claims
                .iter()
                .cloned()
                .zip(self.claim_accepted_at_unix_s.iter().copied())
                .collect(),
            self.chain_id,
            self.settlement_contract,
            provider_authorization_now_unix_s,
            receipt_cryptde,
        )?;
        if &reconstructed != self {
            return Err(ReceiptSettlementError::ArtifactMismatch);
        }
        Ok(())
    }

    fn from_verified(
        mut verified: Vec<(VerifiedReceiptSettlementClaim, u64)>,
        expected_chain_id: u64,
        expected_settlement_contract: Address,
    ) -> Result<Self, ReceiptSettlementError> {
        let mut claim_ids = BTreeSet::new();
        for (claim, _) in &verified {
            if !claim_ids.insert(claim.claim_id) {
                return Err(ReceiptSettlementError::DuplicateClaim(claim.claim_id));
            }
        }
        verified.sort_by_key(|(claim, _)| claim.leaf_hash);

        let mut payout_totals = BTreeMap::<Address, u128>::new();
        let mut total_claimed_wei = 0u128;
        for (claim, _) in &verified {
            total_claimed_wei = total_claimed_wei
                .checked_add(claim.cumulative_charge_wei)
                .ok_or(ReceiptSettlementError::AmountOverflow)?;
            let current_payout = payout_totals
                .entry(claim.payout_wallet_address)
                .or_insert(0);
            *current_payout = current_payout
                .checked_add(claim.cumulative_charge_wei)
                .ok_or(ReceiptSettlementError::AmountOverflow)?;
        }
        let leaf_hashes = verified
            .iter()
            .map(|(claim, _)| claim.leaf_hash)
            .collect::<Vec<_>>();
        let merkle_root = merkle_root(&leaf_hashes);
        let mut contract_claims = verified
            .iter()
            .map(|(claim, _)| claim.contract_claim())
            .collect::<Vec<_>>();
        contract_claims
            .sort_by_key(|claim| claim.leaf_hash(expected_chain_id, expected_settlement_contract));
        let contract_leaf_hashes = contract_claims
            .iter()
            .map(|claim| claim.leaf_hash(expected_chain_id, expected_settlement_contract))
            .collect::<Vec<_>>();
        let contract_merkle_root = contract_merkle_root(&contract_leaf_hashes);
        let payouts = payout_totals
            .into_iter()
            .map(
                |(payout_wallet_address, amount_wei)| ReceiptSettlementPayout {
                    payout_wallet_address,
                    amount_wei,
                },
            )
            .collect();

        Ok(Self {
            chain_id: expected_chain_id,
            settlement_contract: expected_settlement_contract,
            claims: verified
                .iter()
                .map(|(claim, _)| claim.claim.clone())
                .collect(),
            claim_accepted_at_unix_s: verified
                .into_iter()
                .map(|(_, accepted_at_unix_s)| accepted_at_unix_s)
                .collect(),
            leaf_hashes,
            merkle_root,
            payouts,
            total_claimed_wei,
            contract_claims,
            contract_leaf_hashes,
            contract_merkle_root,
        })
    }
}

fn merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    let mut level = leaves.to_vec();
    while level.len() > 1 {
        level = level
            .chunks(2)
            .map(|pair| {
                let right = pair.get(1).unwrap_or(&pair[0]);
                [SETTLEMENT_NODE_DOMAIN, &pair[0], right]
                    .concat()
                    .keccak256()
            })
            .collect();
    }
    level[0]
}

fn contract_merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    let mut level = leaves.to_vec();
    while level.len() > 1 {
        level = level
            .chunks(2)
            .map(|pair| {
                let right = pair.get(1).unwrap_or(&pair[0]);
                [CONTRACT_SETTLEMENT_NODE_DOMAIN, &pair[0], right]
                    .concat()
                    .keccak256()
            })
            .collect();
    }
    level[0]
}

mod u128_be {
    use serde::de::{Error, Visitor};
    use serde::{Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(&value.to_be_bytes())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct U128Visitor;

        impl<'de> Visitor<'de> for U128Visitor {
            type Value = u128;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("exactly 16 big-endian bytes")
            }

            fn visit_bytes<E>(self, bytes: &[u8]) -> Result<Self::Value, E>
            where
                E: Error,
            {
                let fixed: [u8; 16] = bytes
                    .try_into()
                    .map_err(|_| E::invalid_length(bytes.len(), &"exactly 16 big-endian bytes"))?;
                Ok(u128::from_be_bytes(fixed))
            }

            fn visit_byte_buf<E>(self, bytes: Vec<u8>) -> Result<Self::Value, E>
            where
                E: Error,
            {
                self.visit_bytes(&bytes)
            }
        }

        deserializer.deserialize_bytes(U128Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub_lib::cryptde::PublicKey;
    use crate::sub_lib::cryptde_null::CryptDENull;
    use crate::sub_lib::service_receipt::{
        make_accounting_commitment, ProviderSettlementPolicy, ReceiptSessionPolicy, ServiceKind,
        ServiceReceipt,
    };
    use crate::sub_lib::wallet::Wallet;
    use crate::test_utils::make_paying_wallet;
    use masq_lib::test_utils::utils::TEST_DEFAULT_CHAIN;
    use rustc_hex::FromHex;

    fn hex_32(value: &str) -> [u8; 32] {
        let bytes: Vec<u8> = value.from_hex().unwrap();
        assert_eq!(bytes.len(), 32);
        let mut result = [0u8; 32];
        result.copy_from_slice(&bytes);
        result
    }

    fn make_claim(tag: u8, payout_wallet: &Wallet) -> (ReceiptSettlementClaim, CryptDENull) {
        let chain = TEST_DEFAULT_CHAIN;
        let provider_public_key = PublicKey::new(&vec![tag; 24]);
        let payer_session_public_key = PublicKey::new(&vec![tag.wrapping_add(1); 24]);
        let provider_cryptde = CryptDENull::from(&provider_public_key, chain);
        let payer_cryptde = CryptDENull::from(&payer_session_public_key, chain);
        let payer_wallet = make_paying_wallet(&[tag, 0x50]);
        let route_epoch = [tag; 32];
        let receipt = ServiceReceipt::new(
            route_epoch,
            u64::from(tag),
            ServiceKind::Exit,
            provider_public_key.clone(),
            make_accounting_commitment(&route_epoch, &payer_session_public_key),
            1_000 + u64::from(tag),
            500,
            2,
        );
        let acknowledged_receipt = receipt
            .sign(&provider_cryptde)
            .unwrap()
            .acknowledge(&payer_cryptde)
            .unwrap();
        let authorization = ReceiptSessionPolicy::new(
            chain.rec().num_chain_id,
            chain.rec().contract,
            payer_wallet.address(),
            payer_session_public_key,
            1_000_000,
            100,
            200,
            [tag.wrapping_add(2); 32],
        )
        .authorize(&payer_wallet)
        .unwrap();
        let provider_settlement = ProviderSettlementPolicy::new(
            chain.rec().num_chain_id,
            chain.rec().contract,
            payout_wallet.address(),
            provider_public_key,
            100,
            200,
            [tag.wrapping_add(3); 32],
        )
        .authorize(payout_wallet, &provider_cryptde)
        .unwrap();
        (
            ReceiptSettlementClaim::new(
                ServiceReceiptPayload_0v1 {
                    authorization,
                    acknowledged_receipt,
                },
                provider_settlement,
            ),
            provider_cryptde,
        )
    }

    #[test]
    fn claim_verifies_bilateral_receipt_and_provider_payout_authorization() {
        let payout_wallet = make_paying_wallet(b"settlement claim payout");
        let (claim, verifier) = make_claim(0x11, &payout_wallet);
        let expected_amount = claim
            .receipt_payload
            .acknowledged_receipt
            .signed_receipt
            .receipt
            .cumulative_charge_wei;

        let verified = claim
            .verify(
                TEST_DEFAULT_CHAIN.rec().num_chain_id,
                TEST_DEFAULT_CHAIN.rec().contract,
                150,
                &verifier,
            )
            .unwrap();

        assert_eq!(verified.payout_wallet_address, payout_wallet.address());
        assert_eq!(verified.cumulative_charge_wei, expected_amount);
        assert_ne!(verified.claim_id, [0; 32]);
        assert_ne!(verified.leaf_hash, [0; 32]);
        assert_eq!(
            format!("{:?}", claim),
            "ReceiptSettlementClaim { receipt_payload: [REDACTED], provider_settlement: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", verified),
            "VerifiedReceiptSettlementClaim { claim_data: [REDACTED] }"
        );
    }

    #[test]
    fn claim_rejects_a_payout_authorization_for_another_provider() {
        let payout_wallet = make_paying_wallet(b"mismatched settlement payout");
        let (mut claim, verifier) = make_claim(0x21, &payout_wallet);
        let (other_claim, _) = make_claim(0x22, &payout_wallet);
        claim.provider_settlement = other_claim.provider_settlement;

        assert_eq!(
            claim.verify(
                TEST_DEFAULT_CHAIN.rec().num_chain_id,
                TEST_DEFAULT_CHAIN.rec().contract,
                150,
                &verifier,
            ),
            Err(ReceiptSettlementError::Receipt(
                ServiceReceiptError::ProviderPublicKeyMismatch
            ))
        );
    }

    #[test]
    fn batch_is_order_independent_and_aggregates_payout_wallets() {
        let payout_a = make_paying_wallet(b"batch payout a");
        let payout_b = make_paying_wallet(b"batch payout b");
        let (claim_a1, verifier) = make_claim(0x31, &payout_a);
        let (claim_a2, _) = make_claim(0x32, &payout_a);
        let (claim_b, _) = make_claim(0x33, &payout_b);
        let chain_id = TEST_DEFAULT_CHAIN.rec().num_chain_id;
        let contract = TEST_DEFAULT_CHAIN.rec().contract;

        let forward = ReceiptSettlementBatch::build(
            vec![claim_a1.clone(), claim_a2.clone(), claim_b.clone()],
            chain_id,
            contract,
            150,
            &verifier,
        )
        .unwrap();
        let reverse = ReceiptSettlementBatch::build(
            vec![claim_b, claim_a2, claim_a1],
            chain_id,
            contract,
            150,
            &verifier,
        )
        .unwrap();

        assert_eq!(forward, reverse);
        assert_eq!(forward.claims.len(), 3);
        assert_eq!(forward.leaf_hashes.len(), 3);
        assert_eq!(forward.payouts.len(), 2);
        assert_eq!(forward.contract_claims.len(), 3);
        assert_eq!(forward.contract_leaf_hashes.len(), 3);
        assert_ne!(forward.contract_merkle_root, [0; 32]);
        assert!(forward
            .contract_leaf_hashes
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert_eq!(
            forward
                .payouts
                .iter()
                .map(|payout| payout.amount_wei)
                .sum::<u128>(),
            forward.total_claimed_wei
        );
        assert_ne!(forward.merkle_root, [0; 32]);
        assert_eq!(
            format!("{:?}", forward),
            "ReceiptSettlementBatch { claim_count: 3, payout_count: 2, contract_claim_count: 3, settlement_data: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", forward.payouts[0]),
            "ReceiptSettlementPayout { payout_data: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", forward.contract_claims[0]),
            "ReceiptSettlementContractClaim { contract_claim_data: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", ReceiptSettlementError::DuplicateClaim([0x61; 32])),
            "DuplicateClaim([REDACTED])"
        );
        let serialized = serde_cbor::to_vec(&forward).unwrap();
        assert_eq!(
            serde_cbor::from_slice::<ReceiptSettlementBatch>(&serialized).unwrap(),
            forward
        );
    }

    #[test]
    fn exported_batch_reconstructs_all_proofs_and_rejects_incomplete_or_tampered_material() {
        let payout_wallet = make_paying_wallet(b"independent batch verification payout");
        let (claim_a, verifier) = make_claim(0x34, &payout_wallet);
        let (claim_b, _) = make_claim(0x35, &payout_wallet);
        let chain_id = TEST_DEFAULT_CHAIN.rec().num_chain_id;
        let contract = TEST_DEFAULT_CHAIN.rec().contract;
        let batch = ReceiptSettlementBatch::build_from_accepted(
            vec![(claim_a, 120), (claim_b, 130)],
            chain_id,
            contract,
            150,
            &verifier,
        )
        .unwrap();

        assert_eq!(batch.verify_exported(150, &verifier), Ok(()));
        assert_eq!(batch.claims.len(), batch.claim_accepted_at_unix_s.len());

        let mut missing_acceptance_time = batch.clone();
        missing_acceptance_time.claim_accepted_at_unix_s.pop();
        assert_eq!(
            missing_acceptance_time.verify_exported(150, &verifier),
            Err(ReceiptSettlementError::AcceptanceTimeCountMismatch {
                claim_count: 2,
                acceptance_time_count: 1,
            })
        );

        let mut tampered_root = batch.clone();
        tampered_root.contract_merkle_root[0] ^= 1;
        assert_eq!(
            tampered_root.verify_exported(150, &verifier),
            Err(ReceiptSettlementError::ArtifactMismatch)
        );

        let mut future_acceptance = batch;
        future_acceptance.claim_accepted_at_unix_s[0] = 151;
        assert_eq!(
            future_acceptance.verify_exported(150, &verifier),
            Err(ReceiptSettlementError::AcceptanceTimeInFuture(151))
        );
    }

    #[test]
    fn contract_session_leaf_and_odd_merkle_root_match_ethers_solidity_vectors() {
        let chain_id = 84_532;
        let settlement_contract = Address::from([0x11; 20]);
        let payer_wallet_address = Address::from([0x22; 20]);
        let payout_wallet_address = Address::from([0x55; 20]);
        let session_id = receipt_session_contract_id(payer_wallet_address, &[0x33; 32]);
        assert_eq!(
            session_id,
            hex_32("b46d842f632dc5321a4932c48ac1bdde3cab6f16817756baad98e26c2ccd4b81")
        );

        let mut contract_claims = [
            (0x44, 9_876_543_210u128),
            (0x45, 9_876_543_211u128),
            (0x46, 9_876_543_212u128),
        ]
        .iter()
        .map(
            |(tag, cumulative_charge_wei)| ReceiptSettlementContractClaim {
                claim_id: [*tag; 32],
                session_id,
                payer_wallet_address,
                payout_wallet_address,
                cumulative_charge_wei: *cumulative_charge_wei,
            },
        )
        .collect::<Vec<_>>();
        contract_claims.sort_by_key(|claim| claim.leaf_hash(chain_id, settlement_contract));
        let leaf_hashes = contract_claims
            .iter()
            .map(|claim| claim.leaf_hash(chain_id, settlement_contract))
            .collect::<Vec<_>>();

        assert_eq!(
            leaf_hashes,
            vec![
                hex_32("586b6a83f9859a79434deecc40d0e196bf4a8a5357d9ecf7c037143a5063e322"),
                hex_32("a56e64fe6cf8018c1778da1cfd80d6468e737c86be6b40fbd9123b67e48f930e"),
                hex_32("abb70d5438fbb17662e150356a0561f6b5f92791a99f3390659b86146392bd4b"),
            ]
        );
        assert_eq!(
            contract_merkle_root(&leaf_hashes),
            hex_32("88ff4e8191cc333729bfebd2541f3c0af3213b64150313a8f866ecd050c1b829")
        );
    }

    #[test]
    fn batch_rejects_empty_or_exact_duplicate_claims() {
        let payout_wallet = make_paying_wallet(b"duplicate batch payout");
        let (claim, verifier) = make_claim(0x41, &payout_wallet);
        let chain_id = TEST_DEFAULT_CHAIN.rec().num_chain_id;
        let contract = TEST_DEFAULT_CHAIN.rec().contract;

        assert_eq!(
            ReceiptSettlementBatch::build(vec![], chain_id, contract, 150, &verifier),
            Err(ReceiptSettlementError::EmptyBatch)
        );
        let claim_id = claim
            .verify(chain_id, contract, 150, &verifier)
            .unwrap()
            .claim_id;
        assert_eq!(
            ReceiptSettlementBatch::build(
                vec![claim.clone(), claim],
                chain_id,
                contract,
                150,
                &verifier,
            ),
            Err(ReceiptSettlementError::DuplicateClaim(claim_id))
        );
    }
}
