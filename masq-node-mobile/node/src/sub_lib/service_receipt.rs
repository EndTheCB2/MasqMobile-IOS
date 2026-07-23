// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::blockchain::signature::SerializableSignature;
use crate::sub_lib::cryptde::{CryptDE, CryptData, CryptdecError, PlainData, PublicKey};
use crate::sub_lib::wallet::Wallet;
use ethereum_types::Address;
use ethsign::Signature;
use ethsign_crypto::Keccak256;
use rustc_hex::ToHex;
use serde_derive::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::{Debug, Formatter};

pub const SERVICE_RECEIPT_PROTOCOL_VERSION: u16 = 1;
pub const MIN_RECEIPT_SESSION_DURATION_SECONDS: u64 = 60;
pub const MAX_RECEIPT_SESSION_DURATION_SECONDS: u64 = 24 * 60 * 60;
pub const MAX_RECEIPT_SESSION_CHARGE_WEI: u128 = (1u128 << 126) - 1;
pub const MIN_PROVIDER_SETTLEMENT_DURATION_SECONDS: u64 = 60;
pub const MAX_PROVIDER_SETTLEMENT_DURATION_SECONDS: u64 = 30 * 24 * 60 * 60;
pub const SERVICE_RECEIPT_SETTLEMENT_V1_CAPABILITY: u64 = 1 << 0;
/// A node advertising this bit can carry hop-local receipt authorizations and preserve encrypted
/// routing-receipt offers across the request/response CORES round trip.
pub const ROUTING_RECEIPT_V1_CAPABILITY: u64 = 1 << 1;
pub const CURRENT_PROTOCOL_CAPABILITIES: u64 =
    SERVICE_RECEIPT_SETTLEMENT_V1_CAPABILITY | ROUTING_RECEIPT_V1_CAPABILITY;
const ACCOUNTING_COMMITMENT_DOMAIN: &[u8] = b"MASQ_ACCOUNTING_COMMITMENT_V1\0";
const PAYER_ACKNOWLEDGEMENT_DOMAIN: &[u8] = b"MASQ_SERVICE_RECEIPT_ACK_V1\0";
const PROVIDER_SETTLEMENT_SIGNING_DOMAIN: &[u8] = b"MASQ_PROVIDER_SETTLEMENT_TRANSPORT_V1\0";
const SERVICE_RECEIPT_SIGNING_DOMAIN: &[u8] = b"MASQ_SERVICE_RECEIPT_V1\0";
const EIP712_DOMAIN_TYPE: &[u8] =
    b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";
const EIP712_NAME: &[u8] = b"MASQ Service Receipt";
const EIP712_VERSION: &[u8] = b"1";
const PROVIDER_SETTLEMENT_EIP712_NAME: &[u8] = b"MASQ Provider Settlement";
const PROVIDER_SETTLEMENT_AUTHORIZATION_TYPE: &[u8] = b"ProviderSettlementAuthorization(uint16 protocolVersion,address payoutWallet,bytes providerPublicKey,uint64 validFromUnixS,uint64 expiresAtUnixS,bytes32 authorizationNonce)";
const SESSION_AUTHORIZATION_TYPE: &[u8] = b"ReceiptSessionAuthorization(uint16 protocolVersion,address payerWallet,bytes payerSessionPublicKey,uint256 maxTotalChargeWei,uint64 validFromUnixS,uint64 expiresAtUnixS,bytes32 authorizationNonce)";
const SECP256K1_HALF_ORDER: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b, 0x20, 0xa0,
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ServiceKind {
    Routing,
    Exit,
}

/// A compact proof of billable MASQ service. It deliberately contains no hostname, URL, IP
/// address, or wall-clock timestamp. `route_epoch` and `accounting_commitment` are opaque values
/// supplied by the consumer, so a receipt can be reconciled without disclosing browsing intent.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServiceReceipt {
    pub protocol_version: u16,
    pub route_epoch: [u8; 32],
    pub sequence: u64,
    pub service_kind: ServiceKind,
    pub provider_public_key: PublicKey,
    pub accounting_commitment: [u8; 32],
    pub payload_size: u64,
    #[serde(default = "default_service_units")]
    pub service_units: u64,
    pub service_rate: u64,
    pub byte_rate: u64,
    #[serde(with = "u128_be")]
    pub cumulative_charge_wei: u128,
}

impl Debug for ServiceReceipt {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ServiceReceipt {{ protocol_version: {}, service_kind: {:?}, receipt_data: [REDACTED] }}",
            self.protocol_version, self.service_kind
        )
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignedServiceReceipt {
    pub receipt: ServiceReceipt,
    pub provider_signature: CryptData,
}

impl Debug for SignedServiceReceipt {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("SignedServiceReceipt { receipt: [REDACTED], provider_signature: [REDACTED] }")
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct AcknowledgedServiceReceipt {
    pub signed_receipt: SignedServiceReceipt,
    pub payer_session_public_key: PublicKey,
    pub payer_signature: CryptData,
}

impl Debug for AcknowledgedServiceReceipt {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "AcknowledgedServiceReceipt { signed_receipt: [REDACTED], payer_session_public_key: [REDACTED], payer_signature: [REDACTED] }",
        )
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReceiptSessionPolicy {
    pub protocol_version: u16,
    pub chain_id: u64,
    pub settlement_contract: Address,
    pub payer_wallet_address: Address,
    pub payer_session_public_key: PublicKey,
    #[serde(with = "u128_be")]
    pub max_total_charge_wei: u128,
    pub valid_from_unix_s: u64,
    pub expires_at_unix_s: u64,
    pub authorization_nonce: [u8; 32],
}

impl Debug for ReceiptSessionPolicy {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceiptSessionPolicy { authorization_policy: [REDACTED] }")
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthorizedReceiptSession {
    pub policy: ReceiptSessionPolicy,
    #[serde(with = "SerializableSignature")]
    pub wallet_signature: Signature,
}

impl Debug for AuthorizedReceiptSession {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthorizedReceiptSession { policy: [REDACTED], wallet_signature: [REDACTED] }")
    }
}

/// Privacy-preserving payout authority for a receipt provider. This policy is deliberately kept
/// separate from browsing receipts: settlement can prove which wallet may claim for a provider
/// key without putting that wallet address in every portable service record.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderSettlementPolicy {
    pub protocol_version: u16,
    pub chain_id: u64,
    pub settlement_contract: Address,
    pub payout_wallet_address: Address,
    pub provider_public_key: PublicKey,
    pub valid_from_unix_s: u64,
    pub expires_at_unix_s: u64,
    pub authorization_nonce: [u8; 32],
}

impl Debug for ProviderSettlementPolicy {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderSettlementPolicy { authorization_policy: [REDACTED] }")
    }
}

/// Both identities consent to the same payout binding: the wallet signs wallet-readable EIP-712
/// data and the MASQ transport key co-signs the exact policy and wallet signature.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthorizedProviderSettlement {
    pub policy: ProviderSettlementPolicy,
    #[serde(with = "SerializableSignature")]
    pub payout_wallet_signature: Signature,
    pub provider_signature: CryptData,
}

impl Debug for AuthorizedProviderSettlement {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "AuthorizedProviderSettlement { policy: [REDACTED], payout_wallet_signature: [REDACTED], provider_signature: [REDACTED] }",
        )
    }
}

/// Authorization material carried inside the end-to-end encrypted client request. Routing peers
/// cannot inspect it. The exit can validate the wallet policy before serving, while the opaque
/// epoch lets the consumer correlate later offers without putting a hostname or stream key in a
/// portable receipt.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReceiptSessionRequest {
    pub authorization: AuthorizedReceiptSession,
    pub route_epoch: [u8; 32],
    pub accounting_commitment: [u8; 32],
}

impl Debug for ReceiptSessionRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "ReceiptSessionRequest { authorization: [REDACTED], route_epoch: [REDACTED], accounting_commitment: [REDACTED] }",
        )
    }
}

/// Wire payload returned to the service provider after the payer has acknowledged a receipt.
/// Versioning is handled by `VersionedData`; the protocol version inside both signed objects is
/// independently verified so neither wire migration nor deserialization can change consent.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[allow(non_camel_case_types)]
pub struct ServiceReceiptPayload_0v1 {
    pub authorization: AuthorizedReceiptSession,
    pub acknowledged_receipt: AcknowledgedServiceReceipt,
}

impl Debug for ServiceReceiptPayload_0v1 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "ServiceReceiptPayload_0v1 { authorization: [REDACTED], acknowledged_receipt: [REDACTED] }",
        )
    }
}

/// Provider-to-consumer half of the receipt exchange. It carries no authorization or destination
/// data; the consumer binds it to an explicitly authorized local session before acknowledging it.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[allow(non_camel_case_types)]
pub struct ServiceReceiptOfferPayload_0v1 {
    pub signed_receipt: SignedServiceReceipt,
}

impl Debug for ServiceReceiptOfferPayload_0v1 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServiceReceiptOfferPayload_0v1 { signed_receipt: [REDACTED] }")
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReceiptSequenceCheckpoint {
    pub route_epoch: [u8; 32],
    pub provider_public_key: PublicKey,
    pub accounting_commitment: [u8; 32],
    pub payer_session_public_key: PublicKey,
    pub last_sequence: u64,
    #[serde(with = "u128_be")]
    pub cumulative_charge_wei: u128,
}

impl Debug for ReceiptSequenceCheckpoint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceiptSequenceCheckpoint { checkpoint_data: [REDACTED] }")
    }
}

pub(crate) mod u128_be {
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

#[derive(Clone, Eq, PartialEq)]
pub enum ServiceReceiptError {
    AmountLimitExceeded,
    AccountingCommitmentMismatch,
    CumulativeChargeMismatch,
    CumulativeChargeOverflow,
    EmptyAmountLimit,
    EmptyAuthorizationNonce,
    EmptyAccountingCommitment,
    EmptyPayerSessionPublicKey,
    EmptyPayerWallet,
    EmptyProviderPayoutWallet,
    EmptyProviderPublicKey,
    EmptyRouteEpoch,
    EmptyServiceUnits,
    EmptySettlementContract,
    InvalidChain,
    InvalidAuthorizationWindow,
    InvalidPayerSignature,
    InvalidProviderSignature,
    InvalidProviderSettlementSignature,
    InvalidWalletSignature,
    NonMonotonicSequence,
    ProviderPublicKeyMismatch,
    ProviderSettlementAuthorizationExpired,
    ProviderSettlementAuthorizationNotYetValid,
    RouteEpochMismatch,
    SessionAuthorizationExpired,
    SessionAuthorizationNotYetValid,
    SessionKeyMismatch,
    Serialization(String),
    SettlementContractMismatch,
    Signing(CryptdecError),
    WalletMismatch,
    WalletSigning(String),
    WrongChain,
    UnsupportedProtocolVersion(u16),
}

impl Debug for ServiceReceiptError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::AmountLimitExceeded => "AmountLimitExceeded",
            Self::AccountingCommitmentMismatch => "AccountingCommitmentMismatch",
            Self::CumulativeChargeMismatch => "CumulativeChargeMismatch",
            Self::CumulativeChargeOverflow => "CumulativeChargeOverflow",
            Self::EmptyAmountLimit => "EmptyAmountLimit",
            Self::EmptyAuthorizationNonce => "EmptyAuthorizationNonce",
            Self::EmptyAccountingCommitment => "EmptyAccountingCommitment",
            Self::EmptyPayerSessionPublicKey => "EmptyPayerSessionPublicKey",
            Self::EmptyPayerWallet => "EmptyPayerWallet",
            Self::EmptyProviderPayoutWallet => "EmptyProviderPayoutWallet",
            Self::EmptyProviderPublicKey => "EmptyProviderPublicKey",
            Self::EmptyRouteEpoch => "EmptyRouteEpoch",
            Self::EmptyServiceUnits => "EmptyServiceUnits",
            Self::EmptySettlementContract => "EmptySettlementContract",
            Self::InvalidChain => "InvalidChain",
            Self::InvalidAuthorizationWindow => "InvalidAuthorizationWindow",
            Self::InvalidPayerSignature => "InvalidPayerSignature",
            Self::InvalidProviderSignature => "InvalidProviderSignature",
            Self::InvalidProviderSettlementSignature => "InvalidProviderSettlementSignature",
            Self::InvalidWalletSignature => "InvalidWalletSignature",
            Self::NonMonotonicSequence => "NonMonotonicSequence",
            Self::ProviderPublicKeyMismatch => "ProviderPublicKeyMismatch",
            Self::ProviderSettlementAuthorizationExpired => {
                "ProviderSettlementAuthorizationExpired"
            }
            Self::ProviderSettlementAuthorizationNotYetValid => {
                "ProviderSettlementAuthorizationNotYetValid"
            }
            Self::RouteEpochMismatch => "RouteEpochMismatch",
            Self::SessionAuthorizationExpired => "SessionAuthorizationExpired",
            Self::SessionAuthorizationNotYetValid => "SessionAuthorizationNotYetValid",
            Self::SessionKeyMismatch => "SessionKeyMismatch",
            Self::SettlementContractMismatch => "SettlementContractMismatch",
            Self::WalletMismatch => "WalletMismatch",
            Self::WrongChain => "WrongChain",
            Self::UnsupportedProtocolVersion(version) => {
                return f
                    .debug_tuple("UnsupportedProtocolVersion")
                    .field(version)
                    .finish()
            }
            Self::Serialization(_) => "Serialization([REDACTED])",
            Self::Signing(_) => "Signing([REDACTED])",
            Self::WalletSigning(_) => "WalletSigning([REDACTED])",
        };
        f.write_str(name)
    }
}

pub fn make_accounting_commitment(
    route_epoch: &[u8; 32],
    payer_session_public_key: &PublicKey,
) -> [u8; 32] {
    [
        ACCOUNTING_COMMITMENT_DOMAIN,
        &route_epoch[..],
        payer_session_public_key.as_slice(),
    ]
    .concat()
    .keccak256()
}

fn default_service_units() -> u64 {
    1
}

impl ServiceReceipt {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        route_epoch: [u8; 32],
        sequence: u64,
        service_kind: ServiceKind,
        provider_public_key: PublicKey,
        accounting_commitment: [u8; 32],
        payload_size: u64,
        service_rate: u64,
        byte_rate: u64,
    ) -> Self {
        Self::new_with_service_units(
            route_epoch,
            sequence,
            service_kind,
            provider_public_key,
            accounting_commitment,
            payload_size,
            1,
            service_rate,
            byte_rate,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_service_units(
        route_epoch: [u8; 32],
        sequence: u64,
        service_kind: ServiceKind,
        provider_public_key: PublicKey,
        accounting_commitment: [u8; 32],
        payload_size: u64,
        service_units: u64,
        service_rate: u64,
        byte_rate: u64,
    ) -> Self {
        let cumulative_charge_wei =
            Self::checked_charge_wei(service_rate, service_units, byte_rate, payload_size)
                .unwrap_or(u128::MAX);
        Self {
            protocol_version: SERVICE_RECEIPT_PROTOCOL_VERSION,
            route_epoch,
            sequence,
            service_kind,
            provider_public_key,
            accounting_commitment,
            payload_size,
            service_units,
            service_rate,
            byte_rate,
            cumulative_charge_wei,
        }
    }

    pub fn next_for_same_route(
        &self,
        sequence: u64,
        service_kind: ServiceKind,
        payload_size: u64,
        service_rate: u64,
        byte_rate: u64,
    ) -> Result<Self, ServiceReceiptError> {
        self.next_with_service_units(
            sequence,
            service_kind,
            payload_size,
            1,
            service_rate,
            byte_rate,
        )
    }

    pub fn next_with_service_units(
        &self,
        sequence: u64,
        service_kind: ServiceKind,
        payload_size: u64,
        service_units: u64,
        service_rate: u64,
        byte_rate: u64,
    ) -> Result<Self, ServiceReceiptError> {
        self.validate()?;
        if sequence <= self.sequence {
            return Err(ServiceReceiptError::NonMonotonicSequence);
        }
        if service_units == 0 {
            return Err(ServiceReceiptError::EmptyServiceUnits);
        }
        let current_charge =
            Self::checked_charge_wei(service_rate, service_units, byte_rate, payload_size)?;
        let cumulative_charge_wei = self
            .cumulative_charge_wei
            .checked_add(current_charge)
            .ok_or(ServiceReceiptError::CumulativeChargeOverflow)?;
        Ok(Self {
            protocol_version: SERVICE_RECEIPT_PROTOCOL_VERSION,
            route_epoch: self.route_epoch,
            sequence,
            service_kind,
            provider_public_key: self.provider_public_key.clone(),
            accounting_commitment: self.accounting_commitment,
            payload_size,
            service_units,
            service_rate,
            byte_rate,
            cumulative_charge_wei,
        })
    }

    pub fn total_charge_wei(&self) -> u128 {
        self.checked_total_charge_wei().unwrap_or(u128::MAX)
    }

    fn checked_total_charge_wei(&self) -> Result<u128, ServiceReceiptError> {
        Self::checked_charge_wei(
            self.service_rate,
            self.service_units,
            self.byte_rate,
            self.payload_size,
        )
    }

    fn checked_charge_wei(
        service_rate: u64,
        service_units: u64,
        byte_rate: u64,
        payload_size: u64,
    ) -> Result<u128, ServiceReceiptError> {
        u128::from(service_rate)
            .checked_mul(u128::from(service_units))
            .and_then(|service_charge| {
                u128::from(byte_rate)
                    .checked_mul(u128::from(payload_size))
                    .and_then(|byte_charge| service_charge.checked_add(byte_charge))
            })
            .ok_or(ServiceReceiptError::CumulativeChargeOverflow)
    }

    pub fn sign(self, cryptde: &dyn CryptDE) -> Result<SignedServiceReceipt, ServiceReceiptError> {
        self.validate()?;
        if cryptde.public_key() != &self.provider_public_key {
            return Err(ServiceReceiptError::ProviderPublicKeyMismatch);
        }
        let provider_signature = cryptde
            .sign(&self.signing_data()?)
            .map_err(ServiceReceiptError::Signing)?;
        Ok(SignedServiceReceipt {
            receipt: self,
            provider_signature,
        })
    }

    fn validate(&self) -> Result<(), ServiceReceiptError> {
        if self.protocol_version != SERVICE_RECEIPT_PROTOCOL_VERSION {
            return Err(ServiceReceiptError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        if self.route_epoch.iter().all(|byte| *byte == 0) {
            return Err(ServiceReceiptError::EmptyRouteEpoch);
        }
        if self.accounting_commitment.iter().all(|byte| *byte == 0) {
            return Err(ServiceReceiptError::EmptyAccountingCommitment);
        }
        if self.provider_public_key.is_empty() {
            return Err(ServiceReceiptError::EmptyProviderPublicKey);
        }
        if self.service_units == 0 {
            return Err(ServiceReceiptError::EmptyServiceUnits);
        }
        if self.cumulative_charge_wei < self.checked_total_charge_wei()? {
            return Err(ServiceReceiptError::CumulativeChargeMismatch);
        }
        Ok(())
    }

    fn signing_data(&self) -> Result<PlainData, ServiceReceiptError> {
        let serialized = serde_cbor::to_vec(self)
            .map_err(|error| ServiceReceiptError::Serialization(error.to_string()))?;
        let mut domain_separated =
            Vec::with_capacity(SERVICE_RECEIPT_SIGNING_DOMAIN.len() + serialized.len());
        domain_separated.extend_from_slice(SERVICE_RECEIPT_SIGNING_DOMAIN);
        domain_separated.extend_from_slice(&serialized);
        Ok(PlainData::from(domain_separated))
    }
}

impl ReceiptSessionPolicy {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chain_id: u64,
        settlement_contract: Address,
        payer_wallet_address: Address,
        payer_session_public_key: PublicKey,
        max_total_charge_wei: u128,
        valid_from_unix_s: u64,
        expires_at_unix_s: u64,
        authorization_nonce: [u8; 32],
    ) -> Self {
        Self {
            protocol_version: SERVICE_RECEIPT_PROTOCOL_VERSION,
            chain_id,
            settlement_contract,
            payer_wallet_address,
            payer_session_public_key,
            max_total_charge_wei,
            valid_from_unix_s,
            expires_at_unix_s,
            authorization_nonce,
        }
    }

    pub fn authorize(
        self,
        payer_wallet: &Wallet,
    ) -> Result<AuthorizedReceiptSession, ServiceReceiptError> {
        self.validate_structure()?;
        if payer_wallet.address_opt() != Some(self.payer_wallet_address) {
            return Err(ServiceReceiptError::WalletMismatch);
        }
        let wallet_signature = payer_wallet
            .sign(&self.authorization_digest()?)
            .map_err(|error| ServiceReceiptError::WalletSigning(error.to_string()))?;
        Ok(AuthorizedReceiptSession {
            policy: self,
            wallet_signature,
        })
    }

    fn validate_structure(&self) -> Result<(), ServiceReceiptError> {
        if self.protocol_version != SERVICE_RECEIPT_PROTOCOL_VERSION {
            return Err(ServiceReceiptError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        if self.payer_wallet_address == Address::zero() {
            return Err(ServiceReceiptError::EmptyPayerWallet);
        }
        if self.chain_id == 0 {
            return Err(ServiceReceiptError::InvalidChain);
        }
        if self.settlement_contract == Address::zero() {
            return Err(ServiceReceiptError::EmptySettlementContract);
        }
        if self.payer_session_public_key.is_empty() {
            return Err(ServiceReceiptError::EmptyPayerSessionPublicKey);
        }
        if self.max_total_charge_wei == 0 {
            return Err(ServiceReceiptError::EmptyAmountLimit);
        }
        if self.max_total_charge_wei > MAX_RECEIPT_SESSION_CHARGE_WEI {
            return Err(ServiceReceiptError::AmountLimitExceeded);
        }
        if self.valid_from_unix_s >= self.expires_at_unix_s {
            return Err(ServiceReceiptError::InvalidAuthorizationWindow);
        }
        let duration_seconds = self.expires_at_unix_s - self.valid_from_unix_s;
        if !(MIN_RECEIPT_SESSION_DURATION_SECONDS..=MAX_RECEIPT_SESSION_DURATION_SECONDS)
            .contains(&duration_seconds)
        {
            return Err(ServiceReceiptError::InvalidAuthorizationWindow);
        }
        if self.authorization_nonce.iter().all(|byte| *byte == 0) {
            return Err(ServiceReceiptError::EmptyAuthorizationNonce);
        }
        Ok(())
    }

    /// EIP-712 digest used by both the wallet and the Node verifier. Keeping this public lets the
    /// desktop wallet boundary compare the typed-data request with exactly what MASQNode verifies.
    pub fn eip712_digest(&self) -> Result<[u8; 32], ServiceReceiptError> {
        self.validate_structure()?;
        let domain_separator = [
            EIP712_DOMAIN_TYPE.keccak256().as_slice(),
            EIP712_NAME.keccak256().as_slice(),
            EIP712_VERSION.keccak256().as_slice(),
            &Self::uint_word(u128::from(self.chain_id)),
            &Self::address_word(self.settlement_contract),
        ]
        .concat()
        .keccak256();
        let session_key_hash = self.payer_session_public_key.as_slice().keccak256();
        let struct_hash = [
            SESSION_AUTHORIZATION_TYPE.keccak256().as_slice(),
            &Self::uint_word(u128::from(self.protocol_version)),
            &Self::address_word(self.payer_wallet_address),
            session_key_hash.as_slice(),
            &Self::uint_word(self.max_total_charge_wei),
            &Self::uint_word(u128::from(self.valid_from_unix_s)),
            &Self::uint_word(u128::from(self.expires_at_unix_s)),
            &self.authorization_nonce,
        ]
        .concat()
        .keccak256();
        Ok([b"\x19\x01".as_slice(), &domain_separator, &struct_hash]
            .concat()
            .keccak256())
    }

    /// JSON accepted by `eth_signTypedData_v4`. Integers are decimal strings to avoid JavaScript
    /// precision loss; bytes and addresses are canonical 0x-prefixed hex.
    pub fn eip712_typed_data(&self) -> Result<Value, ServiceReceiptError> {
        self.validate_structure()?;
        Ok(json!({
            "types": {
                "EIP712Domain": [
                    {"name": "name", "type": "string"},
                    {"name": "version", "type": "string"},
                    {"name": "chainId", "type": "uint256"},
                    {"name": "verifyingContract", "type": "address"}
                ],
                "ReceiptSessionAuthorization": [
                    {"name": "protocolVersion", "type": "uint16"},
                    {"name": "payerWallet", "type": "address"},
                    {"name": "payerSessionPublicKey", "type": "bytes"},
                    {"name": "maxTotalChargeWei", "type": "uint256"},
                    {"name": "validFromUnixS", "type": "uint64"},
                    {"name": "expiresAtUnixS", "type": "uint64"},
                    {"name": "authorizationNonce", "type": "bytes32"}
                ]
            },
            "primaryType": "ReceiptSessionAuthorization",
            "domain": {
                "name": "MASQ Service Receipt",
                "version": "1",
                "chainId": self.chain_id.to_string(),
                "verifyingContract": format!("{:#x}", self.settlement_contract)
            },
            "message": {
                "protocolVersion": self.protocol_version.to_string(),
                "payerWallet": format!("{:#x}", self.payer_wallet_address),
                "payerSessionPublicKey": format!(
                    "0x{}",
                    self.payer_session_public_key.as_slice().to_hex::<String>()
                ),
                "maxTotalChargeWei": self.max_total_charge_wei.to_string(),
                "validFromUnixS": self.valid_from_unix_s.to_string(),
                "expiresAtUnixS": self.expires_at_unix_s.to_string(),
                "authorizationNonce": format!(
                    "0x{}",
                    self.authorization_nonce.as_ref().to_hex::<String>()
                )
            }
        }))
    }

    fn authorization_digest(&self) -> Result<[u8; 32], ServiceReceiptError> {
        self.eip712_digest()
    }

    fn uint_word(value: u128) -> [u8; 32] {
        let mut word = [0u8; 32];
        word[16..].copy_from_slice(&value.to_be_bytes());
        word
    }

    fn address_word(address: Address) -> [u8; 32] {
        let mut word = [0u8; 32];
        word[12..].copy_from_slice(address.as_bytes());
        word
    }
}

impl ProviderSettlementPolicy {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chain_id: u64,
        settlement_contract: Address,
        payout_wallet_address: Address,
        provider_public_key: PublicKey,
        valid_from_unix_s: u64,
        expires_at_unix_s: u64,
        authorization_nonce: [u8; 32],
    ) -> Self {
        Self {
            protocol_version: SERVICE_RECEIPT_PROTOCOL_VERSION,
            chain_id,
            settlement_contract,
            payout_wallet_address,
            provider_public_key,
            valid_from_unix_s,
            expires_at_unix_s,
            authorization_nonce,
        }
    }

    pub fn authorize(
        self,
        payout_wallet: &Wallet,
        provider_cryptde: &dyn CryptDE,
    ) -> Result<AuthorizedProviderSettlement, ServiceReceiptError> {
        self.validate_structure()?;
        if payout_wallet.address_opt() != Some(self.payout_wallet_address) {
            return Err(ServiceReceiptError::WalletMismatch);
        }
        if provider_cryptde.public_key() != &self.provider_public_key {
            return Err(ServiceReceiptError::ProviderPublicKeyMismatch);
        }
        let payout_wallet_signature = payout_wallet
            .sign(&self.eip712_digest()?)
            .map_err(|error| ServiceReceiptError::WalletSigning(error.to_string()))?;
        self.authorize_with_wallet_signature(payout_wallet_signature, provider_cryptde)
    }

    /// Completes a hardware/browser-wallet flow without exposing the payout private key to the
    /// Node. The provider transport key co-signs only after the external EIP-712 signature has
    /// been recovered, matched to the configured payout address and checked for canonical low-s.
    pub fn authorize_with_wallet_signature(
        self,
        payout_wallet_signature: Signature,
        provider_cryptde: &dyn CryptDE,
    ) -> Result<AuthorizedProviderSettlement, ServiceReceiptError> {
        self.validate_structure()?;
        if provider_cryptde.public_key() != &self.provider_public_key {
            return Err(ServiceReceiptError::ProviderPublicKeyMismatch);
        }
        self.verify_payout_wallet_signature(&payout_wallet_signature)?;
        let provider_signature = provider_cryptde
            .sign(&Self::transport_signing_data(
                &self,
                &payout_wallet_signature,
            )?)
            .map_err(ServiceReceiptError::Signing)?;
        Ok(AuthorizedProviderSettlement {
            policy: self,
            payout_wallet_signature,
            provider_signature,
        })
    }

    fn verify_payout_wallet_signature(
        &self,
        payout_wallet_signature: &Signature,
    ) -> Result<(), ServiceReceiptError> {
        if payout_wallet_signature.v > 1
            || payout_wallet_signature.r.iter().all(|byte| *byte == 0)
            || payout_wallet_signature.s.iter().all(|byte| *byte == 0)
            || payout_wallet_signature.s > SECP256K1_HALF_ORDER
        {
            return Err(ServiceReceiptError::InvalidWalletSignature);
        }
        let digest = self.eip712_digest()?;
        let payout_wallet_public_key = payout_wallet_signature
            .recover(&digest)
            .map_err(|_| ServiceReceiptError::InvalidWalletSignature)?;
        let signature_valid = payout_wallet_public_key
            .verify(payout_wallet_signature, &digest)
            .unwrap_or(false);
        if !signature_valid
            || Address::from(*payout_wallet_public_key.address()) != self.payout_wallet_address
        {
            return Err(ServiceReceiptError::InvalidWalletSignature);
        }
        Ok(())
    }

    fn validate_structure(&self) -> Result<(), ServiceReceiptError> {
        if self.protocol_version != SERVICE_RECEIPT_PROTOCOL_VERSION {
            return Err(ServiceReceiptError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        if self.payout_wallet_address == Address::zero() {
            return Err(ServiceReceiptError::EmptyProviderPayoutWallet);
        }
        if self.chain_id == 0 {
            return Err(ServiceReceiptError::InvalidChain);
        }
        if self.settlement_contract == Address::zero() {
            return Err(ServiceReceiptError::EmptySettlementContract);
        }
        if self.provider_public_key.is_empty() {
            return Err(ServiceReceiptError::EmptyProviderPublicKey);
        }
        if self.valid_from_unix_s >= self.expires_at_unix_s {
            return Err(ServiceReceiptError::InvalidAuthorizationWindow);
        }
        let duration_seconds = self.expires_at_unix_s - self.valid_from_unix_s;
        if !(MIN_PROVIDER_SETTLEMENT_DURATION_SECONDS..=MAX_PROVIDER_SETTLEMENT_DURATION_SECONDS)
            .contains(&duration_seconds)
        {
            return Err(ServiceReceiptError::InvalidAuthorizationWindow);
        }
        if self.authorization_nonce.iter().all(|byte| *byte == 0) {
            return Err(ServiceReceiptError::EmptyAuthorizationNonce);
        }
        Ok(())
    }

    /// Wallet-readable authorization for `eth_signTypedData_v4` implementations.
    pub fn eip712_digest(&self) -> Result<[u8; 32], ServiceReceiptError> {
        self.validate_structure()?;
        let domain_separator = [
            EIP712_DOMAIN_TYPE.keccak256().as_slice(),
            PROVIDER_SETTLEMENT_EIP712_NAME.keccak256().as_slice(),
            EIP712_VERSION.keccak256().as_slice(),
            &ReceiptSessionPolicy::uint_word(u128::from(self.chain_id)),
            &ReceiptSessionPolicy::address_word(self.settlement_contract),
        ]
        .concat()
        .keccak256();
        let provider_key_hash = self.provider_public_key.as_slice().keccak256();
        let struct_hash = [
            PROVIDER_SETTLEMENT_AUTHORIZATION_TYPE
                .keccak256()
                .as_slice(),
            &ReceiptSessionPolicy::uint_word(u128::from(self.protocol_version)),
            &ReceiptSessionPolicy::address_word(self.payout_wallet_address),
            provider_key_hash.as_slice(),
            &ReceiptSessionPolicy::uint_word(u128::from(self.valid_from_unix_s)),
            &ReceiptSessionPolicy::uint_word(u128::from(self.expires_at_unix_s)),
            &self.authorization_nonce,
        ]
        .concat()
        .keccak256();
        Ok([b"\x19\x01".as_slice(), &domain_separator, &struct_hash]
            .concat()
            .keccak256())
    }

    pub fn eip712_typed_data(&self) -> Result<Value, ServiceReceiptError> {
        self.validate_structure()?;
        Ok(json!({
            "types": {
                "EIP712Domain": [
                    {"name": "name", "type": "string"},
                    {"name": "version", "type": "string"},
                    {"name": "chainId", "type": "uint256"},
                    {"name": "verifyingContract", "type": "address"}
                ],
                "ProviderSettlementAuthorization": [
                    {"name": "protocolVersion", "type": "uint16"},
                    {"name": "payoutWallet", "type": "address"},
                    {"name": "providerPublicKey", "type": "bytes"},
                    {"name": "validFromUnixS", "type": "uint64"},
                    {"name": "expiresAtUnixS", "type": "uint64"},
                    {"name": "authorizationNonce", "type": "bytes32"}
                ]
            },
            "primaryType": "ProviderSettlementAuthorization",
            "domain": {
                "name": "MASQ Provider Settlement",
                "version": "1",
                "chainId": self.chain_id.to_string(),
                "verifyingContract": format!("{:#x}", self.settlement_contract)
            },
            "message": {
                "protocolVersion": self.protocol_version.to_string(),
                "payoutWallet": format!("{:#x}", self.payout_wallet_address),
                "providerPublicKey": format!(
                    "0x{}",
                    self.provider_public_key.as_slice().to_hex::<String>()
                ),
                "validFromUnixS": self.valid_from_unix_s.to_string(),
                "expiresAtUnixS": self.expires_at_unix_s.to_string(),
                "authorizationNonce": format!(
                    "0x{}",
                    self.authorization_nonce.as_ref().to_hex::<String>()
                )
            }
        }))
    }

    fn transport_signing_data(
        policy: &Self,
        payout_wallet_signature: &Signature,
    ) -> Result<PlainData, ServiceReceiptError> {
        let serialized = serde_cbor::to_vec(policy)
            .map_err(|error| ServiceReceiptError::Serialization(error.to_string()))?;
        let mut domain_separated =
            Vec::with_capacity(PROVIDER_SETTLEMENT_SIGNING_DOMAIN.len() + serialized.len() + 65);
        domain_separated.extend_from_slice(PROVIDER_SETTLEMENT_SIGNING_DOMAIN);
        domain_separated.extend_from_slice(&serialized);
        domain_separated.push(payout_wallet_signature.v);
        domain_separated.extend_from_slice(&payout_wallet_signature.r);
        domain_separated.extend_from_slice(&payout_wallet_signature.s);
        Ok(PlainData::from(domain_separated))
    }
}

impl AuthorizedProviderSettlement {
    #[allow(clippy::too_many_arguments)]
    pub fn verify(
        &self,
        expected_chain_id: u64,
        expected_settlement_contract: Address,
        expected_provider_public_key: &PublicKey,
        now_unix_s: u64,
        receipt_cryptde: &dyn CryptDE,
    ) -> Result<Address, ServiceReceiptError> {
        self.policy.validate_structure()?;
        if self.policy.chain_id != expected_chain_id {
            return Err(ServiceReceiptError::WrongChain);
        }
        if self.policy.settlement_contract != expected_settlement_contract {
            return Err(ServiceReceiptError::SettlementContractMismatch);
        }
        if &self.policy.provider_public_key != expected_provider_public_key {
            return Err(ServiceReceiptError::ProviderPublicKeyMismatch);
        }
        if now_unix_s < self.policy.valid_from_unix_s {
            return Err(ServiceReceiptError::ProviderSettlementAuthorizationNotYetValid);
        }
        if now_unix_s > self.policy.expires_at_unix_s {
            return Err(ServiceReceiptError::ProviderSettlementAuthorizationExpired);
        }
        self.policy
            .verify_payout_wallet_signature(&self.payout_wallet_signature)?;
        let signing_data = ProviderSettlementPolicy::transport_signing_data(
            &self.policy,
            &self.payout_wallet_signature,
        )?;
        if self.provider_signature.len() < self.policy.provider_public_key.len()
            || !receipt_cryptde.verify_signature(
                &signing_data,
                &self.provider_signature,
                &self.policy.provider_public_key,
            )
        {
            return Err(ServiceReceiptError::InvalidProviderSettlementSignature);
        }
        Ok(self.policy.payout_wallet_address)
    }
}

impl SignedServiceReceipt {
    pub fn verify(&self, cryptde: &dyn CryptDE) -> Result<u128, ServiceReceiptError> {
        self.receipt.validate()?;
        let signing_data = self.receipt.signing_data()?;
        if self.provider_signature.len() < self.receipt.provider_public_key.len()
            || !cryptde.verify_signature(
                &signing_data,
                &self.provider_signature,
                &self.receipt.provider_public_key,
            )
        {
            return Err(ServiceReceiptError::InvalidProviderSignature);
        }
        self.receipt.checked_total_charge_wei()
    }

    pub fn acknowledge(
        self,
        payer_cryptde: &dyn CryptDE,
    ) -> Result<AcknowledgedServiceReceipt, ServiceReceiptError> {
        self.verify(payer_cryptde)?;
        let payer_session_public_key = payer_cryptde.public_key().clone();
        if payer_session_public_key.is_empty() {
            return Err(ServiceReceiptError::EmptyPayerSessionPublicKey);
        }
        if make_accounting_commitment(&self.receipt.route_epoch, &payer_session_public_key)
            != self.receipt.accounting_commitment
        {
            return Err(ServiceReceiptError::AccountingCommitmentMismatch);
        }
        let payer_signature = payer_cryptde
            .sign(&self.acknowledgement_data(&payer_session_public_key)?)
            .map_err(ServiceReceiptError::Signing)?;
        Ok(AcknowledgedServiceReceipt {
            signed_receipt: self,
            payer_session_public_key,
            payer_signature,
        })
    }

    fn acknowledgement_data(
        &self,
        payer_session_public_key: &PublicKey,
    ) -> Result<PlainData, ServiceReceiptError> {
        let serialized = serde_cbor::to_vec(&(self, payer_session_public_key))
            .map_err(|error| ServiceReceiptError::Serialization(error.to_string()))?;
        let mut domain_separated =
            Vec::with_capacity(PAYER_ACKNOWLEDGEMENT_DOMAIN.len() + serialized.len());
        domain_separated.extend_from_slice(PAYER_ACKNOWLEDGEMENT_DOMAIN);
        domain_separated.extend_from_slice(&serialized);
        Ok(PlainData::from(domain_separated))
    }
}

impl AcknowledgedServiceReceipt {
    pub fn verify(&self, cryptde: &dyn CryptDE) -> Result<u128, ServiceReceiptError> {
        let total_charge = self.signed_receipt.verify(cryptde)?;
        if self.payer_session_public_key.is_empty() {
            return Err(ServiceReceiptError::EmptyPayerSessionPublicKey);
        }
        if make_accounting_commitment(
            &self.signed_receipt.receipt.route_epoch,
            &self.payer_session_public_key,
        ) != self.signed_receipt.receipt.accounting_commitment
        {
            return Err(ServiceReceiptError::AccountingCommitmentMismatch);
        }
        let acknowledgement_data = self
            .signed_receipt
            .acknowledgement_data(&self.payer_session_public_key)?;
        if self.payer_signature.len() < self.payer_session_public_key.len()
            || !cryptde.verify_signature(
                &acknowledgement_data,
                &self.payer_signature,
                &self.payer_session_public_key,
            )
        {
            return Err(ServiceReceiptError::InvalidPayerSignature);
        }
        Ok(total_charge)
    }
}

impl AuthorizedReceiptSession {
    #[allow(clippy::too_many_arguments)]
    pub fn verify(
        &self,
        expected_chain_id: u64,
        expected_settlement_contract: Address,
        payer_session_public_key: &PublicKey,
        now_unix_s: u64,
        cumulative_charge_wei: u128,
    ) -> Result<(), ServiceReceiptError> {
        self.policy.validate_structure()?;
        if self.policy.chain_id != expected_chain_id {
            return Err(ServiceReceiptError::WrongChain);
        }
        if self.policy.settlement_contract != expected_settlement_contract {
            return Err(ServiceReceiptError::SettlementContractMismatch);
        }
        if &self.policy.payer_session_public_key != payer_session_public_key {
            return Err(ServiceReceiptError::SessionKeyMismatch);
        }
        if now_unix_s < self.policy.valid_from_unix_s {
            return Err(ServiceReceiptError::SessionAuthorizationNotYetValid);
        }
        if now_unix_s > self.policy.expires_at_unix_s {
            return Err(ServiceReceiptError::SessionAuthorizationExpired);
        }
        if cumulative_charge_wei > self.policy.max_total_charge_wei {
            return Err(ServiceReceiptError::AmountLimitExceeded);
        }
        let digest = self.policy.authorization_digest()?;
        let payer_public_key = self
            .wallet_signature
            .recover(&digest)
            .map_err(|_| ServiceReceiptError::InvalidWalletSignature)?;
        let signature_valid = payer_public_key
            .verify(&self.wallet_signature, &digest)
            .unwrap_or(false);
        if !signature_valid
            || Address::from(*payer_public_key.address()) != self.policy.payer_wallet_address
        {
            return Err(ServiceReceiptError::InvalidWalletSignature);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn verify_for_receipt(
        &self,
        acknowledged_receipt: &AcknowledgedServiceReceipt,
        receipt_cryptde: &dyn CryptDE,
        expected_chain_id: u64,
        expected_settlement_contract: Address,
        now_unix_s: u64,
    ) -> Result<u128, ServiceReceiptError> {
        let receipt_charge = acknowledged_receipt.verify(receipt_cryptde)?;
        self.verify(
            expected_chain_id,
            expected_settlement_contract,
            &acknowledged_receipt.payer_session_public_key,
            now_unix_s,
            acknowledged_receipt
                .signed_receipt
                .receipt
                .cumulative_charge_wei,
        )?;
        Ok(receipt_charge)
    }
}

impl ReceiptSessionRequest {
    pub fn new(
        authorization: AuthorizedReceiptSession,
        route_epoch: [u8; 32],
    ) -> Result<Self, ServiceReceiptError> {
        if route_epoch.iter().all(|byte| *byte == 0) {
            return Err(ServiceReceiptError::EmptyRouteEpoch);
        }
        let accounting_commitment = make_accounting_commitment(
            &route_epoch,
            &authorization.policy.payer_session_public_key,
        );
        Ok(Self {
            authorization,
            route_epoch,
            accounting_commitment,
        })
    }

    pub fn verify(
        &self,
        expected_chain_id: u64,
        expected_settlement_contract: Address,
        now_unix_s: u64,
    ) -> Result<(), ServiceReceiptError> {
        if self.route_epoch.iter().all(|byte| *byte == 0) {
            return Err(ServiceReceiptError::EmptyRouteEpoch);
        }
        if make_accounting_commitment(
            &self.route_epoch,
            &self.authorization.policy.payer_session_public_key,
        ) != self.accounting_commitment
        {
            return Err(ServiceReceiptError::AccountingCommitmentMismatch);
        }
        self.authorization.verify(
            expected_chain_id,
            expected_settlement_contract,
            &self.authorization.policy.payer_session_public_key,
            now_unix_s,
            0,
        )
    }
}

impl ReceiptSequenceCheckpoint {
    pub fn begin(
        acknowledged_receipt: &AcknowledgedServiceReceipt,
        cryptde: &dyn CryptDE,
    ) -> Result<Self, ServiceReceiptError> {
        Self::begin_with_contiguity(acknowledged_receipt, cryptde, true)
    }

    /// Provider-side recovery accepts a later payer-acknowledged cumulative receipt when an
    /// earlier acknowledgement was lost in transit. Consumer-side observation must keep using
    /// `begin`, which requires the first cumulative amount to equal the visible current charge.
    pub fn begin_for_settlement(
        acknowledged_receipt: &AcknowledgedServiceReceipt,
        cryptde: &dyn CryptDE,
    ) -> Result<Self, ServiceReceiptError> {
        Self::begin_with_contiguity(acknowledged_receipt, cryptde, false)
    }

    fn begin_with_contiguity(
        acknowledged_receipt: &AcknowledgedServiceReceipt,
        cryptde: &dyn CryptDE,
        require_contiguous: bool,
    ) -> Result<Self, ServiceReceiptError> {
        let receipt_charge = acknowledged_receipt.verify(cryptde)?;
        let receipt = &acknowledged_receipt.signed_receipt.receipt;
        if (require_contiguous && receipt.cumulative_charge_wei != receipt_charge)
            || receipt.cumulative_charge_wei < receipt_charge
        {
            return Err(ServiceReceiptError::CumulativeChargeMismatch);
        }
        Ok(Self {
            route_epoch: receipt.route_epoch,
            provider_public_key: receipt.provider_public_key.clone(),
            accounting_commitment: receipt.accounting_commitment,
            payer_session_public_key: acknowledged_receipt.payer_session_public_key.clone(),
            last_sequence: receipt.sequence,
            cumulative_charge_wei: receipt.cumulative_charge_wei,
        })
    }

    pub fn advance(
        &mut self,
        acknowledged_receipt: &AcknowledgedServiceReceipt,
        cryptde: &dyn CryptDE,
    ) -> Result<u128, ServiceReceiptError> {
        self.advance_with_contiguity(acknowledged_receipt, cryptde, true)
    }

    /// Provider settlement may catch up to a newer cumulative payer acknowledgement. The
    /// current receipt's own derived charge remains a lower bound on the signed delta.
    pub fn advance_for_settlement(
        &mut self,
        acknowledged_receipt: &AcknowledgedServiceReceipt,
        cryptde: &dyn CryptDE,
    ) -> Result<u128, ServiceReceiptError> {
        self.advance_with_contiguity(acknowledged_receipt, cryptde, false)
    }

    fn advance_with_contiguity(
        &mut self,
        acknowledged_receipt: &AcknowledgedServiceReceipt,
        cryptde: &dyn CryptDE,
        require_contiguous: bool,
    ) -> Result<u128, ServiceReceiptError> {
        let receipt_charge = acknowledged_receipt.verify(cryptde)?;
        let receipt = &acknowledged_receipt.signed_receipt.receipt;
        if receipt.route_epoch != self.route_epoch {
            return Err(ServiceReceiptError::RouteEpochMismatch);
        }
        if receipt.provider_public_key != self.provider_public_key {
            return Err(ServiceReceiptError::ProviderPublicKeyMismatch);
        }
        if receipt.accounting_commitment != self.accounting_commitment {
            return Err(ServiceReceiptError::AccountingCommitmentMismatch);
        }
        if acknowledged_receipt.payer_session_public_key != self.payer_session_public_key {
            return Err(ServiceReceiptError::SessionKeyMismatch);
        }
        if receipt.sequence <= self.last_sequence {
            return Err(ServiceReceiptError::NonMonotonicSequence);
        }
        let expected_cumulative_charge = self
            .cumulative_charge_wei
            .checked_add(receipt_charge)
            .ok_or(ServiceReceiptError::CumulativeChargeOverflow)?;
        if (require_contiguous && receipt.cumulative_charge_wei != expected_cumulative_charge)
            || receipt.cumulative_charge_wei < expected_cumulative_charge
        {
            return Err(ServiceReceiptError::CumulativeChargeMismatch);
        }
        self.last_sequence = receipt.sequence;
        self.cumulative_charge_wei = receipt.cumulative_charge_wei;
        Ok(self.cumulative_charge_wei)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub_lib::cryptde_null::CryptDENull;
    use crate::test_utils::make_paying_wallet;
    use ethabi::Token;
    use ethereum_types::U256;
    use masq_lib::test_utils::utils::TEST_DEFAULT_CHAIN;

    fn make_subject() -> (ServiceReceipt, CryptDENull, CryptDENull) {
        let provider_public_key = PublicKey::new(b"provider public key");
        let provider_cryptde = CryptDENull::from(&provider_public_key, TEST_DEFAULT_CHAIN);
        let payer_public_key = PublicKey::new(b"payer session public key");
        let payer_cryptde = CryptDENull::from(&payer_public_key, TEST_DEFAULT_CHAIN);
        let route_epoch = [0x11; 32];
        let receipt = ServiceReceipt::new(
            route_epoch,
            7,
            ServiceKind::Routing,
            provider_public_key,
            make_accounting_commitment(&route_epoch, &payer_public_key),
            4_096,
            5_000,
            3,
        );
        (receipt, provider_cryptde, payer_cryptde)
    }

    fn make_session_policy(
        payer_wallet: &Wallet,
        payer_session_public_key: &PublicKey,
    ) -> ReceiptSessionPolicy {
        ReceiptSessionPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
            payer_wallet.address(),
            payer_session_public_key.clone(),
            1_000_000,
            100,
            200,
            [0x33; 32],
        )
    }

    fn make_provider_settlement_policy(
        payout_wallet: &Wallet,
        provider_public_key: &PublicKey,
    ) -> ProviderSettlementPolicy {
        ProviderSettlementPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
            payout_wallet.address(),
            provider_public_key.clone(),
            100,
            200,
            [0x44; 32],
        )
    }

    #[test]
    fn receipt_debug_redacts_wallet_route_financial_and_signature_material() {
        let (receipt, provider_cryptde, payer_cryptde) = make_subject();
        let signed_receipt = receipt.clone().sign(&provider_cryptde).unwrap();
        let acknowledged_receipt = signed_receipt.clone().acknowledge(&payer_cryptde).unwrap();
        let payer_wallet = make_paying_wallet(b"receipt debug payer wallet");
        let payer_session_public_key = PublicKey::new(b"receipt debug payer session");
        let receipt_authorization = make_session_policy(&payer_wallet, &payer_session_public_key)
            .authorize(&payer_wallet)
            .unwrap();
        let payout_wallet = make_paying_wallet(b"provider debug payout wallet");
        let provider_public_key = PublicKey::new(b"provider debug transport key");
        let provider_cryptde = CryptDENull::from(&provider_public_key, TEST_DEFAULT_CHAIN);
        let provider_authorization =
            make_provider_settlement_policy(&payout_wallet, &provider_public_key)
                .authorize(&payout_wallet, &provider_cryptde)
                .unwrap();

        assert_eq!(
            format!("{:?}", receipt),
            "ServiceReceipt { protocol_version: 1, service_kind: Routing, receipt_data: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", signed_receipt),
            "SignedServiceReceipt { receipt: [REDACTED], provider_signature: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", acknowledged_receipt),
            "AcknowledgedServiceReceipt { signed_receipt: [REDACTED], payer_session_public_key: [REDACTED], payer_signature: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", receipt_authorization.policy),
            "ReceiptSessionPolicy { authorization_policy: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", receipt_authorization),
            "AuthorizedReceiptSession { policy: [REDACTED], wallet_signature: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", provider_authorization.policy),
            "ProviderSettlementPolicy { authorization_policy: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", provider_authorization),
            "AuthorizedProviderSettlement { policy: [REDACTED], payout_wallet_signature: [REDACTED], provider_signature: [REDACTED] }"
        );
        let request = ReceiptSessionRequest {
            authorization: receipt_authorization.clone(),
            route_epoch: [0x51; 32],
            accounting_commitment: [0x52; 32],
        };
        let payload = ServiceReceiptPayload_0v1 {
            authorization: receipt_authorization,
            acknowledged_receipt,
        };
        let offer = ServiceReceiptOfferPayload_0v1 { signed_receipt };
        let checkpoint = ReceiptSequenceCheckpoint {
            route_epoch: [0x53; 32],
            provider_public_key,
            accounting_commitment: [0x54; 32],
            payer_session_public_key,
            last_sequence: 987_654,
            cumulative_charge_wei: 123_456_789,
        };

        assert_eq!(
            format!("{:?}", request),
            "ReceiptSessionRequest { authorization: [REDACTED], route_epoch: [REDACTED], accounting_commitment: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", payload),
            "ServiceReceiptPayload_0v1 { authorization: [REDACTED], acknowledged_receipt: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", offer),
            "ServiceReceiptOfferPayload_0v1 { signed_receipt: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", checkpoint),
            "ReceiptSequenceCheckpoint { checkpoint_data: [REDACTED] }"
        );
        assert_eq!(
            format!(
                "{:?}",
                ServiceReceiptError::Serialization("private receipt marker".to_string())
            ),
            "Serialization([REDACTED])"
        );
    }

    #[test]
    fn provider_settlement_policy_exposes_readable_eip712_data_and_matches_abi_encoding() {
        let payout_wallet = make_paying_wallet(b"provider settlement eip712 wallet");
        let provider_public_key = PublicKey::new(b"provider settlement eip712 transport");
        let policy = make_provider_settlement_policy(&payout_wallet, &provider_public_key);
        let typed_data = policy.eip712_typed_data().unwrap();

        assert_eq!(
            typed_data["primaryType"],
            Value::String("ProviderSettlementAuthorization".to_string())
        );
        assert_eq!(
            typed_data["message"]["payoutWallet"],
            Value::String(format!("{:#x}", payout_wallet.address()))
        );
        assert_eq!(
            typed_data["message"]["providerPublicKey"],
            Value::String(format!(
                "0x{}",
                provider_public_key.as_slice().to_hex::<String>()
            ))
        );

        let domain_separator = ethabi::encode(&[
            Token::FixedBytes(EIP712_DOMAIN_TYPE.keccak256().to_vec()),
            Token::FixedBytes(PROVIDER_SETTLEMENT_EIP712_NAME.keccak256().to_vec()),
            Token::FixedBytes(EIP712_VERSION.keccak256().to_vec()),
            Token::Uint(U256::from(policy.chain_id)),
            Token::Address(policy.settlement_contract),
        ])
        .keccak256();
        let struct_hash = ethabi::encode(&[
            Token::FixedBytes(PROVIDER_SETTLEMENT_AUTHORIZATION_TYPE.keccak256().to_vec()),
            Token::Uint(U256::from(policy.protocol_version)),
            Token::Address(policy.payout_wallet_address),
            Token::FixedBytes(policy.provider_public_key.as_slice().keccak256().to_vec()),
            Token::Uint(U256::from(policy.valid_from_unix_s)),
            Token::Uint(U256::from(policy.expires_at_unix_s)),
            Token::FixedBytes(policy.authorization_nonce.to_vec()),
        ])
        .keccak256();
        let independently_encoded = [b"\x19\x01".as_slice(), &domain_separator, &struct_hash]
            .concat()
            .keccak256();

        assert_eq!(policy.eip712_digest().unwrap(), independently_encoded);
    }

    #[test]
    fn provider_settlement_authorization_round_trips_and_verifies_both_signatures() {
        let payout_wallet = make_paying_wallet(b"provider settlement payout wallet");
        let provider_public_key = PublicKey::new(b"provider settlement transport key");
        let provider_cryptde = CryptDENull::from(&provider_public_key, TEST_DEFAULT_CHAIN);
        let authorized = make_provider_settlement_policy(&payout_wallet, &provider_public_key)
            .authorize(&payout_wallet, &provider_cryptde)
            .unwrap();
        let serialized = serde_cbor::to_vec(&authorized).unwrap();
        let restored: AuthorizedProviderSettlement = serde_cbor::from_slice(&serialized).unwrap();

        assert_eq!(
            restored.verify(
                TEST_DEFAULT_CHAIN.rec().num_chain_id,
                TEST_DEFAULT_CHAIN.rec().contract,
                &provider_public_key,
                150,
                &provider_cryptde,
            ),
            Ok(payout_wallet.address())
        );
        assert_eq!(restored, authorized);
    }

    #[test]
    fn provider_settlement_accepts_only_matching_canonical_external_wallet_signatures() {
        let payout_wallet = make_paying_wallet(b"external provider payout wallet");
        let unrelated_wallet = make_paying_wallet(b"unrelated external payout wallet");
        let provider_public_key = PublicKey::new(b"external payout provider transport");
        let provider_cryptde = CryptDENull::from(&provider_public_key, TEST_DEFAULT_CHAIN);
        let policy = make_provider_settlement_policy(&payout_wallet, &provider_public_key);
        let digest = policy.eip712_digest().unwrap();
        let signature = payout_wallet.sign(&digest).unwrap();

        let authorized = policy
            .clone()
            .authorize_with_wallet_signature(signature, &provider_cryptde)
            .unwrap();
        assert_eq!(
            authorized.verify(
                TEST_DEFAULT_CHAIN.rec().num_chain_id,
                TEST_DEFAULT_CHAIN.rec().contract,
                &provider_public_key,
                150,
                &provider_cryptde,
            ),
            Ok(payout_wallet.address())
        );

        let wrong_signature = unrelated_wallet.sign(&digest).unwrap();
        assert_eq!(
            policy
                .clone()
                .authorize_with_wallet_signature(wrong_signature, &provider_cryptde),
            Err(ServiceReceiptError::InvalidWalletSignature)
        );
        let mut noncanonical_signature = payout_wallet.sign(&digest).unwrap();
        noncanonical_signature.s = [0xff; 32];
        assert_eq!(
            policy.authorize_with_wallet_signature(noncanonical_signature, &provider_cryptde),
            Err(ServiceReceiptError::InvalidWalletSignature)
        );
    }

    #[test]
    fn provider_settlement_authorization_enforces_scope_time_and_transport_consent() {
        let payout_wallet = make_paying_wallet(b"bounded provider payout wallet");
        let provider_public_key = PublicKey::new(b"bounded provider transport key");
        let provider_cryptde = CryptDENull::from(&provider_public_key, TEST_DEFAULT_CHAIN);
        let mut authorized = make_provider_settlement_policy(&payout_wallet, &provider_public_key)
            .authorize(&payout_wallet, &provider_cryptde)
            .unwrap();
        let chain_id = TEST_DEFAULT_CHAIN.rec().num_chain_id;
        let contract = TEST_DEFAULT_CHAIN.rec().contract;

        assert_eq!(
            authorized.verify(
                chain_id + 1,
                contract,
                &provider_public_key,
                150,
                &provider_cryptde
            ),
            Err(ServiceReceiptError::WrongChain)
        );
        assert_eq!(
            authorized.verify(
                chain_id,
                Address::from_low_u64_be(123),
                &provider_public_key,
                150,
                &provider_cryptde,
            ),
            Err(ServiceReceiptError::SettlementContractMismatch)
        );
        assert_eq!(
            authorized.verify(
                chain_id,
                contract,
                &PublicKey::new(b"different provider transport"),
                150,
                &provider_cryptde,
            ),
            Err(ServiceReceiptError::ProviderPublicKeyMismatch)
        );
        assert_eq!(
            authorized.verify(
                chain_id,
                contract,
                &provider_public_key,
                99,
                &provider_cryptde
            ),
            Err(ServiceReceiptError::ProviderSettlementAuthorizationNotYetValid)
        );
        assert_eq!(
            authorized.verify(
                chain_id,
                contract,
                &provider_public_key,
                201,
                &provider_cryptde
            ),
            Err(ServiceReceiptError::ProviderSettlementAuthorizationExpired)
        );

        authorized.provider_signature = CryptData::new(b"tampered provider consent");
        assert_eq!(
            authorized.verify(
                chain_id,
                contract,
                &provider_public_key,
                150,
                &provider_cryptde
            ),
            Err(ServiceReceiptError::InvalidProviderSettlementSignature)
        );
    }

    #[test]
    fn provider_settlement_policy_rejects_identity_and_structure_mismatches() {
        let payout_wallet = make_paying_wallet(b"provider payout structure wallet");
        let other_wallet = make_paying_wallet(b"unrelated provider payout wallet");
        let provider_public_key = PublicKey::new(b"provider payout structure transport");
        let provider_cryptde = CryptDENull::from(&provider_public_key, TEST_DEFAULT_CHAIN);
        let policy = make_provider_settlement_policy(&payout_wallet, &provider_public_key);

        assert_eq!(
            policy.clone().authorize(&other_wallet, &provider_cryptde),
            Err(ServiceReceiptError::WalletMismatch)
        );
        let unrelated_provider = CryptDENull::from(
            &PublicKey::new(b"unrelated provider transport"),
            TEST_DEFAULT_CHAIN,
        );
        assert_eq!(
            policy
                .clone()
                .authorize(&payout_wallet, &unrelated_provider),
            Err(ServiceReceiptError::ProviderPublicKeyMismatch)
        );

        let mut invalid = policy;
        invalid.payout_wallet_address = Address::zero();
        assert_eq!(
            invalid.clone().authorize(&payout_wallet, &provider_cryptde),
            Err(ServiceReceiptError::EmptyProviderPayoutWallet)
        );
        invalid.payout_wallet_address = payout_wallet.address();
        invalid.provider_public_key = PublicKey::new(&[]);
        assert_eq!(
            invalid.clone().authorize(&payout_wallet, &provider_cryptde),
            Err(ServiceReceiptError::EmptyProviderPublicKey)
        );
        invalid.provider_public_key = provider_public_key;
        invalid.valid_from_unix_s = 100;
        invalid.expires_at_unix_s = 100 + MIN_PROVIDER_SETTLEMENT_DURATION_SECONDS - 1;
        assert_eq!(
            invalid.clone().authorize(&payout_wallet, &provider_cryptde),
            Err(ServiceReceiptError::InvalidAuthorizationWindow)
        );
        invalid.expires_at_unix_s = 100 + MAX_PROVIDER_SETTLEMENT_DURATION_SECONDS + 1;
        assert_eq!(
            invalid.authorize(&payout_wallet, &provider_cryptde),
            Err(ServiceReceiptError::InvalidAuthorizationWindow)
        );
    }

    #[test]
    fn session_policy_exposes_wallet_readable_eip712_data_and_matches_abi_encoding() {
        let payer_wallet = make_paying_wallet(b"eip712 receipt wallet");
        let payer_session_public_key = PublicKey::new(b"eip712 payer session");
        let policy = make_session_policy(&payer_wallet, &payer_session_public_key);
        let typed_data = policy.eip712_typed_data().unwrap();

        assert_eq!(
            typed_data["primaryType"],
            Value::String("ReceiptSessionAuthorization".to_string())
        );
        assert_eq!(
            typed_data["domain"]["chainId"],
            Value::String(policy.chain_id.to_string())
        );
        assert_eq!(
            typed_data["message"]["maxTotalChargeWei"],
            Value::String(policy.max_total_charge_wei.to_string())
        );
        assert_eq!(
            typed_data["message"]["payerSessionPublicKey"],
            Value::String(format!(
                "0x{}",
                payer_session_public_key.as_slice().to_hex::<String>()
            ))
        );

        let domain_separator = ethabi::encode(&[
            Token::FixedBytes(EIP712_DOMAIN_TYPE.keccak256().to_vec()),
            Token::FixedBytes(EIP712_NAME.keccak256().to_vec()),
            Token::FixedBytes(EIP712_VERSION.keccak256().to_vec()),
            Token::Uint(U256::from(policy.chain_id)),
            Token::Address(policy.settlement_contract),
        ])
        .keccak256();
        let struct_hash = ethabi::encode(&[
            Token::FixedBytes(SESSION_AUTHORIZATION_TYPE.keccak256().to_vec()),
            Token::Uint(U256::from(policy.protocol_version)),
            Token::Address(policy.payer_wallet_address),
            Token::FixedBytes(
                policy
                    .payer_session_public_key
                    .as_slice()
                    .keccak256()
                    .to_vec(),
            ),
            Token::Uint(U256::from(policy.max_total_charge_wei)),
            Token::Uint(U256::from(policy.valid_from_unix_s)),
            Token::Uint(U256::from(policy.expires_at_unix_s)),
            Token::FixedBytes(policy.authorization_nonce.to_vec()),
        ])
        .keccak256();
        let independently_encoded = [b"\x19\x01".as_slice(), &domain_separator, &struct_hash]
            .concat()
            .keccak256();

        assert_eq!(policy.eip712_digest().unwrap(), independently_encoded);
    }

    #[test]
    fn signed_receipt_round_trips_and_verifies() {
        let (receipt, cryptde, _) = make_subject();
        let expected_charge = receipt.total_charge_wei();
        let signed = receipt.sign(&cryptde).unwrap();

        let serialized = serde_cbor::to_vec(&signed).unwrap();
        let deserialized: SignedServiceReceipt = serde_cbor::from_slice(&serialized).unwrap();

        assert_eq!(deserialized.verify(&cryptde), Ok(expected_charge));
        assert_eq!(deserialized, signed);
    }

    #[test]
    fn receipt_charge_uses_exact_u128_arithmetic_at_u64_limits() {
        let (mut receipt, _, _) = make_subject();
        receipt.payload_size = u64::MAX;
        receipt.service_rate = u64::MAX;
        receipt.byte_rate = u64::MAX;

        assert_eq!(
            receipt.total_charge_wei(),
            u128::from(u64::MAX) * (u128::from(u64::MAX) + 1)
        );
    }

    #[test]
    fn receipt_charge_overflow_fails_closed_before_signature_verification() {
        let (receipt, cryptde, _) = make_subject();
        let overflowing = ServiceReceipt::new_with_service_units(
            receipt.route_epoch,
            receipt.sequence,
            receipt.service_kind,
            receipt.provider_public_key,
            receipt.accounting_commitment,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
        );

        assert_eq!(overflowing.cumulative_charge_wei, u128::MAX);
        assert_eq!(overflowing.total_charge_wei(), u128::MAX);
        assert_eq!(
            overflowing.clone().sign(&cryptde),
            Err(ServiceReceiptError::CumulativeChargeOverflow)
        );
        assert_eq!(
            SignedServiceReceipt {
                receipt: overflowing,
                provider_signature: CryptData::new(&[0xAA]),
            }
            .verify(&cryptde),
            Err(ServiceReceiptError::CumulativeChargeOverflow)
        );
    }

    #[test]
    fn aggregate_service_units_are_signed_and_charged_exactly() {
        let (receipt, provider_cryptde, _) = make_subject();
        let aggregate = ServiceReceipt::new_with_service_units(
            receipt.route_epoch,
            receipt.sequence,
            ServiceKind::Exit,
            receipt.provider_public_key.clone(),
            receipt.accounting_commitment,
            700,
            3,
            500,
            2,
        );

        assert_eq!(aggregate.total_charge_wei(), 2_900);
        assert_eq!(
            aggregate
                .clone()
                .sign(&provider_cryptde)
                .unwrap()
                .verify(&provider_cryptde),
            Ok(2_900)
        );

        let mut invalid = aggregate;
        invalid.service_units = 0;
        assert_eq!(
            invalid.sign(&provider_cryptde),
            Err(ServiceReceiptError::EmptyServiceUnits)
        );
        assert_eq!(
            receipt.next_with_service_units(8, ServiceKind::Exit, 1, 0, 1, 1),
            Err(ServiceReceiptError::EmptyServiceUnits)
        );
    }

    #[test]
    fn legacy_receipt_without_service_units_deserializes_as_one_unit() {
        #[derive(serde_derive::Serialize)]
        struct LegacyServiceReceipt<'a> {
            protocol_version: u16,
            route_epoch: [u8; 32],
            sequence: u64,
            service_kind: ServiceKind,
            provider_public_key: &'a PublicKey,
            accounting_commitment: [u8; 32],
            payload_size: u64,
            service_rate: u64,
            byte_rate: u64,
            #[serde(with = "u128_be")]
            cumulative_charge_wei: u128,
        }

        let (receipt, _, _) = make_subject();
        let serialized = serde_cbor::to_vec(&LegacyServiceReceipt {
            protocol_version: receipt.protocol_version,
            route_epoch: receipt.route_epoch,
            sequence: receipt.sequence,
            service_kind: receipt.service_kind,
            provider_public_key: &receipt.provider_public_key,
            accounting_commitment: receipt.accounting_commitment,
            payload_size: receipt.payload_size,
            service_rate: receipt.service_rate,
            byte_rate: receipt.byte_rate,
            cumulative_charge_wei: receipt.cumulative_charge_wei,
        })
        .unwrap();
        let restored: ServiceReceipt = serde_cbor::from_slice(&serialized).unwrap();

        assert_eq!(restored.service_units, 1);
        assert_eq!(restored.total_charge_wei(), receipt.total_charge_wei());
    }

    #[test]
    fn maximum_u128_amounts_round_trip_through_canonical_cbor() {
        let (mut receipt, _, payer_cryptde) = make_subject();
        receipt.cumulative_charge_wei = u128::MAX;
        let receipt_bytes = serde_cbor::to_vec(&receipt).unwrap();
        assert_eq!(
            serde_cbor::from_slice::<ServiceReceipt>(&receipt_bytes).unwrap(),
            receipt
        );

        let payer_wallet = make_paying_wallet(b"maximum u128 session wallet");
        let mut policy = make_session_policy(&payer_wallet, payer_cryptde.public_key());
        policy.max_total_charge_wei = u128::MAX;
        let policy_bytes = serde_cbor::to_vec(&policy).unwrap();
        assert_eq!(
            serde_cbor::from_slice::<ReceiptSessionPolicy>(&policy_bytes).unwrap(),
            policy
        );

        let checkpoint = ReceiptSequenceCheckpoint {
            route_epoch: [0x41; 32],
            provider_public_key: PublicKey::new(b"maximum u128 provider"),
            accounting_commitment: [0x42; 32],
            payer_session_public_key: payer_cryptde.public_key().clone(),
            last_sequence: u64::MAX,
            cumulative_charge_wei: u128::MAX,
        };
        let checkpoint_bytes = serde_cbor::to_vec(&checkpoint).unwrap();
        assert_eq!(
            serde_cbor::from_slice::<ReceiptSequenceCheckpoint>(&checkpoint_bytes).unwrap(),
            checkpoint
        );
    }

    #[test]
    fn tampering_with_billable_data_invalidates_signature() {
        let (receipt, cryptde, _) = make_subject();
        let mut signed = receipt.sign(&cryptde).unwrap();
        signed.receipt.payload_size += 1;
        signed.receipt.cumulative_charge_wei += u128::from(signed.receipt.byte_rate);

        assert_eq!(
            signed.verify(&cryptde),
            Err(ServiceReceiptError::InvalidProviderSignature)
        );
    }

    #[test]
    fn truncated_provider_signature_is_rejected_without_panicking() {
        let (receipt, cryptde, _) = make_subject();
        let mut signed = receipt.sign(&cryptde).unwrap();
        signed.provider_signature = CryptData::new(b"truncated signature");

        assert_eq!(
            signed.verify(&cryptde),
            Err(ServiceReceiptError::InvalidProviderSignature)
        );
    }

    #[test]
    fn provider_key_must_match_signer() {
        let (mut receipt, cryptde, _) = make_subject();
        receipt.provider_public_key = PublicKey::new(b"different provider");

        assert_eq!(
            receipt.sign(&cryptde),
            Err(ServiceReceiptError::ProviderPublicKeyMismatch)
        );
    }

    #[test]
    fn empty_privacy_identifiers_are_rejected() {
        let (mut receipt, cryptde, _) = make_subject();
        receipt.route_epoch = [0; 32];
        assert_eq!(
            receipt.clone().sign(&cryptde),
            Err(ServiceReceiptError::EmptyRouteEpoch)
        );

        receipt.route_epoch = [0x11; 32];
        receipt.accounting_commitment = [0; 32];
        assert_eq!(
            receipt.sign(&cryptde),
            Err(ServiceReceiptError::EmptyAccountingCommitment)
        );
    }

    #[test]
    fn unsupported_version_is_rejected_before_signature_verification() {
        let (receipt, cryptde, _) = make_subject();
        let mut signed = receipt.sign(&cryptde).unwrap();
        signed.receipt.protocol_version += 1;

        assert_eq!(
            signed.verify(&cryptde),
            Err(ServiceReceiptError::UnsupportedProtocolVersion(2))
        );
    }

    #[test]
    fn payer_acknowledgement_round_trips_and_verifies() {
        let (receipt, provider_cryptde, payer_cryptde) = make_subject();
        let expected_charge = receipt.total_charge_wei();
        let acknowledged = receipt
            .sign(&provider_cryptde)
            .unwrap()
            .acknowledge(&payer_cryptde)
            .unwrap();

        let serialized = serde_cbor::to_vec(&acknowledged).unwrap();
        let deserialized: AcknowledgedServiceReceipt = serde_cbor::from_slice(&serialized).unwrap();

        assert_eq!(deserialized.verify(&provider_cryptde), Ok(expected_charge));
        assert_eq!(deserialized, acknowledged);
    }

    #[test]
    fn unrelated_payer_session_cannot_acknowledge_receipt() {
        let (receipt, provider_cryptde, _) = make_subject();
        let unrelated_key = PublicKey::new(b"unrelated payer session");
        let unrelated_payer = CryptDENull::from(&unrelated_key, TEST_DEFAULT_CHAIN);
        let signed = receipt.sign(&provider_cryptde).unwrap();

        assert_eq!(
            signed.acknowledge(&unrelated_payer),
            Err(ServiceReceiptError::AccountingCommitmentMismatch)
        );
    }

    #[test]
    fn tampering_with_payer_acknowledgement_invalidates_it() {
        let (receipt, provider_cryptde, payer_cryptde) = make_subject();
        let mut acknowledged = receipt
            .sign(&provider_cryptde)
            .unwrap()
            .acknowledge(&payer_cryptde)
            .unwrap();
        acknowledged.payer_signature = CryptData::new(b"tampered signature");

        assert_eq!(
            acknowledged.verify(&provider_cryptde),
            Err(ServiceReceiptError::InvalidPayerSignature)
        );
    }

    #[test]
    fn wallet_authorized_session_round_trips_and_accepts_bilateral_receipt() {
        let (receipt, provider_cryptde, payer_cryptde) = make_subject();
        let receipt_charge = receipt.total_charge_wei();
        let acknowledged = receipt
            .sign(&provider_cryptde)
            .unwrap()
            .acknowledge(&payer_cryptde)
            .unwrap();
        let payer_wallet = make_paying_wallet(b"receipt session wallet");
        let authorized = make_session_policy(&payer_wallet, payer_cryptde.public_key())
            .authorize(&payer_wallet)
            .unwrap();

        let serialized = serde_cbor::to_vec(&authorized).unwrap();
        let deserialized: AuthorizedReceiptSession = serde_cbor::from_slice(&serialized).unwrap();

        assert_eq!(
            deserialized.verify_for_receipt(
                &acknowledged,
                &provider_cryptde,
                TEST_DEFAULT_CHAIN.rec().num_chain_id,
                TEST_DEFAULT_CHAIN.rec().contract,
                150,
            ),
            Ok(receipt_charge)
        );
        assert_eq!(deserialized, authorized);
    }

    #[test]
    fn session_policy_rejects_wrong_wallet_and_tampering() {
        let (_, _, payer_cryptde) = make_subject();
        let payer_wallet = make_paying_wallet(b"authorized wallet");
        let other_wallet = make_paying_wallet(b"other wallet");
        let policy = make_session_policy(&payer_wallet, payer_cryptde.public_key());

        assert_eq!(
            policy.clone().authorize(&other_wallet),
            Err(ServiceReceiptError::WalletMismatch)
        );

        let mut authorized = policy.authorize(&payer_wallet).unwrap();
        authorized.policy.max_total_charge_wei += 1;
        assert_eq!(
            authorized.verify(
                TEST_DEFAULT_CHAIN.rec().num_chain_id,
                TEST_DEFAULT_CHAIN.rec().contract,
                payer_cryptde.public_key(),
                150,
                1,
            ),
            Err(ServiceReceiptError::InvalidWalletSignature)
        );
    }

    #[test]
    fn session_authorization_enforces_chain_contract_key_time_and_amount() {
        let (_, _, payer_cryptde) = make_subject();
        let payer_wallet = make_paying_wallet(b"bounded session wallet");
        let authorized = make_session_policy(&payer_wallet, payer_cryptde.public_key())
            .authorize(&payer_wallet)
            .unwrap();
        let chain_id = TEST_DEFAULT_CHAIN.rec().num_chain_id;
        let contract = TEST_DEFAULT_CHAIN.rec().contract;

        assert_eq!(
            authorized.verify(chain_id + 1, contract, payer_cryptde.public_key(), 150, 1),
            Err(ServiceReceiptError::WrongChain)
        );
        assert_eq!(
            authorized.verify(
                chain_id,
                Address::from_low_u64_be(123),
                payer_cryptde.public_key(),
                150,
                1,
            ),
            Err(ServiceReceiptError::SettlementContractMismatch)
        );
        assert_eq!(
            authorized.verify(
                chain_id,
                contract,
                &PublicKey::new(b"wrong session"),
                150,
                1,
            ),
            Err(ServiceReceiptError::SessionKeyMismatch)
        );
        assert_eq!(
            authorized.verify(chain_id, contract, payer_cryptde.public_key(), 99, 1),
            Err(ServiceReceiptError::SessionAuthorizationNotYetValid)
        );
        assert_eq!(
            authorized.verify(chain_id, contract, payer_cryptde.public_key(), 201, 1),
            Err(ServiceReceiptError::SessionAuthorizationExpired)
        );
        assert_eq!(
            authorized.verify(
                chain_id,
                contract,
                payer_cryptde.public_key(),
                150,
                1_000_001,
            ),
            Err(ServiceReceiptError::AmountLimitExceeded)
        );
    }

    #[test]
    fn structurally_invalid_session_policies_are_rejected() {
        let (_, _, payer_cryptde) = make_subject();
        let payer_wallet = make_paying_wallet(b"policy structure wallet");
        let mut policy = make_session_policy(&payer_wallet, payer_cryptde.public_key());
        policy.authorization_nonce = [0; 32];
        assert_eq!(
            policy.clone().authorize(&payer_wallet),
            Err(ServiceReceiptError::EmptyAuthorizationNonce)
        );

        policy.authorization_nonce = [0x33; 32];
        policy.valid_from_unix_s = policy.expires_at_unix_s;
        assert_eq!(
            policy.clone().authorize(&payer_wallet),
            Err(ServiceReceiptError::InvalidAuthorizationWindow)
        );

        policy.valid_from_unix_s = 100;
        policy.chain_id = 0;
        assert_eq!(
            policy.clone().authorize(&payer_wallet),
            Err(ServiceReceiptError::InvalidChain)
        );

        policy.chain_id = TEST_DEFAULT_CHAIN.rec().num_chain_id;
        policy.settlement_contract = Address::zero();
        assert_eq!(
            policy.clone().authorize(&payer_wallet),
            Err(ServiceReceiptError::EmptySettlementContract)
        );

        policy.settlement_contract = TEST_DEFAULT_CHAIN.rec().contract;
        policy.max_total_charge_wei = 0;
        assert_eq!(
            policy.clone().authorize(&payer_wallet),
            Err(ServiceReceiptError::EmptyAmountLimit)
        );
        policy.max_total_charge_wei = MAX_RECEIPT_SESSION_CHARGE_WEI + 1;
        assert_eq!(
            policy.clone().authorize(&payer_wallet),
            Err(ServiceReceiptError::AmountLimitExceeded)
        );
        policy.max_total_charge_wei = 1;
        policy.valid_from_unix_s = 100;
        policy.expires_at_unix_s = 100 + MIN_RECEIPT_SESSION_DURATION_SECONDS - 1;
        assert_eq!(
            policy.clone().authorize(&payer_wallet),
            Err(ServiceReceiptError::InvalidAuthorizationWindow)
        );
        policy.expires_at_unix_s = 100 + MAX_RECEIPT_SESSION_DURATION_SECONDS + 1;
        assert_eq!(
            policy.authorize(&payer_wallet),
            Err(ServiceReceiptError::InvalidAuthorizationWindow)
        );
    }

    #[test]
    fn sequence_checkpoint_accepts_exact_progress_and_rejects_replay_or_amount_jump() {
        let (receipt, provider_cryptde, payer_cryptde) = make_subject();
        let next_receipt = receipt
            .next_for_same_route(8, ServiceKind::Exit, 2_048, 1_000, 2)
            .unwrap();
        let expected_cumulative = next_receipt.cumulative_charge_wei;
        let acknowledged_initial = receipt
            .sign(&provider_cryptde)
            .unwrap()
            .acknowledge(&payer_cryptde)
            .unwrap();
        let acknowledged_next = next_receipt
            .clone()
            .sign(&provider_cryptde)
            .unwrap()
            .acknowledge(&payer_cryptde)
            .unwrap();
        let mut checkpoint =
            ReceiptSequenceCheckpoint::begin(&acknowledged_initial, &provider_cryptde).unwrap();

        assert_eq!(
            checkpoint.advance(&acknowledged_next, &provider_cryptde),
            Ok(expected_cumulative)
        );
        let serialized_checkpoint = serde_cbor::to_vec(&checkpoint).unwrap();
        let restored_checkpoint: ReceiptSequenceCheckpoint =
            serde_cbor::from_slice(&serialized_checkpoint).unwrap();
        assert_eq!(restored_checkpoint, checkpoint);
        assert_eq!(
            checkpoint.advance(&acknowledged_next, &provider_cryptde),
            Err(ServiceReceiptError::NonMonotonicSequence)
        );

        let mut jumped_receipt = next_receipt
            .next_for_same_route(9, ServiceKind::Routing, 100, 50, 2)
            .unwrap();
        jumped_receipt.cumulative_charge_wei += 1;
        let acknowledged_jump = jumped_receipt
            .sign(&provider_cryptde)
            .unwrap()
            .acknowledge(&payer_cryptde)
            .unwrap();
        assert_eq!(
            checkpoint.advance(&acknowledged_jump, &provider_cryptde),
            Err(ServiceReceiptError::CumulativeChargeMismatch)
        );
    }

    #[test]
    fn provider_settlement_checkpoint_catches_up_without_weakening_consumer_contiguity() {
        let (receipt, provider_cryptde, payer_cryptde) = make_subject();
        let acknowledged_initial = receipt
            .clone()
            .sign(&provider_cryptde)
            .unwrap()
            .acknowledge(&payer_cryptde)
            .unwrap();
        let mut skipped_initial = receipt.clone();
        skipped_initial.sequence += 2;
        skipped_initial.cumulative_charge_wei += 123;
        let acknowledged_skipped_initial = skipped_initial
            .sign(&provider_cryptde)
            .unwrap()
            .acknowledge(&payer_cryptde)
            .unwrap();

        assert_eq!(
            ReceiptSequenceCheckpoint::begin(&acknowledged_skipped_initial, &provider_cryptde),
            Err(ServiceReceiptError::CumulativeChargeMismatch)
        );
        assert_eq!(
            ReceiptSequenceCheckpoint::begin_for_settlement(
                &acknowledged_skipped_initial,
                &provider_cryptde,
            )
            .unwrap()
            .cumulative_charge_wei,
            acknowledged_skipped_initial
                .signed_receipt
                .receipt
                .cumulative_charge_wei
        );

        let mut checkpoint =
            ReceiptSequenceCheckpoint::begin(&acknowledged_initial, &provider_cryptde).unwrap();
        let mut later = receipt
            .next_for_same_route(10, ServiceKind::Exit, 512, 100, 2)
            .unwrap();
        later.cumulative_charge_wei += 456;
        let expected_cumulative = later.cumulative_charge_wei;
        let acknowledged_later = later
            .sign(&provider_cryptde)
            .unwrap()
            .acknowledge(&payer_cryptde)
            .unwrap();

        assert_eq!(
            checkpoint.advance(&acknowledged_later, &provider_cryptde),
            Err(ServiceReceiptError::CumulativeChargeMismatch)
        );
        assert_eq!(
            checkpoint.advance_for_settlement(&acknowledged_later, &provider_cryptde),
            Ok(expected_cumulative)
        );
    }
}
