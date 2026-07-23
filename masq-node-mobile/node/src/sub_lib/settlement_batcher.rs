// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::sub_lib::cryptde_real::CryptDEReal;
use crate::sub_lib::receipt_settlement::{
    receipt_session_contract_id, ReceiptSettlementBatch, ReceiptSettlementContractClaim,
    ReceiptSettlementError,
};
use ethabi::{ParamType, Token};
use ethereum_types::{Address, H256, U256};
use ethsign_crypto::Keccak256;
use futures::Future;
use libsecp256k1::{recover, verify, Message, PublicKey, RecoveryId, Signature};
use masq_lib::blockchains::chains::Chain;
use rlp::{Rlp, RlpStream};
use rustc_hex::{FromHex, ToHex};
use serde_derive::{Deserialize, Serialize};
use serde_json::json;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use web3::transports::Http;
use web3::types::{BlockId, BlockNumber, Bytes, CallRequest, TransactionId, TransactionReceipt};
use web3::{Transport, Web3};

pub const MAX_BATCHER_CLAIMS: usize = 128;
pub const MAX_RPC_BLOCK_AGE_SECONDS: u64 = 300;
pub const MAX_RPC_BLOCK_FUTURE_DRIFT_SECONDS: u64 = 30;
pub const MIN_PROVIDER_AUTHORIZATION_REMAINING_SECONDS: u64 = 60;
pub const MIN_CONFIRMATION_DEPTH: u64 = 12;
pub const MAX_CONFIRMATION_DEPTH: u64 = 100_000;
pub const MAX_SIGNED_EIP1559_TRANSACTION_BYTES: usize = 64 * 1024;
const SUBMIT_BATCH_SIGNATURE: &[u8] =
    b"submitBatch(uint64,bytes32,(bytes32,bytes32,address,address,uint128)[])";
const SUBMIT_BATCH_WITH_SESSIONS_SIGNATURE: &[u8] = b"submitBatchWithSessions((uint16,address,bytes,uint256,uint64,uint64,bytes32)[],bytes[],uint64,bytes32,(bytes32,bytes32,address,address,uint128)[])";
const NEXT_BATCH_SEQUENCE_SIGNATURE: &[u8] = b"nextBatchSequence()";
const HAS_ROLE_SIGNATURE: &[u8] = b"hasRole(bytes32,address)";
const BATCHER_ROLE_NAME: &[u8] = b"BATCHER_ROLE";
const BATCH_SETTLED_EVENT_SIGNATURE: &[u8] = b"BatchSettled(uint64,bytes32,uint256,uint256)";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettlementSubmissionManifest {
    pub manifest_version: u16,
    pub assurance_mode: String,
    pub chain_id: u64,
    pub settlement_contract: String,
    pub verification_time_unix_s: u64,
    pub batch_sequence: u64,
    pub claim_count: usize,
    pub claim_ids: Vec<String>,
    pub portable_merkle_root: String,
    pub contract_merkle_root: String,
    pub total_cumulative_charge_wei: String,
    pub batch_cbor_keccak256: String,
    pub calldata_keccak256: String,
    pub value_wei: String,
    pub calldata: String,
    #[serde(rename = "rpcPreflight", skip_serializing_if = "Option::is_none")]
    pub rpc_preflight_opt: Option<SettlementRpcPreflight>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettlementRpcPreflight {
    pub observed_block_number: u64,
    pub observed_block_hash: String,
    pub observed_block_timestamp_unix_s: u64,
    pub settlement_contract_code_keccak256: String,
    pub batcher_address: String,
    pub provider_authorization_remaining_seconds: u64,
    pub batcher_role_authorized: bool,
    pub submit_batch_simulated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettlementTransactionObservation {
    pub lifecycle_state: String,
    pub chain_id: u64,
    pub settlement_contract: String,
    pub transaction_hash: String,
    pub batcher_address: String,
    pub batch_sequence: u64,
    pub required_confirmation_depth: u64,
    pub latest_block_number: Option<u64>,
    pub included_block_number: Option<u64>,
    pub included_block_hash: Option<String>,
    pub confirmation_depth: Option<u64>,
    pub gas_used: Option<String>,
    pub batch_total_delta_wei: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettlementBroadcastPolicy {
    pub expected_nonce: U256,
    pub maximum_gas_limit: U256,
    pub maximum_fee_per_gas_wei: U256,
    pub maximum_priority_fee_per_gas_wei: U256,
    pub maximum_total_fee_wei: U256,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedSignedEip1559Submission {
    pub transaction_hash: H256,
    pub signer_address: Address,
    pub nonce: U256,
    pub maximum_priority_fee_per_gas_wei: U256,
    pub maximum_fee_per_gas_wei: U256,
    pub gas_limit: U256,
    pub maximum_total_fee_wei: U256,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettlementBroadcastResult {
    pub transaction_hash: String,
    pub signer_address: String,
    pub nonce: String,
    pub maximum_priority_fee_per_gas_wei: String,
    pub maximum_fee_per_gas_wei: String,
    pub gas_limit: String,
    pub maximum_total_fee_wei: String,
    pub current_preflight: SettlementRpcPreflight,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettlementBatcherError {
    BatchTooLarge(usize),
    BatcherRoleMissing(Address),
    Broadcast(String),
    ChainIdMismatch { expected: u64, actual: u64 },
    ConfirmationDepthOutOfRange(u64),
    Decode(String),
    EmptyBatcherAddress,
    InvalidRpcResponse(&'static str),
    NonCanonicalCbor,
    Proof(ReceiptSettlementError),
    ProviderAuthorizationExpiryTooClose { remaining_seconds: u64 },
    Rpc(String),
    RpcBlockInFuture { block_unix_s: u64, now_unix_s: u64 },
    RpcBlockStale { block_unix_s: u64, now_unix_s: u64 },
    RpcChainIdMismatch { expected: u64, actual: U256 },
    Serialize(String),
    SignedTransaction(String),
    SubmissionPolicy(String),
    TrackingManifest(String),
    TransactionMismatch(&'static str),
}

impl Display for SettlementBatcherError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BatchTooLarge(count) => write!(
                formatter,
                "batch has {} claims; the independent safety limit is {}",
                count, MAX_BATCHER_CLAIMS
            ),
            Self::BatcherRoleMissing(address) => write!(
                formatter,
                "batcher address {:#x} does not hold BATCHER_ROLE at the observed block",
                address
            ),
            Self::Broadcast(error) => write!(
                formatter,
                "signed transaction broadcast outcome is uncertain: {}",
                error
            ),
            Self::ChainIdMismatch { expected, actual } => write!(
                formatter,
                "batch chain ID {} does not match selected chain ID {}",
                actual, expected
            ),
            Self::ConfirmationDepthOutOfRange(depth) => write!(
                formatter,
                "confirmation depth {} is outside the allowed {}..={} range",
                depth, MIN_CONFIRMATION_DEPTH, MAX_CONFIRMATION_DEPTH
            ),
            Self::Decode(error) => write!(formatter, "cannot decode batch CBOR: {}", error),
            Self::EmptyBatcherAddress => write!(formatter, "batcher address cannot be zero"),
            Self::InvalidRpcResponse(field) => {
                write!(formatter, "RPC returned an invalid {} response", field)
            }
            Self::NonCanonicalCbor => {
                write!(formatter, "batch is not encoded as canonical MASQ CBOR")
            }
            Self::Proof(error) => write!(formatter, "batch proof verification failed: {:?}", error),
            Self::ProviderAuthorizationExpiryTooClose { remaining_seconds } => write!(
                formatter,
                "provider payout authorization has only {} seconds remaining; at least {} are required",
                remaining_seconds, MIN_PROVIDER_AUTHORIZATION_REMAINING_SECONDS
            ),
            Self::Rpc(error) => write!(formatter, "settlement RPC operation failed: {}", error),
            Self::RpcBlockInFuture {
                block_unix_s,
                now_unix_s,
            } => write!(
                formatter,
                "RPC block timestamp {} is too far ahead of local time {}",
                block_unix_s, now_unix_s
            ),
            Self::RpcBlockStale {
                block_unix_s,
                now_unix_s,
            } => write!(
                formatter,
                "RPC block timestamp {} is stale relative to local time {}",
                block_unix_s, now_unix_s
            ),
            Self::RpcChainIdMismatch { expected, actual } => write!(
                formatter,
                "RPC chain ID {} does not match selected chain ID {}",
                actual, expected
            ),
            Self::Serialize(error) => {
                write!(formatter, "cannot canonicalize batch CBOR: {}", error)
            }
            Self::SignedTransaction(error) => {
                write!(formatter, "invalid signed EIP-1559 transaction: {}", error)
            }
            Self::SubmissionPolicy(error) => {
                write!(formatter, "submission policy rejected transaction: {}", error)
            }
            Self::TrackingManifest(error) => {
                write!(formatter, "invalid tracking manifest: {}", error)
            }
            Self::TransactionMismatch(field) => {
                write!(formatter, "tracked transaction does not match manifest field {}", field)
            }
        }
    }
}

impl From<ReceiptSettlementError> for SettlementBatcherError {
    fn from(error: ReceiptSettlementError) -> Self {
        Self::Proof(error)
    }
}

/// Verifies canonical exported CBOR in a separate process and prepares only unsigned calldata.
/// Nonce, fees, gas, role authorization, signing and broadcasting intentionally remain the
/// responsibility of an external wallet/HSM transaction policy.
pub fn prepare_submission_manifest(
    canonical_batch_cbor: &[u8],
    expected_chain: Chain,
    batch_sequence: u64,
    verification_time_unix_s: u64,
) -> Result<SettlementSubmissionManifest, SettlementBatcherError> {
    let batch = verify_exported_batch(
        canonical_batch_cbor,
        expected_chain,
        verification_time_unix_s,
    )?;
    Ok(manifest_from_verified_batch(
        canonical_batch_cbor,
        &batch,
        batch_sequence,
        verification_time_unix_s,
        None,
    ))
}

/// Performs the same independent proof reconstruction and then binds the unsigned calldata to
/// one immutable latest block hash. The selected RPC must report the expected chain, return
/// contract code, authorize the external batcher address, and successfully simulate the complete
/// atomic submitBatchWithSessions call at that exact hash.
pub fn prepare_rpc_bound_submission_manifest(
    canonical_batch_cbor: &[u8],
    expected_chain: Chain,
    batcher_address: Address,
    system_now_unix_s: u64,
    rpc_url: &str,
) -> Result<SettlementSubmissionManifest, SettlementBatcherError> {
    prepare_rpc_bound_submission_manifest_with_gas(
        canonical_batch_cbor,
        expected_chain,
        batcher_address,
        system_now_unix_s,
        rpc_url,
        None,
    )
}

fn prepare_rpc_bound_submission_manifest_with_gas(
    canonical_batch_cbor: &[u8],
    expected_chain: Chain,
    batcher_address: Address,
    system_now_unix_s: u64,
    rpc_url: &str,
    simulation_gas_opt: Option<U256>,
) -> Result<SettlementSubmissionManifest, SettlementBatcherError> {
    let (_event_loop_handle, transport) = Http::with_max_parallel(rpc_url, 1)
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?;
    prepare_rpc_bound_submission_manifest_with_transport(
        canonical_batch_cbor,
        expected_chain,
        batcher_address,
        system_now_unix_s,
        &transport,
        simulation_gas_opt,
    )
}

fn prepare_rpc_bound_submission_manifest_with_transport(
    canonical_batch_cbor: &[u8],
    expected_chain: Chain,
    batcher_address: Address,
    system_now_unix_s: u64,
    transport: &Http,
    simulation_gas_opt: Option<U256>,
) -> Result<SettlementSubmissionManifest, SettlementBatcherError> {
    if batcher_address == Address::zero() {
        return Err(SettlementBatcherError::EmptyBatcherAddress);
    }
    let batch = decode_canonical_batch(canonical_batch_cbor, expected_chain)?;
    let expected_chain_id = expected_chain.rec().num_chain_id;
    let web3 = Web3::new(transport.clone());
    let rpc_chain_id = web3
        .eth()
        .chain_id()
        .wait()
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?;
    if rpc_chain_id != U256::from(expected_chain_id) {
        return Err(SettlementBatcherError::RpcChainIdMismatch {
            expected: expected_chain_id,
            actual: rpc_chain_id,
        });
    }
    let observed_block_number = web3
        .eth()
        .block_number()
        .wait()
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?
        .as_u64();
    let block = web3
        .eth()
        .block(BlockId::Number(BlockNumber::Number(
            observed_block_number.into(),
        )))
        .wait()
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?
        .ok_or(SettlementBatcherError::InvalidRpcResponse("latest block"))?;
    if block.number.map(|number| number.as_u64()) != Some(observed_block_number) {
        return Err(SettlementBatcherError::InvalidRpcResponse(
            "latest block number",
        ));
    }
    let observed_block_hash = block
        .hash
        .ok_or(SettlementBatcherError::InvalidRpcResponse(
            "latest block hash",
        ))?;
    if block.timestamp.bits() > 64 {
        return Err(SettlementBatcherError::InvalidRpcResponse(
            "latest block timestamp",
        ));
    }
    let observed_block_timestamp_unix_s = block.timestamp.low_u64();
    validate_rpc_block_freshness(observed_block_timestamp_unix_s, system_now_unix_s)?;
    verify_batch_proofs(&batch, expected_chain, observed_block_timestamp_unix_s)?;
    let provider_authorization_remaining_seconds =
        provider_authorization_remaining_seconds(&batch, observed_block_timestamp_unix_s)?;
    let contract_code =
        contract_code_at_hash(transport, batch.settlement_contract, observed_block_hash)?;
    if contract_code.0.is_empty() {
        return Err(SettlementBatcherError::InvalidRpcResponse(
            "settlement contract code",
        ));
    }
    let batch_sequence =
        next_batch_sequence_at_hash(transport, batch.settlement_contract, observed_block_hash)?;
    if !batcher_has_role_at_hash(
        transport,
        batch.settlement_contract,
        batcher_address,
        observed_block_hash,
    )? {
        return Err(SettlementBatcherError::BatcherRoleMissing(batcher_address));
    }
    let calldata = submit_batch_with_sessions_calldata(batch_sequence, &batch);
    let simulation_gas = simulation_gas_opt.unwrap_or(block.gas_limit);
    if simulation_gas.is_zero() || simulation_gas > block.gas_limit {
        return Err(SettlementBatcherError::SubmissionPolicy(
            "signed gas limit is zero or exceeds the current block gas limit".to_string(),
        ));
    }
    let simulation_result = contract_call_at_hash(
        transport,
        batch.settlement_contract,
        Some(batcher_address),
        Some(simulation_gas),
        calldata,
        observed_block_hash,
    )?;
    if !simulation_result.0.is_empty() {
        return Err(SettlementBatcherError::InvalidRpcResponse(
            "submitBatchWithSessions simulation",
        ));
    }

    let preflight = SettlementRpcPreflight {
        observed_block_number,
        observed_block_hash: format!("{:#x}", observed_block_hash),
        observed_block_timestamp_unix_s,
        settlement_contract_code_keccak256: format!(
            "0x{}",
            contract_code.0.keccak256().to_hex::<String>()
        ),
        batcher_address: format!("{:#x}", batcher_address),
        provider_authorization_remaining_seconds,
        batcher_role_authorized: true,
        submit_batch_simulated: true,
    };
    Ok(manifest_from_verified_batch(
        canonical_batch_cbor,
        &batch,
        batch_sequence,
        observed_block_timestamp_unix_s,
        Some(preflight),
    ))
}

/// Fully decodes and recovers one externally signed EIP-1559 type-2 transaction. This accepts no
/// key material and rejects legacy envelopes, non-canonical RLP, access lists, high-s signatures,
/// mismatched calldata/value/signer/chain and any fee or nonce outside explicit operator policy.
pub fn verify_signed_eip1559_submission(
    manifest: &SettlementSubmissionManifest,
    raw_transaction: &[u8],
    policy: &SettlementBroadcastPolicy,
) -> Result<VerifiedSignedEip1559Submission, SettlementBatcherError> {
    let validated = validate_tracking_manifest(manifest)?;
    if raw_transaction.len() > MAX_SIGNED_EIP1559_TRANSACTION_BYTES {
        return Err(signed_transaction_error(
            "transaction exceeds the byte limit",
        ));
    }
    if raw_transaction.first() != Some(&0x02) {
        return Err(signed_transaction_error(
            "only an EIP-1559 type-2 envelope is accepted",
        ));
    }
    let rlp = Rlp::new(&raw_transaction[1..]);
    if rlp
        .item_count()
        .map_err(|_| signed_transaction_error("RLP envelope is malformed"))?
        != 12
    {
        return Err(signed_transaction_error(
            "type-2 envelope must contain exactly 12 fields",
        ));
    }
    let chain_id = rlp_u256(&rlp, 0, "chain ID")?;
    let nonce = rlp_u256(&rlp, 1, "nonce")?;
    let maximum_priority_fee_per_gas_wei = rlp_u256(&rlp, 2, "priority fee")?;
    let maximum_fee_per_gas_wei = rlp_u256(&rlp, 3, "maximum fee")?;
    let gas_limit = rlp_u256(&rlp, 4, "gas limit")?;
    let destination_bytes = rlp_data(&rlp, 5, "destination")?;
    if destination_bytes.len() != 20 {
        return Err(signed_transaction_error(
            "destination must be one 20-byte contract address",
        ));
    }
    let destination = Address::from_slice(destination_bytes);
    let value = rlp_u256(&rlp, 6, "value")?;
    let calldata = rlp_data(&rlp, 7, "calldata")?.to_vec();
    let access_list = rlp
        .at(8)
        .map_err(|_| signed_transaction_error("access list is malformed"))?;
    if !access_list.is_list()
        || access_list
            .item_count()
            .map_err(|_| signed_transaction_error("access list is malformed"))?
            != 0
    {
        return Err(signed_transaction_error(
            "access lists are not permitted for settlement submission",
        ));
    }
    let y_parity = rlp_u256(&rlp, 9, "y parity")?;
    if y_parity > U256::one() {
        return Err(signed_transaction_error("y parity must be zero or one"));
    }
    let signature_r = rlp_u256(&rlp, 10, "signature r")?;
    let signature_s = rlp_u256(&rlp, 11, "signature s")?;
    let canonical_raw_transaction = encode_eip1559_transaction(
        chain_id,
        nonce,
        maximum_priority_fee_per_gas_wei,
        maximum_fee_per_gas_wei,
        gas_limit,
        destination,
        value,
        &calldata,
        Some((y_parity.low_u32() as u8, signature_r, signature_s)),
    );
    if canonical_raw_transaction != raw_transaction {
        return Err(signed_transaction_error(
            "transaction is not canonically RLP encoded",
        ));
    }
    if chain_id != U256::from(manifest.chain_id) {
        return Err(signed_transaction_error(
            "chain ID does not match the submission manifest",
        ));
    }
    if destination != validated.settlement_contract {
        return Err(signed_transaction_error(
            "destination does not match the settlement contract",
        ));
    }
    if !value.is_zero() {
        return Err(signed_transaction_error("transaction value must be zero"));
    }
    if calldata != validated.calldata {
        return Err(signed_transaction_error(
            "calldata does not match the submission manifest",
        ));
    }
    validate_broadcast_policy(
        nonce,
        maximum_priority_fee_per_gas_wei,
        maximum_fee_per_gas_wei,
        gas_limit,
        policy,
    )?;

    let signing_payload = encode_eip1559_transaction(
        chain_id,
        nonce,
        maximum_priority_fee_per_gas_wei,
        maximum_fee_per_gas_wei,
        gas_limit,
        destination,
        value,
        &calldata,
        None,
    );
    let signing_hash = signing_payload.keccak256();
    let message = Message::parse(&signing_hash);
    let mut signature_bytes = [0u8; 64];
    signature_r.to_big_endian(&mut signature_bytes[..32]);
    signature_s.to_big_endian(&mut signature_bytes[32..]);
    let signature = Signature::parse_standard(&signature_bytes)
        .map_err(|_| signed_transaction_error("signature r or s is out of range"))?;
    let mut normalized_signature = signature;
    normalized_signature.normalize_s();
    if normalized_signature != signature {
        return Err(signed_transaction_error("signature is not canonical low-s"));
    }
    let recovery_id = RecoveryId::parse(y_parity.low_u32() as u8)
        .map_err(|_| signed_transaction_error("y parity is invalid"))?;
    let public_key = recover(&message, &signature, &recovery_id)
        .map_err(|_| signed_transaction_error("signature recovery failed"))?;
    if !verify(&message, &signature, &public_key) {
        return Err(signed_transaction_error("signature verification failed"));
    }
    let signer_address = ethereum_address(&public_key);
    if signer_address != validated.batcher_address {
        return Err(signed_transaction_error(
            "recovered signer does not match the preflight batcher",
        ));
    }
    let maximum_total_fee_wei = gas_limit
        .checked_mul(maximum_fee_per_gas_wei)
        .ok_or_else(|| submission_policy_error("maximum total fee overflows uint256"))?;
    Ok(VerifiedSignedEip1559Submission {
        transaction_hash: H256::from_slice(&raw_transaction.keccak256()),
        signer_address,
        nonce,
        maximum_priority_fee_per_gas_wei,
        maximum_fee_per_gas_wei,
        gas_limit,
        maximum_total_fee_wei,
    })
}

/// Reconstructs and re-preflights the batch at a fresh canonical block before relaying the exact
/// externally signed bytes. A returned RPC transaction hash must equal the locally derived hash.
pub fn broadcast_signed_eip1559_submission(
    canonical_batch_cbor: &[u8],
    expected_chain: Chain,
    manifest: &SettlementSubmissionManifest,
    raw_transaction: &[u8],
    policy: &SettlementBroadcastPolicy,
    system_now_unix_s: u64,
    rpc_url: &str,
) -> Result<SettlementBroadcastResult, SettlementBatcherError> {
    let verified = verify_signed_eip1559_submission(manifest, raw_transaction, policy)?;
    let (_event_loop_handle, transport) = Http::with_max_parallel(rpc_url, 1)
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?;
    let current_manifest = prepare_rpc_bound_submission_manifest_with_transport(
        canonical_batch_cbor,
        expected_chain,
        verified.signer_address,
        system_now_unix_s,
        &transport,
        Some(verified.gas_limit),
    )?;
    if !submission_payloads_match(manifest, &current_manifest) {
        return Err(submission_policy_error(
            "fresh RPC preflight no longer matches the signed manifest payload",
        ));
    }
    let current_preflight = current_manifest
        .rpc_preflight_opt
        .clone()
        .ok_or_else(|| submission_policy_error("fresh RPC preflight evidence is missing"))?;
    let web3 = Web3::new(transport);
    let pending_nonce = web3
        .eth()
        .transaction_count(verified.signer_address, Some(BlockNumber::Pending))
        .wait()
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?;
    if pending_nonce != verified.nonce {
        return Err(submission_policy_error(
            "signed nonce does not match the RPC pending nonce",
        ));
    }
    let submitted_hash = web3
        .eth()
        .send_raw_transaction(Bytes(raw_transaction.to_vec()))
        .wait()
        .map_err(|error| SettlementBatcherError::Broadcast(error.to_string()))?;
    if submitted_hash != verified.transaction_hash {
        return Err(SettlementBatcherError::Broadcast(
            "RPC returned a transaction hash different from the locally derived hash".to_string(),
        ));
    }
    Ok(SettlementBroadcastResult {
        transaction_hash: format!("{:#x}", verified.transaction_hash),
        signer_address: format!("{:#x}", verified.signer_address),
        nonce: verified.nonce.to_string(),
        maximum_priority_fee_per_gas_wei: verified.maximum_priority_fee_per_gas_wei.to_string(),
        maximum_fee_per_gas_wei: verified.maximum_fee_per_gas_wei.to_string(),
        gas_limit: verified.gas_limit.to_string(),
        maximum_total_fee_wei: verified.maximum_total_fee_wei.to_string(),
        current_preflight,
    })
}

fn validate_broadcast_policy(
    nonce: U256,
    maximum_priority_fee_per_gas_wei: U256,
    maximum_fee_per_gas_wei: U256,
    gas_limit: U256,
    policy: &SettlementBroadcastPolicy,
) -> Result<(), SettlementBatcherError> {
    if policy.maximum_gas_limit.is_zero()
        || policy.maximum_fee_per_gas_wei.is_zero()
        || policy.maximum_total_fee_wei.is_zero()
    {
        return Err(submission_policy_error(
            "gas, maximum-fee and total-fee limits must be nonzero",
        ));
    }
    if policy.expected_nonce != nonce {
        return Err(submission_policy_error(
            "nonce does not match the explicit expected nonce",
        ));
    }
    if gas_limit.is_zero() || gas_limit > policy.maximum_gas_limit {
        return Err(submission_policy_error(
            "gas limit is zero or exceeds operator policy",
        ));
    }
    if maximum_priority_fee_per_gas_wei > maximum_fee_per_gas_wei {
        return Err(submission_policy_error(
            "priority fee exceeds maximum fee per gas",
        ));
    }
    if maximum_priority_fee_per_gas_wei > policy.maximum_priority_fee_per_gas_wei {
        return Err(submission_policy_error(
            "priority fee exceeds operator policy",
        ));
    }
    if maximum_fee_per_gas_wei.is_zero() || maximum_fee_per_gas_wei > policy.maximum_fee_per_gas_wei
    {
        return Err(submission_policy_error(
            "maximum fee per gas is zero or exceeds operator policy",
        ));
    }
    let maximum_total_fee_wei = gas_limit
        .checked_mul(maximum_fee_per_gas_wei)
        .ok_or_else(|| submission_policy_error("maximum total fee overflows uint256"))?;
    if maximum_total_fee_wei > policy.maximum_total_fee_wei {
        return Err(submission_policy_error(
            "maximum total fee exceeds operator policy",
        ));
    }
    Ok(())
}

fn submission_payloads_match(
    original: &SettlementSubmissionManifest,
    current: &SettlementSubmissionManifest,
) -> bool {
    original.chain_id == current.chain_id
        && original.settlement_contract == current.settlement_contract
        && original.batch_sequence == current.batch_sequence
        && original.claim_count == current.claim_count
        && original.claim_ids == current.claim_ids
        && original.portable_merkle_root == current.portable_merkle_root
        && original.contract_merkle_root == current.contract_merkle_root
        && original.total_cumulative_charge_wei == current.total_cumulative_charge_wei
        && original.batch_cbor_keccak256 == current.batch_cbor_keccak256
        && original.calldata_keccak256 == current.calldata_keccak256
        && original.value_wei == current.value_wei
        && original.calldata == current.calldata
}

fn encode_eip1559_transaction(
    chain_id: U256,
    nonce: U256,
    maximum_priority_fee_per_gas_wei: U256,
    maximum_fee_per_gas_wei: U256,
    gas_limit: U256,
    destination: Address,
    value: U256,
    calldata: &[u8],
    signature_opt: Option<(u8, U256, U256)>,
) -> Vec<u8> {
    let mut stream = RlpStream::new_list(if signature_opt.is_some() { 12 } else { 9 });
    stream.append(&chain_id);
    stream.append(&nonce);
    stream.append(&maximum_priority_fee_per_gas_wei);
    stream.append(&maximum_fee_per_gas_wei);
    stream.append(&gas_limit);
    let destination_bytes: &[u8] = destination.as_bytes();
    stream.append(&destination_bytes);
    stream.append(&value);
    stream.append(&calldata);
    stream.begin_list(0);
    if let Some((y_parity, signature_r, signature_s)) = signature_opt {
        stream.append(&y_parity);
        stream.append(&signature_r);
        stream.append(&signature_s);
    }
    let mut encoded = vec![0x02];
    encoded.extend(stream.out());
    encoded
}

fn rlp_u256(rlp: &Rlp<'_>, index: usize, field: &str) -> Result<U256, SettlementBatcherError> {
    rlp.val_at(index)
        .map_err(|_| signed_transaction_error(&format!("{} is malformed", field)))
}

fn rlp_data<'a>(
    rlp: &'a Rlp<'a>,
    index: usize,
    field: &str,
) -> Result<&'a [u8], SettlementBatcherError> {
    rlp.at(index)
        .and_then(|value| value.data())
        .map_err(|_| signed_transaction_error(&format!("{} is malformed", field)))
}

fn ethereum_address(public_key: &PublicKey) -> Address {
    let serialized = public_key.serialize();
    Address::from_slice(&serialized[1..].keccak256()[12..])
}

fn signed_transaction_error(message: &str) -> SettlementBatcherError {
    SettlementBatcherError::SignedTransaction(message.to_string())
}

fn submission_policy_error(message: &str) -> SettlementBatcherError {
    SettlementBatcherError::SubmissionPolicy(message.to_string())
}

/// Observes a transaction broadcast by an external signer without ever accepting signing keys.
/// The manifest is revalidated before any RPC result is trusted. An included transaction must
/// match the exact zero-value calldata, sender and contract from the preflight, and its receipt,
/// deployed code and BatchSettled event must all agree at one canonical block.
pub fn track_submission_transaction(
    manifest: &SettlementSubmissionManifest,
    transaction_hash: H256,
    required_confirmation_depth: u64,
    rpc_url: &str,
) -> Result<SettlementTransactionObservation, SettlementBatcherError> {
    if !(MIN_CONFIRMATION_DEPTH..=MAX_CONFIRMATION_DEPTH).contains(&required_confirmation_depth) {
        return Err(SettlementBatcherError::ConfirmationDepthOutOfRange(
            required_confirmation_depth,
        ));
    }
    if transaction_hash == H256::zero() {
        return Err(SettlementBatcherError::TrackingManifest(
            "transaction hash cannot be zero".to_string(),
        ));
    }
    let validated = validate_tracking_manifest(manifest)?;
    let (_event_loop_handle, transport) = Http::with_max_parallel(rpc_url, 1)
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?;
    let web3 = Web3::new(transport.clone());
    let rpc_chain_id = web3
        .eth()
        .chain_id()
        .wait()
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?;
    if rpc_chain_id != U256::from(manifest.chain_id) {
        return Err(SettlementBatcherError::RpcChainIdMismatch {
            expected: manifest.chain_id,
            actual: rpc_chain_id,
        });
    }
    let transaction_opt = web3
        .eth()
        .transaction(TransactionId::Hash(transaction_hash))
        .wait()
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?;
    let transaction = match transaction_opt {
        Some(transaction) => transaction,
        None => {
            return Ok(transaction_observation(
                "not-found",
                manifest,
                transaction_hash,
                validated.batcher_address,
                required_confirmation_depth,
                None,
                None,
                None,
                None,
                None,
                None,
            ))
        }
    };
    if transaction.hash != transaction_hash {
        return Err(SettlementBatcherError::TransactionMismatch("hash"));
    }
    if transaction.from != validated.batcher_address {
        return Err(SettlementBatcherError::TransactionMismatch("from"));
    }
    if transaction.to != Some(validated.settlement_contract) {
        return Err(SettlementBatcherError::TransactionMismatch("to"));
    }
    if !transaction.value.is_zero() {
        return Err(SettlementBatcherError::TransactionMismatch("value"));
    }
    if transaction.input.0 != validated.calldata {
        return Err(SettlementBatcherError::TransactionMismatch("calldata"));
    }

    let receipt_opt = web3
        .eth()
        .transaction_receipt(transaction_hash)
        .wait()
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?;
    let receipt = match receipt_opt {
        Some(receipt) => receipt,
        None => {
            let latest_block_number = current_block_number(&web3)?;
            return Ok(transaction_observation(
                "pending",
                manifest,
                transaction_hash,
                validated.batcher_address,
                required_confirmation_depth,
                Some(latest_block_number),
                None,
                None,
                None,
                None,
                None,
            ));
        }
    };
    if receipt.transaction_hash != transaction_hash {
        return Err(SettlementBatcherError::TransactionMismatch(
            "receipt transaction hash",
        ));
    }
    let included_block_number = receipt
        .block_number
        .ok_or(SettlementBatcherError::InvalidRpcResponse(
            "receipt block number",
        ))?
        .as_u64();
    let included_block_hash =
        receipt
            .block_hash
            .ok_or(SettlementBatcherError::InvalidRpcResponse(
                "receipt block hash",
            ))?;
    if transaction.block_number.map(|number| number.as_u64()) != Some(included_block_number)
        || transaction.block_hash != Some(included_block_hash)
    {
        return Err(SettlementBatcherError::TransactionMismatch(
            "transaction inclusion block",
        ));
    }
    let latest_block_number = current_block_number(&web3)?;
    let canonical_block = web3
        .eth()
        .block(BlockId::Number(BlockNumber::Number(
            included_block_number.into(),
        )))
        .wait()
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?
        .ok_or(SettlementBatcherError::InvalidRpcResponse(
            "canonical receipt block",
        ))?;
    let canonical_block_hash =
        canonical_block
            .hash
            .ok_or(SettlementBatcherError::InvalidRpcResponse(
                "canonical receipt block hash",
            ))?;
    if canonical_block_hash != included_block_hash {
        return Ok(transaction_observation(
            "reorged",
            manifest,
            transaction_hash,
            validated.batcher_address,
            required_confirmation_depth,
            Some(latest_block_number),
            Some(included_block_number),
            Some(included_block_hash),
            None,
            receipt.gas_used,
            None,
        ));
    }
    if latest_block_number < included_block_number {
        return Err(SettlementBatcherError::InvalidRpcResponse(
            "latest block number",
        ));
    }
    let contract_code = contract_code_at_hash(
        &transport,
        validated.settlement_contract,
        included_block_hash,
    )?;
    if contract_code.0.is_empty() || contract_code.0.keccak256() != validated.settlement_code_hash.0
    {
        return Err(SettlementBatcherError::TransactionMismatch(
            "settlement contract code hash",
        ));
    }
    let confirmation_depth = latest_block_number - included_block_number;
    match receipt.status.map(|status| status.as_u64()) {
        Some(0) => {
            return Ok(transaction_observation(
                "reverted",
                manifest,
                transaction_hash,
                validated.batcher_address,
                required_confirmation_depth,
                Some(latest_block_number),
                Some(included_block_number),
                Some(included_block_hash),
                Some(confirmation_depth),
                receipt.gas_used,
                None,
            ))
        }
        Some(1) => {}
        _ => return Err(SettlementBatcherError::InvalidRpcResponse("receipt status")),
    }
    let batch_total_delta_wei = validate_batch_settled_event(
        &receipt,
        validated.settlement_contract,
        transaction_hash,
        included_block_number,
        included_block_hash,
        manifest,
        validated.contract_merkle_root,
    )?;
    Ok(transaction_observation(
        if confirmation_depth >= required_confirmation_depth {
            "finalized"
        } else {
            "included"
        },
        manifest,
        transaction_hash,
        validated.batcher_address,
        required_confirmation_depth,
        Some(latest_block_number),
        Some(included_block_number),
        Some(included_block_hash),
        Some(confirmation_depth),
        receipt.gas_used,
        Some(batch_total_delta_wei),
    ))
}

struct ValidatedTrackingManifest {
    settlement_contract: Address,
    batcher_address: Address,
    settlement_code_hash: H256,
    contract_merkle_root: H256,
    calldata: Vec<u8>,
}

fn validate_tracking_manifest(
    manifest: &SettlementSubmissionManifest,
) -> Result<ValidatedTrackingManifest, SettlementBatcherError> {
    if manifest.manifest_version != 1 {
        return Err(tracking_manifest_error("unsupported manifest version"));
    }
    if manifest.assurance_mode != "rpc-bound-preflight" {
        return Err(tracking_manifest_error(
            "only rpc-bound-preflight manifests can be tracked",
        ));
    }
    if manifest.chain_id == 0 {
        return Err(tracking_manifest_error("chain ID cannot be zero"));
    }
    if manifest.value_wei != "0" {
        return Err(tracking_manifest_error("transaction value must be zero"));
    }
    if manifest.claim_count == 0 || manifest.claim_count > MAX_BATCHER_CLAIMS {
        return Err(tracking_manifest_error(
            "claim count is outside the safety limit",
        ));
    }
    if manifest.claim_ids.len() != manifest.claim_count {
        return Err(tracking_manifest_error(
            "claim ID count does not match claim count",
        ));
    }
    let parsed_claim_ids = manifest
        .claim_ids
        .iter()
        .map(|claim_id| parse_hash(claim_id, "claim ID"))
        .collect::<Result<Vec<_>, _>>()?;
    let mut unique_claim_ids = parsed_claim_ids.clone();
    unique_claim_ids.sort();
    unique_claim_ids.dedup();
    if unique_claim_ids.len() != parsed_claim_ids.len() {
        return Err(tracking_manifest_error("claim IDs are not unique"));
    }
    let settlement_contract = parse_address(&manifest.settlement_contract, "settlement contract")?;
    if settlement_contract == Address::zero() {
        return Err(tracking_manifest_error(
            "settlement contract cannot be zero",
        ));
    }
    let contract_merkle_root = parse_hash(&manifest.contract_merkle_root, "contract Merkle root")?;
    parse_hash(&manifest.portable_merkle_root, "portable Merkle root")?;
    parse_hash(&manifest.batch_cbor_keccak256, "batch CBOR hash")?;
    let calldata_hash = parse_hash(&manifest.calldata_keccak256, "calldata hash")?;
    let calldata = parse_prefixed_hex(&manifest.calldata, "calldata")?;
    if calldata.keccak256() != calldata_hash.0 {
        return Err(tracking_manifest_error(
            "calldata hash does not match calldata",
        ));
    }
    if calldata.len() < 4 || calldata[..4] != SUBMIT_BATCH_WITH_SESSIONS_SIGNATURE.keccak256()[..4]
    {
        return Err(tracking_manifest_error(
            "calldata selector is not submitBatchWithSessions",
        ));
    }
    let tokens = ethabi::decode(
        &[
            ParamType::Array(Box::new(ParamType::Tuple(vec![
                Box::new(ParamType::Uint(16)),
                Box::new(ParamType::Address),
                Box::new(ParamType::Bytes),
                Box::new(ParamType::Uint(256)),
                Box::new(ParamType::Uint(64)),
                Box::new(ParamType::Uint(64)),
                Box::new(ParamType::FixedBytes(32)),
            ]))),
            ParamType::Array(Box::new(ParamType::Bytes)),
            ParamType::Uint(64),
            ParamType::FixedBytes(32),
            ParamType::Array(Box::new(ParamType::Tuple(vec![
                Box::new(ParamType::FixedBytes(32)),
                Box::new(ParamType::FixedBytes(32)),
                Box::new(ParamType::Address),
                Box::new(ParamType::Address),
                Box::new(ParamType::Uint(128)),
            ]))),
        ],
        &calldata[4..],
    )
    .map_err(|_| tracking_manifest_error("calldata ABI is invalid"))?;
    let canonical_arguments = ethabi::encode(&tokens);
    if canonical_arguments != calldata[4..] {
        return Err(tracking_manifest_error("calldata ABI is non-canonical"));
    }
    let authorizations = match &tokens[0] {
        Token::Array(authorizations) => authorizations,
        _ => {
            return Err(tracking_manifest_error(
                "calldata authorizations are invalid",
            ))
        }
    };
    let signatures = match &tokens[1] {
        Token::Array(signatures) => signatures,
        _ => return Err(tracking_manifest_error("calldata signatures are invalid")),
    };
    if authorizations.len() != signatures.len()
        || authorizations.is_empty()
        || authorizations.len() > manifest.claim_count
        || authorizations.len() > MAX_BATCHER_CLAIMS
    {
        return Err(tracking_manifest_error(
            "calldata session registration count is invalid",
        ));
    }
    let mut registered_session_ids = Vec::<[u8; 32]>::new();
    for (authorization, signature) in authorizations.iter().zip(signatures.iter()) {
        let fields = match authorization {
            Token::Tuple(fields) if fields.len() == 7 => fields,
            _ => {
                return Err(tracking_manifest_error(
                    "calldata authorization tuple is invalid",
                ))
            }
        };
        let payer = match &fields[1] {
            Token::Address(payer) => payer,
            _ => return Err(tracking_manifest_error("calldata payer is invalid")),
        };
        let authorization_nonce = match &fields[6] {
            Token::FixedBytes(bytes) if bytes.len() == 32 => bytes,
            _ => {
                return Err(tracking_manifest_error(
                    "calldata authorization nonce is invalid",
                ))
            }
        };
        let mut nonce = [0u8; 32];
        nonce.copy_from_slice(authorization_nonce);
        registered_session_ids.push(receipt_session_contract_id(*payer, &nonce));
        match signature {
            Token::Bytes(bytes) if bytes.len() == 65 && matches!(bytes[64], 27 | 28) => {}
            _ => {
                return Err(tracking_manifest_error(
                    "calldata wallet signature is invalid",
                ))
            }
        }
    }
    registered_session_ids.sort();
    registered_session_ids.dedup();
    if registered_session_ids.len() != authorizations.len() {
        return Err(tracking_manifest_error(
            "calldata session registrations are not unique",
        ));
    }
    let sequence = match &tokens[2] {
        Token::Uint(value) => value,
        _ => return Err(tracking_manifest_error("calldata sequence is invalid")),
    };
    if *sequence != U256::from(manifest.batch_sequence) {
        return Err(tracking_manifest_error(
            "calldata sequence does not match manifest",
        ));
    }
    let root = match &tokens[3] {
        Token::FixedBytes(bytes) => bytes,
        _ => return Err(tracking_manifest_error("calldata Merkle root is invalid")),
    };
    if root.as_slice() != contract_merkle_root.as_bytes() {
        return Err(tracking_manifest_error(
            "calldata Merkle root does not match manifest",
        ));
    }
    let claims = match &tokens[4] {
        Token::Array(claims) => claims,
        _ => return Err(tracking_manifest_error("calldata claims are invalid")),
    };
    if claims.len() != manifest.claim_count {
        return Err(tracking_manifest_error(
            "calldata claim count does not match manifest",
        ));
    }
    let mut total_cumulative_charge_wei = U256::zero();
    let mut claimed_session_ids = Vec::<[u8; 32]>::new();
    for (index, claim) in claims.iter().enumerate() {
        let fields = match claim {
            Token::Tuple(fields) if fields.len() == 5 => fields,
            _ => return Err(tracking_manifest_error("calldata claim tuple is invalid")),
        };
        let claim_id = match &fields[0] {
            Token::FixedBytes(bytes) => bytes,
            _ => return Err(tracking_manifest_error("calldata claim ID is invalid")),
        };
        if claim_id.as_slice() != parsed_claim_ids[index].as_bytes() {
            return Err(tracking_manifest_error(
                "calldata claim ID does not match manifest",
            ));
        }
        let session_id = match &fields[1] {
            Token::FixedBytes(bytes) if bytes.len() == 32 => bytes,
            _ => return Err(tracking_manifest_error("calldata session ID is invalid")),
        };
        let mut session_id_array = [0u8; 32];
        session_id_array.copy_from_slice(session_id);
        claimed_session_ids.push(session_id_array);
        let cumulative_charge = match &fields[4] {
            Token::Uint(value) => value,
            _ => {
                return Err(tracking_manifest_error(
                    "calldata cumulative charge is invalid",
                ))
            }
        };
        total_cumulative_charge_wei = total_cumulative_charge_wei
            .checked_add(*cumulative_charge)
            .ok_or_else(|| tracking_manifest_error("cumulative charge sum overflows"))?;
    }
    claimed_session_ids.sort();
    claimed_session_ids.dedup();
    if claimed_session_ids != registered_session_ids {
        return Err(tracking_manifest_error(
            "calldata registrations do not cover the claim sessions",
        ));
    }
    let manifest_total = U256::from_dec_str(&manifest.total_cumulative_charge_wei)
        .map_err(|_| tracking_manifest_error("total cumulative charge is invalid"))?;
    if manifest_total != total_cumulative_charge_wei {
        return Err(tracking_manifest_error(
            "calldata cumulative charge total does not match manifest",
        ));
    }
    let preflight = manifest
        .rpc_preflight_opt
        .as_ref()
        .ok_or_else(|| tracking_manifest_error("RPC preflight evidence is missing"))?;
    if !preflight.batcher_role_authorized || !preflight.submit_batch_simulated {
        return Err(tracking_manifest_error("RPC preflight did not pass"));
    }
    if preflight.observed_block_timestamp_unix_s != manifest.verification_time_unix_s {
        return Err(tracking_manifest_error(
            "preflight timestamp does not match verification time",
        ));
    }
    if preflight.provider_authorization_remaining_seconds
        < MIN_PROVIDER_AUTHORIZATION_REMAINING_SECONDS
    {
        return Err(tracking_manifest_error(
            "provider payout authorization margin is too short",
        ));
    }
    parse_hash(&preflight.observed_block_hash, "preflight block hash")?;
    let settlement_code_hash = parse_hash(
        &preflight.settlement_contract_code_keccak256,
        "settlement contract code hash",
    )?;
    let batcher_address = parse_address(&preflight.batcher_address, "batcher address")?;
    if batcher_address == Address::zero() {
        return Err(tracking_manifest_error("batcher address cannot be zero"));
    }
    Ok(ValidatedTrackingManifest {
        settlement_contract,
        batcher_address,
        settlement_code_hash,
        contract_merkle_root,
        calldata,
    })
}

fn validate_batch_settled_event(
    receipt: &TransactionReceipt,
    settlement_contract: Address,
    transaction_hash: H256,
    included_block_number: u64,
    included_block_hash: H256,
    manifest: &SettlementSubmissionManifest,
    contract_merkle_root: H256,
) -> Result<U256, SettlementBatcherError> {
    let event_topic = H256::from_slice(&BATCH_SETTLED_EVENT_SIGNATURE.keccak256());
    let sequence_topic = u64_topic(manifest.batch_sequence);
    let matching_logs = receipt
        .logs
        .iter()
        .filter(|log| {
            log.address == settlement_contract
                && !log.is_removed()
                && log.topics.get(0) == Some(&event_topic)
        })
        .collect::<Vec<_>>();
    if matching_logs.len() != 1 {
        return Err(SettlementBatcherError::InvalidRpcResponse(
            "BatchSettled event count",
        ));
    }
    let log = matching_logs[0];
    if log.topics.len() != 3
        || log.topics[1] != sequence_topic
        || log.topics[2] != contract_merkle_root
    {
        return Err(SettlementBatcherError::TransactionMismatch(
            "BatchSettled event topics",
        ));
    }
    if log.block_number.map(|number| number.as_u64()) != Some(included_block_number)
        || log.block_hash != Some(included_block_hash)
        || log.transaction_hash != Some(transaction_hash)
    {
        return Err(SettlementBatcherError::TransactionMismatch(
            "BatchSettled event inclusion",
        ));
    }
    let values = ethabi::decode(&[ParamType::Uint(256), ParamType::Uint(256)], &log.data.0)
        .map_err(|_| SettlementBatcherError::InvalidRpcResponse("BatchSettled event data"))?;
    if ethabi::encode(&values) != log.data.0 {
        return Err(SettlementBatcherError::InvalidRpcResponse(
            "BatchSettled event data",
        ));
    }
    let claim_count = match &values[0] {
        Token::Uint(value) => value,
        _ => unreachable!(),
    };
    if *claim_count != U256::from(manifest.claim_count) {
        return Err(SettlementBatcherError::TransactionMismatch(
            "BatchSettled claim count",
        ));
    }
    match &values[1] {
        Token::Uint(value) => Ok(*value),
        _ => unreachable!(),
    }
}

fn current_block_number<T: Transport>(web3: &Web3<T>) -> Result<u64, SettlementBatcherError> {
    web3.eth()
        .block_number()
        .wait()
        .map(|number| number.as_u64())
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
fn transaction_observation(
    lifecycle_state: &str,
    manifest: &SettlementSubmissionManifest,
    transaction_hash: H256,
    batcher_address: Address,
    required_confirmation_depth: u64,
    latest_block_number: Option<u64>,
    included_block_number: Option<u64>,
    included_block_hash: Option<H256>,
    confirmation_depth: Option<u64>,
    gas_used: Option<U256>,
    batch_total_delta_wei: Option<U256>,
) -> SettlementTransactionObservation {
    SettlementTransactionObservation {
        lifecycle_state: lifecycle_state.to_string(),
        chain_id: manifest.chain_id,
        settlement_contract: manifest.settlement_contract.clone(),
        transaction_hash: format!("{:#x}", transaction_hash),
        batcher_address: format!("{:#x}", batcher_address),
        batch_sequence: manifest.batch_sequence,
        required_confirmation_depth,
        latest_block_number,
        included_block_number,
        included_block_hash: included_block_hash.map(|hash| format!("{:#x}", hash)),
        confirmation_depth,
        gas_used: gas_used.map(|gas| gas.to_string()),
        batch_total_delta_wei: batch_total_delta_wei.map(|amount| amount.to_string()),
    }
}

fn parse_address(value: &str, field: &str) -> Result<Address, SettlementBatcherError> {
    if value.len() != 42 || !value.starts_with("0x") {
        return Err(tracking_manifest_error(&format!(
            "{} must be exactly 20 bytes of 0x-prefixed hexadecimal",
            field
        )));
    }
    Address::from_str(&value[2..])
        .map_err(|_| tracking_manifest_error(&format!("{} is invalid", field)))
}

fn parse_hash(value: &str, field: &str) -> Result<H256, SettlementBatcherError> {
    if value.len() != 66 || !value.starts_with("0x") {
        return Err(tracking_manifest_error(&format!(
            "{} must be exactly 32 bytes of 0x-prefixed hexadecimal",
            field
        )));
    }
    H256::from_str(&value[2..])
        .map_err(|_| tracking_manifest_error(&format!("{} is invalid", field)))
}

fn parse_prefixed_hex(value: &str, field: &str) -> Result<Vec<u8>, SettlementBatcherError> {
    let hex = value
        .strip_prefix("0x")
        .ok_or_else(|| tracking_manifest_error(&format!("{} is not 0x-prefixed", field)))?;
    if hex.len() % 2 != 0 {
        return Err(tracking_manifest_error(&format!(
            "{} does not contain whole bytes",
            field
        )));
    }
    hex.from_hex::<Vec<u8>>()
        .map_err(|_| tracking_manifest_error(&format!("{} contains invalid hexadecimal", field)))
}

fn tracking_manifest_error(message: &str) -> SettlementBatcherError {
    SettlementBatcherError::TrackingManifest(message.to_string())
}

fn u64_topic(value: u64) -> H256 {
    let mut word = [0u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    H256::from(word)
}

fn verify_exported_batch(
    canonical_batch_cbor: &[u8],
    expected_chain: Chain,
    verification_time_unix_s: u64,
) -> Result<ReceiptSettlementBatch, SettlementBatcherError> {
    let batch = decode_canonical_batch(canonical_batch_cbor, expected_chain)?;
    verify_batch_proofs(&batch, expected_chain, verification_time_unix_s)?;
    Ok(batch)
}

fn decode_canonical_batch(
    canonical_batch_cbor: &[u8],
    expected_chain: Chain,
) -> Result<ReceiptSettlementBatch, SettlementBatcherError> {
    let batch = serde_cbor::from_slice::<ReceiptSettlementBatch>(canonical_batch_cbor)
        .map_err(|error| SettlementBatcherError::Decode(error.to_string()))?;
    let reencoded = serde_cbor::to_vec(&batch)
        .map_err(|error| SettlementBatcherError::Serialize(error.to_string()))?;
    if reencoded != canonical_batch_cbor {
        return Err(SettlementBatcherError::NonCanonicalCbor);
    }
    let expected_chain_id = expected_chain.rec().num_chain_id;
    if batch.chain_id != expected_chain_id {
        return Err(SettlementBatcherError::ChainIdMismatch {
            expected: expected_chain_id,
            actual: batch.chain_id,
        });
    }
    if batch.claims.len() > MAX_BATCHER_CLAIMS {
        return Err(SettlementBatcherError::BatchTooLarge(batch.claims.len()));
    }

    Ok(batch)
}

fn verify_batch_proofs(
    batch: &ReceiptSettlementBatch,
    expected_chain: Chain,
    verification_time_unix_s: u64,
) -> Result<(), SettlementBatcherError> {
    let receipt_verifier = CryptDEReal::new(expected_chain);
    batch.verify_exported(verification_time_unix_s, &receipt_verifier)?;
    Ok(())
}

fn validate_rpc_block_freshness(
    block_unix_s: u64,
    now_unix_s: u64,
) -> Result<(), SettlementBatcherError> {
    if block_unix_s > now_unix_s.saturating_add(MAX_RPC_BLOCK_FUTURE_DRIFT_SECONDS) {
        return Err(SettlementBatcherError::RpcBlockInFuture {
            block_unix_s,
            now_unix_s,
        });
    }
    if now_unix_s.saturating_sub(block_unix_s) > MAX_RPC_BLOCK_AGE_SECONDS {
        return Err(SettlementBatcherError::RpcBlockStale {
            block_unix_s,
            now_unix_s,
        });
    }
    Ok(())
}

fn provider_authorization_remaining_seconds(
    batch: &ReceiptSettlementBatch,
    observed_block_timestamp_unix_s: u64,
) -> Result<u64, SettlementBatcherError> {
    let remaining_seconds = batch
        .claims
        .iter()
        .map(|claim| {
            claim
                .provider_settlement
                .policy
                .expires_at_unix_s
                .saturating_sub(observed_block_timestamp_unix_s)
        })
        .min()
        .ok_or(SettlementBatcherError::InvalidRpcResponse(
            "provider payout authorization",
        ))?;
    if remaining_seconds < MIN_PROVIDER_AUTHORIZATION_REMAINING_SECONDS {
        return Err(
            SettlementBatcherError::ProviderAuthorizationExpiryTooClose { remaining_seconds },
        );
    }
    Ok(remaining_seconds)
}

fn manifest_from_verified_batch(
    canonical_batch_cbor: &[u8],
    batch: &ReceiptSettlementBatch,
    batch_sequence: u64,
    verification_time_unix_s: u64,
    rpc_preflight_opt: Option<SettlementRpcPreflight>,
) -> SettlementSubmissionManifest {
    let calldata = submit_batch_with_sessions_calldata(batch_sequence, batch);
    SettlementSubmissionManifest {
        manifest_version: 1,
        assurance_mode: if rpc_preflight_opt.is_some() {
            "rpc-bound-preflight".to_string()
        } else {
            "offline-manual-sequence".to_string()
        },
        chain_id: batch.chain_id,
        settlement_contract: format!("{:#x}", batch.settlement_contract),
        verification_time_unix_s,
        batch_sequence,
        claim_count: batch.contract_claims.len(),
        claim_ids: batch
            .contract_claims
            .iter()
            .map(|claim| format!("0x{}", claim.claim_id.to_hex::<String>()))
            .collect(),
        portable_merkle_root: format!("0x{}", batch.merkle_root.to_hex::<String>()),
        contract_merkle_root: format!("0x{}", batch.contract_merkle_root.to_hex::<String>()),
        total_cumulative_charge_wei: batch.total_claimed_wei.to_string(),
        batch_cbor_keccak256: format!("0x{}", canonical_batch_cbor.keccak256().to_hex::<String>()),
        calldata_keccak256: format!("0x{}", calldata.keccak256().to_hex::<String>()),
        value_wei: "0".to_string(),
        calldata: format!("0x{}", calldata.to_hex::<String>()),
        rpc_preflight_opt,
    }
}

fn contract_code_at_hash(
    transport: &Http,
    settlement_contract: Address,
    block_hash: H256,
) -> Result<Bytes, SettlementBatcherError> {
    let value = transport
        .execute(
            "eth_getCode",
            vec![
                json!(format!("{:#x}", settlement_contract)),
                json!({
                    "blockHash": format!("{:#x}", block_hash),
                    "requireCanonical": true
                }),
            ],
        )
        .wait()
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?;
    serde_json::from_value(value)
        .map_err(|_| SettlementBatcherError::InvalidRpcResponse("settlement contract code"))
}

fn next_batch_sequence_at_hash(
    transport: &Http,
    settlement_contract: Address,
    block_hash: H256,
) -> Result<u64, SettlementBatcherError> {
    let response = contract_call_at_hash(
        transport,
        settlement_contract,
        None,
        None,
        NEXT_BATCH_SEQUENCE_SIGNATURE.keccak256()[..4].to_vec(),
        block_hash,
    )?;
    let word = response_word(&response, "nextBatchSequence")?;
    if word[..24].iter().any(|byte| *byte != 0) {
        return Err(SettlementBatcherError::InvalidRpcResponse(
            "nextBatchSequence",
        ));
    }
    let mut sequence = [0u8; 8];
    sequence.copy_from_slice(&word[24..]);
    Ok(u64::from_be_bytes(sequence))
}

fn batcher_has_role_at_hash(
    transport: &Http,
    settlement_contract: Address,
    batcher_address: Address,
    block_hash: H256,
) -> Result<bool, SettlementBatcherError> {
    let mut calldata = HAS_ROLE_SIGNATURE.keccak256()[..4].to_vec();
    calldata.extend(ethabi::encode(&[
        Token::FixedBytes(BATCHER_ROLE_NAME.keccak256().to_vec()),
        Token::Address(batcher_address),
    ]));
    let response = contract_call_at_hash(
        transport,
        settlement_contract,
        None,
        None,
        calldata,
        block_hash,
    )?;
    let word = response_word(&response, "hasRole")?;
    if word[..31].iter().any(|byte| *byte != 0) || word[31] > 1 {
        return Err(SettlementBatcherError::InvalidRpcResponse("hasRole"));
    }
    Ok(word[31] == 1)
}

fn contract_call_at_hash(
    transport: &Http,
    settlement_contract: Address,
    from: Option<Address>,
    gas: Option<U256>,
    data: Vec<u8>,
    block_hash: H256,
) -> Result<Bytes, SettlementBatcherError> {
    let request = serde_json::to_value(CallRequest {
        from,
        to: settlement_contract,
        gas,
        gas_price: None,
        value: Some(U256::zero()),
        data: Some(Bytes(data)),
    })
    .map_err(|_| SettlementBatcherError::InvalidRpcResponse("eth_call request"))?;
    let value = transport
        .execute(
            "eth_call",
            vec![
                request,
                json!({
                    "blockHash": format!("{:#x}", block_hash),
                    "requireCanonical": true
                }),
            ],
        )
        .wait()
        .map_err(|error| SettlementBatcherError::Rpc(error.to_string()))?;
    serde_json::from_value(value)
        .map_err(|_| SettlementBatcherError::InvalidRpcResponse("eth_call"))
}

fn response_word(
    response: &Bytes,
    field: &'static str,
) -> Result<[u8; 32], SettlementBatcherError> {
    response
        .0
        .as_slice()
        .try_into()
        .map_err(|_| SettlementBatcherError::InvalidRpcResponse(field))
}

pub fn submit_batch_calldata(
    batch_sequence: u64,
    contract_merkle_root: [u8; 32],
    claims: &[ReceiptSettlementContractClaim],
) -> Vec<u8> {
    let claim_tokens = claims
        .iter()
        .map(|claim| {
            Token::Tuple(vec![
                Token::FixedBytes(claim.claim_id.to_vec()),
                Token::FixedBytes(claim.session_id.to_vec()),
                Token::Address(claim.payer_wallet_address),
                Token::Address(claim.payout_wallet_address),
                Token::Uint(U256::from(claim.cumulative_charge_wei)),
            ])
        })
        .collect::<Vec<_>>();
    let mut calldata = SUBMIT_BATCH_SIGNATURE.keccak256()[..4].to_vec();
    calldata.extend(ethabi::encode(&[
        Token::Uint(U256::from(batch_sequence)),
        Token::FixedBytes(contract_merkle_root.to_vec()),
        Token::Array(claim_tokens),
    ]));
    calldata
}

pub fn submit_batch_with_sessions_calldata(
    batch_sequence: u64,
    batch: &ReceiptSettlementBatch,
) -> Vec<u8> {
    let mut registrations = batch
        .claims
        .iter()
        .map(|claim| {
            let authorized = &claim.receipt_payload.authorization;
            (
                receipt_session_contract_id(
                    authorized.policy.payer_wallet_address,
                    &authorized.policy.authorization_nonce,
                ),
                authorized,
            )
        })
        .collect::<Vec<_>>();
    registrations.sort_by_key(|(session_id, _)| *session_id);
    registrations.dedup_by_key(|(session_id, _)| *session_id);

    let authorization_tokens = registrations
        .iter()
        .map(|(_, authorized)| {
            let policy = &authorized.policy;
            Token::Tuple(vec![
                Token::Uint(U256::from(policy.protocol_version)),
                Token::Address(policy.payer_wallet_address),
                Token::Bytes(policy.payer_session_public_key.as_slice().to_vec()),
                Token::Uint(U256::from(policy.max_total_charge_wei)),
                Token::Uint(U256::from(policy.valid_from_unix_s)),
                Token::Uint(U256::from(policy.expires_at_unix_s)),
                Token::FixedBytes(policy.authorization_nonce.to_vec()),
            ])
        })
        .collect::<Vec<_>>();
    let signature_tokens = registrations
        .iter()
        .map(|(_, authorized)| {
            let mut signature = Vec::with_capacity(65);
            signature.extend_from_slice(&authorized.wallet_signature.r);
            signature.extend_from_slice(&authorized.wallet_signature.s);
            signature.push(authorized.wallet_signature.v + 27);
            Token::Bytes(signature)
        })
        .collect::<Vec<_>>();
    let claim_tokens = batch
        .contract_claims
        .iter()
        .map(|claim| {
            Token::Tuple(vec![
                Token::FixedBytes(claim.claim_id.to_vec()),
                Token::FixedBytes(claim.session_id.to_vec()),
                Token::Address(claim.payer_wallet_address),
                Token::Address(claim.payout_wallet_address),
                Token::Uint(U256::from(claim.cumulative_charge_wei)),
            ])
        })
        .collect::<Vec<_>>();
    let mut calldata = SUBMIT_BATCH_WITH_SESSIONS_SIGNATURE.keccak256()[..4].to_vec();
    calldata.extend(ethabi::encode(&[
        Token::Array(authorization_tokens),
        Token::Array(signature_tokens),
        Token::Uint(U256::from(batch_sequence)),
        Token::FixedBytes(batch.contract_merkle_root.to_vec()),
        Token::Array(claim_tokens),
    ]));
    calldata
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub_lib::cryptde::CryptDE;
    use crate::sub_lib::receipt_settlement::ReceiptSettlementClaim;
    use crate::sub_lib::service_receipt::{
        make_accounting_commitment, ProviderSettlementPolicy, ReceiptSessionPolicy, ServiceKind,
        ServiceReceipt, ServiceReceiptPayload_0v1,
    };
    use crate::test_utils::make_paying_wallet;
    use ethereum_types::Address;
    use libsecp256k1::{sign, SecretKey};
    use masq_lib::test_utils::mock_blockchain_client_server::MBCSBuilder;
    use masq_lib::utils::find_free_port;
    use rustc_hex::FromHex;
    use std::net::Ipv4Addr;
    use std::str::FromStr;

    fn make_real_batch_cbor() -> (Chain, Address, Vec<u8>) {
        let chain = Chain::Dev;
        let provider = CryptDEReal::new(chain);
        let payer_session = CryptDEReal::new(chain);
        let payer_wallet = make_paying_wallet(b"batcher manifest payer");
        let payout_wallet = make_paying_wallet(b"batcher manifest payout");
        let settlement_contract = Address::from([0x77; 20]);
        let route_epoch = [0x42; 32];
        let authorization = ReceiptSessionPolicy::new(
            chain.rec().num_chain_id,
            settlement_contract,
            payer_wallet.address(),
            payer_session.public_key().clone(),
            10_000,
            100,
            200,
            [0x43; 32],
        )
        .authorize(&payer_wallet)
        .unwrap();
        let acknowledged_receipt = ServiceReceipt::new(
            route_epoch,
            1,
            ServiceKind::Exit,
            provider.public_key().clone(),
            make_accounting_commitment(&route_epoch, payer_session.public_key()),
            100,
            10,
            2,
        )
        .sign(&provider)
        .unwrap()
        .acknowledge(&payer_session)
        .unwrap();
        let provider_settlement = ProviderSettlementPolicy::new(
            chain.rec().num_chain_id,
            settlement_contract,
            payout_wallet.address(),
            provider.public_key().clone(),
            100,
            300,
            [0x44; 32],
        )
        .authorize(&payout_wallet, &provider)
        .unwrap();
        let batch = ReceiptSettlementBatch::build_from_accepted(
            vec![(
                ReceiptSettlementClaim::new(
                    ServiceReceiptPayload_0v1 {
                        authorization,
                        acknowledged_receipt,
                    },
                    provider_settlement,
                ),
                125,
            )],
            chain.rec().num_chain_id,
            settlement_contract,
            150,
            &provider,
        )
        .unwrap();
        (
            chain,
            settlement_contract,
            serde_cbor::to_vec(&batch).unwrap(),
        )
    }

    fn make_tracking_manifest(
        code: &[u8],
        batcher_address: Address,
    ) -> SettlementSubmissionManifest {
        let (chain, _, cbor) = make_real_batch_cbor();
        let mut manifest = prepare_submission_manifest(&cbor, chain, 9, 150).unwrap();
        manifest.assurance_mode = "rpc-bound-preflight".to_string();
        manifest.rpc_preflight_opt = Some(SettlementRpcPreflight {
            observed_block_number: 90,
            observed_block_hash: format!("0x{}", "99".repeat(32)),
            observed_block_timestamp_unix_s: 150,
            settlement_contract_code_keccak256: format!(
                "0x{}",
                code.keccak256().to_hex::<String>()
            ),
            batcher_address: format!("{:#x}", batcher_address),
            provider_authorization_remaining_seconds: 150,
            batcher_role_authorized: true,
            submit_batch_simulated: true,
        });
        manifest
    }

    fn make_fixed_tracking_manifest(batcher_address: Address) -> SettlementSubmissionManifest {
        let payer = Address::from([0x33; 20]);
        let authorization_nonce = [0x66; 32];
        let session_id = receipt_session_contract_id(payer, &authorization_nonce);
        let authorization = Token::Tuple(vec![
            Token::Uint(U256::from(1)),
            Token::Address(payer),
            Token::Bytes(vec![0x02, 0x11, 0x22, 0x33]),
            Token::Uint(U256::from(100)),
            Token::Uint(U256::from(100)),
            Token::Uint(U256::from(200)),
            Token::FixedBytes(authorization_nonce.to_vec()),
        ]);
        let mut signature = vec![0x77; 32];
        signature.extend(vec![0x08; 32]);
        signature.push(27);
        let contract_claim = Token::Tuple(vec![
            Token::FixedBytes(vec![0x11; 32]),
            Token::FixedBytes(session_id.to_vec()),
            Token::Address(payer),
            Token::Address(Address::from([0x44; 20])),
            Token::Uint(U256::from(5)),
        ]);
        let mut calldata = SUBMIT_BATCH_WITH_SESSIONS_SIGNATURE.keccak256()[..4].to_vec();
        calldata.extend(ethabi::encode(&[
            Token::Array(vec![authorization]),
            Token::Array(vec![Token::Bytes(signature)]),
            Token::Uint(U256::from(7)),
            Token::FixedBytes(vec![0x55; 32]),
            Token::Array(vec![contract_claim]),
        ]));
        SettlementSubmissionManifest {
            manifest_version: 1,
            assurance_mode: "rpc-bound-preflight".to_string(),
            chain_id: 2,
            settlement_contract: format!("{:#x}", Address::from([0x77; 20])),
            verification_time_unix_s: 150,
            batch_sequence: 7,
            claim_count: 1,
            claim_ids: vec![format!("0x{}", "11".repeat(32))],
            portable_merkle_root: format!("0x{}", "66".repeat(32)),
            contract_merkle_root: format!("0x{}", "55".repeat(32)),
            total_cumulative_charge_wei: "5".to_string(),
            batch_cbor_keccak256: format!("0x{}", "88".repeat(32)),
            calldata_keccak256: format!("0x{}", calldata.keccak256().to_hex::<String>()),
            value_wei: "0".to_string(),
            calldata: format!("0x{}", calldata.to_hex::<String>()),
            rpc_preflight_opt: Some(SettlementRpcPreflight {
                observed_block_number: 90,
                observed_block_hash: format!("0x{}", "99".repeat(32)),
                observed_block_timestamp_unix_s: 150,
                settlement_contract_code_keccak256: format!("0x{}", "aa".repeat(32)),
                batcher_address: format!("{:#x}", batcher_address),
                provider_authorization_remaining_seconds: 150,
                batcher_role_authorized: true,
                submit_batch_simulated: true,
            }),
        }
    }

    fn broadcast_policy() -> SettlementBroadcastPolicy {
        SettlementBroadcastPolicy {
            expected_nonce: U256::from(3),
            maximum_gas_limit: U256::from(500_000),
            maximum_fee_per_gas_wei: U256::from(2_000_000_000u64),
            maximum_priority_fee_per_gas_wei: U256::from(1_000_000_000u64),
            maximum_total_fee_wei: U256::from(1_000_000_000_000_000u64),
        }
    }

    fn sign_manifest_eip1559(
        manifest: &SettlementSubmissionManifest,
        secret_key: &SecretKey,
    ) -> Vec<u8> {
        let destination = Address::from_str(&manifest.settlement_contract[2..]).unwrap();
        let calldata = manifest.calldata[2..].from_hex::<Vec<u8>>().unwrap();
        let unsigned = encode_eip1559_transaction(
            U256::from(manifest.chain_id),
            U256::from(3),
            U256::from(1_000_000_000u64),
            U256::from(2_000_000_000u64),
            U256::from(500_000),
            destination,
            U256::zero(),
            &calldata,
            None,
        );
        let message = Message::parse(&unsigned.keccak256());
        let (signature, recovery_id) = sign(&message, secret_key);
        let signature_bytes = signature.serialize();
        encode_eip1559_transaction(
            U256::from(manifest.chain_id),
            U256::from(3),
            U256::from(1_000_000_000u64),
            U256::from(2_000_000_000u64),
            U256::from(500_000),
            destination,
            U256::zero(),
            &calldata,
            Some((
                recovery_id.serialize(),
                U256::from_big_endian(&signature_bytes[..32]),
                U256::from_big_endian(&signature_bytes[32..]),
            )),
        )
    }

    #[test]
    fn submit_batch_calldata_matches_ethers_abi_vector() {
        let calldata = submit_batch_calldata(
            7,
            [0x55; 32],
            &[ReceiptSettlementContractClaim {
                claim_id: [0x11; 32],
                session_id: [0x22; 32],
                payer_wallet_address: Address::from([0x33; 20]),
                payout_wallet_address: Address::from([0x44; 20]),
                cumulative_charge_wei: 5,
            }],
        );
        let expected: Vec<u8> = "a218920f000000000000000000000000000000000000000000000000000000000000000755555555555555555555555555555555555555555555555555555555555555550000000000000000000000000000000000000000000000000000000000000060000000000000000000000000000000000000000000000000000000000000000111111111111111111111111111111111111111111111111111111111111111112222222222222222222222222222222222222222222222222222222222222222000000000000000000000000333333333333333333333333333333333333333300000000000000000000000044444444444444444444444444444444444444440000000000000000000000000000000000000000000000000000000000000005"
            .from_hex()
            .unwrap();

        assert_eq!(calldata, expected);
    }

    #[test]
    fn manifest_is_prepared_only_after_real_signature_reconstruction() {
        let (chain, _, cbor) = make_real_batch_cbor();

        let manifest = prepare_submission_manifest(&cbor, chain, 9, 150).unwrap();

        assert_eq!(manifest.manifest_version, 1);
        assert_eq!(manifest.assurance_mode, "offline-manual-sequence");
        assert_eq!(manifest.chain_id, chain.rec().num_chain_id);
        assert_eq!(manifest.batch_sequence, 9);
        assert_eq!(manifest.claim_count, 1);
        assert_eq!(manifest.claim_ids.len(), 1);
        assert!(manifest.calldata.starts_with(&format!(
            "0x{}",
            SUBMIT_BATCH_WITH_SESSIONS_SIGNATURE.keccak256()[..4].to_hex::<String>()
        )));
        assert!(!manifest.calldata.starts_with("0xa218920f"));
        assert_ne!(
            manifest.batch_cbor_keccak256,
            format!("0x{}", [0; 32].to_hex::<String>())
        );
        assert_ne!(
            manifest.calldata_keccak256,
            format!("0x{}", [0; 32].to_hex::<String>())
        );
        assert_eq!(manifest.rpc_preflight_opt, None);
    }

    #[test]
    fn preflight_time_policy_rejects_stale_future_and_near_expiry_state() {
        assert_eq!(validate_rpc_block_freshness(700, 1_000), Ok(()));
        assert_eq!(validate_rpc_block_freshness(1_030, 1_000), Ok(()));
        assert_eq!(
            validate_rpc_block_freshness(699, 1_000),
            Err(SettlementBatcherError::RpcBlockStale {
                block_unix_s: 699,
                now_unix_s: 1_000,
            })
        );
        assert_eq!(
            validate_rpc_block_freshness(1_031, 1_000),
            Err(SettlementBatcherError::RpcBlockInFuture {
                block_unix_s: 1_031,
                now_unix_s: 1_000,
            })
        );

        let (chain, _, cbor) = make_real_batch_cbor();
        let batch = decode_canonical_batch(&cbor, chain).unwrap();
        assert_eq!(
            provider_authorization_remaining_seconds(&batch, 240),
            Ok(60)
        );
        assert_eq!(
            provider_authorization_remaining_seconds(&batch, 241),
            Err(
                SettlementBatcherError::ProviderAuthorizationExpiryTooClose {
                    remaining_seconds: 59,
                }
            )
        );
    }

    #[test]
    fn tracking_manifest_is_revalidated_instead_of_trusted_as_json() {
        let code = vec![0x60, 0x00];
        let batcher_address = Address::from([0x88; 20]);
        let manifest = make_tracking_manifest(&code, batcher_address);

        let validated = validate_tracking_manifest(&manifest).unwrap();

        assert_eq!(validated.batcher_address, batcher_address);
        assert_eq!(
            validated.calldata,
            parse_prefixed_hex(&manifest.calldata, "calldata").unwrap()
        );
        let mut tampered = manifest.clone();
        tampered.total_cumulative_charge_wei = "21".to_string();
        assert_eq!(
            validate_tracking_manifest(&tampered).err(),
            Some(SettlementBatcherError::TrackingManifest(
                "calldata cumulative charge total does not match manifest".to_string()
            ))
        );
        let mut offline = manifest.clone();
        offline.assurance_mode = "offline-manual-sequence".to_string();
        assert_eq!(
            validate_tracking_manifest(&offline).err(),
            Some(SettlementBatcherError::TrackingManifest(
                "only rpc-bound-preflight manifests can be tracked".to_string()
            ))
        );
        let mut tampered_calldata = manifest;
        tampered_calldata.calldata.push_str("00");
        assert_eq!(
            validate_tracking_manifest(&tampered_calldata).err(),
            Some(SettlementBatcherError::TrackingManifest(
                "calldata hash does not match calldata".to_string()
            ))
        );
    }

    #[test]
    fn signed_eip1559_decoder_matches_independent_ethers_fixture_and_policy() {
        let batcher_address =
            Address::from_str("19e7e376e7c213b7e7e7e46cc70a5dd086daff2a").unwrap();
        let manifest = make_fixed_tracking_manifest(batcher_address);
        let raw_transaction = "02f903f10203843b9aca0084773594008307a12094777777777777777777777777777777777777777780b90384968bfa4100000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000007555555555555555555555555555555555555555555555555555555555555555500000000000000000000000000000000000000000000000000000000000002c0000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000001000000000000000000000000333333333333333333333333333333333333333300000000000000000000000000000000000000000000000000000000000000e00000000000000000000000000000000000000000000000000000000000000064000000000000000000000000000000000000000000000000000000000000006400000000000000000000000000000000000000000000000000000000000000c8666666666666666666666666666666666666666666666666666666666666666600000000000000000000000000000000000000000000000000000000000000040211223300000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000041777777777777777777777777777777777777777777777777777777777777777708080808080808080808080808080808080808080808080808080808080808081b00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000111111111111111111111111111111111111111111111111111111111111111112bbb8d3dfcb4e18b5050b1d551a5c592a038048e3d29216b8e253f0ea43aa5dc000000000000000000000000333333333333333333333333333333333333333300000000000000000000000044444444444444444444444444444444444444440000000000000000000000000000000000000000000000000000000000000005c001a021622f39db2c8a2d4c4e22d3433bdb80f6b73020da3cbee5af104179a08c4f57a06ba17161cd43962b0e555d36dc13269200da7e1411e4ecc1365fb034cb104bac"
            .from_hex::<Vec<u8>>()
            .unwrap();

        let verified =
            verify_signed_eip1559_submission(&manifest, &raw_transaction, &broadcast_policy())
                .unwrap();

        assert_eq!(verified.signer_address, batcher_address);
        assert_eq!(verified.nonce, U256::from(3));
        assert_eq!(verified.gas_limit, U256::from(500_000));
        assert_eq!(
            verified.transaction_hash,
            H256::from_str("b0a3705fdbe95a3d0f58d0f0d10725fdf10bb42ee4e8164bfddb73366775c2c9")
                .unwrap()
        );
        let mut restrictive_policy = broadcast_policy();
        restrictive_policy.maximum_total_fee_wei -= U256::one();
        assert_eq!(
            verify_signed_eip1559_submission(&manifest, &raw_transaction, &restrictive_policy)
                .err(),
            Some(SettlementBatcherError::SubmissionPolicy(
                "maximum total fee exceeds operator policy".to_string()
            ))
        );
        let mut legacy_envelope = raw_transaction;
        legacy_envelope[0] = 0xf9;
        assert_eq!(
            verify_signed_eip1559_submission(&manifest, &legacy_envelope, &broadcast_policy())
                .err(),
            Some(SettlementBatcherError::SignedTransaction(
                "only an EIP-1559 type-2 envelope is accepted".to_string()
            ))
        );
    }

    #[test]
    fn rpc_preflight_binds_code_role_sequence_and_simulation_to_one_block_hash() {
        let (chain, settlement_contract, cbor) = make_real_batch_cbor();
        let batcher_address = Address::from([0x88; 20]);
        let port = find_free_port();
        let block_hash = format!("0x{}", "11".repeat(32));
        let block = serde_json::json!({
            "hash": block_hash,
            "parentHash": format!("0x{}", "22".repeat(32)),
            "sha3Uncles": format!("0x{}", "33".repeat(32)),
            "miner": format!("0x{}", "44".repeat(20)),
            "stateRoot": format!("0x{}", "55".repeat(32)),
            "transactionsRoot": format!("0x{}", "66".repeat(32)),
            "receiptsRoot": format!("0x{}", "77".repeat(32)),
            "number": "0x64",
            "gasUsed": "0x0",
            "gasLimit": "0x1c9c380",
            "extraData": "0x",
            "logsBloom": null,
            "timestamp": "0x96",
            "difficulty": "0x0",
            "totalDifficulty": "0x0",
            "sealFields": [],
            "uncles": [],
            "transactions": [],
            "size": "0x0",
            "mixHash": null,
            "nonce": null
        });
        let server = MBCSBuilder::new(port)
            .ok_response("0x2", 1)
            .ok_response("0x64", 1)
            .ok_response(block, 1)
            .ok_response("0x6000", 1)
            .ok_response(format!("0x{:064x}", 9), 1)
            .ok_response(format!("0x{:064x}", 1), 1)
            .ok_response("0x", 1)
            .start();

        let manifest = prepare_rpc_bound_submission_manifest(
            &cbor,
            chain,
            batcher_address,
            150,
            &format!("http://{}:{}", Ipv4Addr::LOCALHOST, port),
        )
        .unwrap();

        assert_eq!(manifest.batch_sequence, 9);
        assert_eq!(manifest.assurance_mode, "rpc-bound-preflight");
        assert_eq!(
            manifest.settlement_contract,
            format!("{:#x}", settlement_contract)
        );
        assert_eq!(
            manifest.rpc_preflight_opt,
            Some(SettlementRpcPreflight {
                observed_block_number: 100,
                observed_block_hash: block_hash.clone(),
                observed_block_timestamp_unix_s: 150,
                settlement_contract_code_keccak256: format!(
                    "0x{}",
                    vec![0x60, 0x00].keccak256().to_hex::<String>()
                ),
                batcher_address: format!("{:#x}", batcher_address),
                provider_authorization_remaining_seconds: 150,
                batcher_role_authorized: true,
                submit_batch_simulated: true,
            })
        );
        let requests = server.requests();
        let calls = requests
            .iter()
            .filter(|request| request.contains("eth_call"))
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|request| request.contains(&block_hash)));
        assert!(calls
            .iter()
            .all(|request| request.contains("requireCanonical")));
        let code_request = requests
            .iter()
            .find(|request| request.contains("eth_getCode"))
            .unwrap();
        assert!(code_request.contains(&block_hash));
        assert!(code_request.contains("requireCanonical"));
    }

    #[test]
    fn signed_submission_is_freshly_preflighted_with_signed_gas_before_broadcast() {
        let (chain, settlement_contract, cbor) = make_real_batch_cbor();
        let secret_key = SecretKey::parse(&[0x11; 32]).unwrap();
        let batcher_address = ethereum_address(&PublicKey::from_secret_key(&secret_key));
        let mut manifest = prepare_submission_manifest(&cbor, chain, 9, 150).unwrap();
        manifest.assurance_mode = "rpc-bound-preflight".to_string();
        manifest.rpc_preflight_opt = Some(SettlementRpcPreflight {
            observed_block_number: 90,
            observed_block_hash: format!("0x{}", "99".repeat(32)),
            observed_block_timestamp_unix_s: 150,
            settlement_contract_code_keccak256: format!(
                "0x{}",
                [0x60, 0x00].keccak256().to_hex::<String>()
            ),
            batcher_address: format!("{:#x}", batcher_address),
            provider_authorization_remaining_seconds: 150,
            batcher_role_authorized: true,
            submit_batch_simulated: true,
        });
        let raw_transaction = sign_manifest_eip1559(&manifest, &secret_key);
        let expected_transaction_hash = H256::from_slice(&raw_transaction.keccak256());
        let port = find_free_port();
        let block_hash = format!("0x{}", "11".repeat(32));
        let block = serde_json::json!({
            "hash": block_hash,
            "parentHash": format!("0x{}", "22".repeat(32)),
            "sha3Uncles": format!("0x{}", "33".repeat(32)),
            "miner": format!("0x{}", "44".repeat(20)),
            "stateRoot": format!("0x{}", "55".repeat(32)),
            "transactionsRoot": format!("0x{}", "66".repeat(32)),
            "receiptsRoot": format!("0x{}", "77".repeat(32)),
            "number": "0x64",
            "gasUsed": "0x0",
            "gasLimit": "0x1c9c380",
            "extraData": "0x",
            "logsBloom": null,
            "timestamp": "0x96",
            "difficulty": "0x0",
            "totalDifficulty": "0x0",
            "sealFields": [],
            "uncles": [],
            "transactions": [],
            "size": "0x0",
            "mixHash": null,
            "nonce": null
        });
        let server = MBCSBuilder::new(port)
            .ok_response("0x2", 1)
            .ok_response("0x64", 1)
            .ok_response(block, 1)
            .ok_response("0x6000", 1)
            .ok_response(format!("0x{:064x}", 9), 1)
            .ok_response(format!("0x{:064x}", 1), 1)
            .ok_response("0x", 1)
            .ok_response("0x3", 1)
            .ok_response(format!("{:#x}", expected_transaction_hash), 1)
            .start();

        let result = broadcast_signed_eip1559_submission(
            &cbor,
            chain,
            &manifest,
            &raw_transaction,
            &broadcast_policy(),
            150,
            &format!("http://{}:{}", Ipv4Addr::LOCALHOST, port),
        )
        .unwrap();

        assert_eq!(
            result.transaction_hash,
            format!("{:#x}", expected_transaction_hash)
        );
        assert_eq!(result.signer_address, format!("{:#x}", batcher_address));
        assert_eq!(result.current_preflight.observed_block_hash, block_hash);
        let requests = server.requests();
        let simulation = requests
            .iter()
            .find(|request| request.contains("eth_call") && request.contains(&manifest.calldata))
            .unwrap();
        assert!(simulation.contains("0x7a120"));
        let broadcast = requests
            .iter()
            .find(|request| request.contains("eth_sendRawTransaction"))
            .unwrap();
        let nonce_request_index = requests
            .iter()
            .position(|request| request.contains("eth_getTransactionCount"))
            .unwrap();
        let broadcast_request_index = requests
            .iter()
            .position(|request| request.contains("eth_sendRawTransaction"))
            .unwrap();
        assert!(requests[nonce_request_index].contains("pending"));
        assert!(requests[nonce_request_index].contains(&format!("{:#x}", batcher_address)));
        assert!(nonce_request_index < broadcast_request_index);
        assert!(broadcast.contains(&format!("0x{}", raw_transaction.to_hex::<String>())));
        assert_eq!(
            manifest.settlement_contract,
            format!("{:#x}", settlement_contract)
        );
    }

    #[test]
    fn transaction_tracker_requires_matching_canonical_receipt_code_and_event() {
        let code = vec![0x60, 0x00];
        let batcher_address = Address::from([0x88; 20]);
        let manifest = make_tracking_manifest(&code, batcher_address);
        let settlement_contract = Address::from_str(&manifest.settlement_contract[2..]).unwrap();
        let transaction_hash = H256::from([0xaa; 32]);
        let included_block_hash = H256::from([0xbb; 32]);
        let contract_merkle_root = H256::from_str(&manifest.contract_merkle_root[2..]).unwrap();
        let event_data = ethabi::encode(&[
            Token::Uint(U256::from(manifest.claim_count)),
            Token::Uint(U256::from(17)),
        ]);
        let transaction = serde_json::json!({
            "hash": format!("{:#x}", transaction_hash),
            "nonce": "0x1",
            "blockHash": format!("{:#x}", included_block_hash),
            "blockNumber": "0x64",
            "transactionIndex": "0x0",
            "from": format!("{:#x}", batcher_address),
            "to": format!("{:#x}", settlement_contract),
            "value": "0x0",
            "gasPrice": "0x1",
            "gas": "0xf4240",
            "input": manifest.calldata
        });
        let receipt = serde_json::json!({
            "transactionHash": format!("{:#x}", transaction_hash),
            "transactionIndex": "0x0",
            "blockHash": format!("{:#x}", included_block_hash),
            "blockNumber": "0x64",
            "cumulativeGasUsed": "0x5208",
            "gasUsed": "0x5208",
            "contractAddress": null,
            "logs": [{
                "address": format!("{:#x}", settlement_contract),
                "topics": [
                    format!("0x{}", BATCH_SETTLED_EVENT_SIGNATURE.keccak256().to_hex::<String>()),
                    format!("{:#x}", u64_topic(manifest.batch_sequence)),
                    format!("{:#x}", contract_merkle_root)
                ],
                "data": format!("0x{}", event_data.to_hex::<String>()),
                "blockHash": format!("{:#x}", included_block_hash),
                "blockNumber": "0x64",
                "transactionHash": format!("{:#x}", transaction_hash),
                "transactionIndex": "0x0",
                "logIndex": "0x0",
                "transactionLogIndex": "0x0",
                "logType": "mined",
                "removed": false
            }],
            "status": "0x1",
            "root": null,
            "logsBloom": format!("0x{}", "00".repeat(256))
        });
        let canonical_block = serde_json::json!({
            "hash": format!("{:#x}", included_block_hash),
            "parentHash": format!("0x{}", "22".repeat(32)),
            "sha3Uncles": format!("0x{}", "33".repeat(32)),
            "miner": format!("0x{}", "44".repeat(20)),
            "stateRoot": format!("0x{}", "55".repeat(32)),
            "transactionsRoot": format!("0x{}", "66".repeat(32)),
            "receiptsRoot": format!("0x{}", "77".repeat(32)),
            "number": "0x64",
            "gasUsed": "0x5208",
            "gasLimit": "0x1c9c380",
            "extraData": "0x",
            "logsBloom": null,
            "timestamp": "0x96",
            "difficulty": "0x0",
            "totalDifficulty": "0x0",
            "sealFields": [],
            "uncles": [],
            "transactions": [],
            "size": "0x0",
            "mixHash": null,
            "nonce": null
        });
        let port = find_free_port();
        let server = MBCSBuilder::new(port)
            .ok_response("0x2", 1)
            .ok_response(transaction, 1)
            .ok_response(receipt, 1)
            .ok_response("0xa4", 1)
            .ok_response(canonical_block, 1)
            .ok_response(format!("0x{}", code.to_hex::<String>()), 1)
            .start();

        let observation = track_submission_transaction(
            &manifest,
            transaction_hash,
            64,
            &format!("http://{}:{}", Ipv4Addr::LOCALHOST, port),
        )
        .unwrap();

        assert_eq!(observation.lifecycle_state, "finalized");
        assert_eq!(observation.latest_block_number, Some(164));
        assert_eq!(observation.included_block_number, Some(100));
        assert_eq!(observation.confirmation_depth, Some(64));
        assert_eq!(observation.gas_used, Some("21000".to_string()));
        assert_eq!(observation.batch_total_delta_wei, Some("17".to_string()));
        let code_request = server
            .requests()
            .into_iter()
            .find(|request| request.contains("eth_getCode"))
            .unwrap();
        assert!(code_request.contains(&format!("{:#x}", included_block_hash)));
        assert!(code_request.contains("requireCanonical"));
    }

    #[test]
    fn transaction_tracker_reports_reorg_before_trusting_code_or_events() {
        let code = vec![0x60, 0x00];
        let batcher_address = Address::from([0x88; 20]);
        let manifest = make_tracking_manifest(&code, batcher_address);
        let settlement_contract = Address::from_str(&manifest.settlement_contract[2..]).unwrap();
        let transaction_hash = H256::from([0xaa; 32]);
        let receipt_block_hash = H256::from([0xbb; 32]);
        let canonical_block_hash = H256::from([0xcc; 32]);
        let transaction = serde_json::json!({
            "hash": format!("{:#x}", transaction_hash),
            "nonce": "0x1",
            "blockHash": format!("{:#x}", receipt_block_hash),
            "blockNumber": "0x64",
            "transactionIndex": "0x0",
            "from": format!("{:#x}", batcher_address),
            "to": format!("{:#x}", settlement_contract),
            "value": "0x0",
            "gasPrice": "0x1",
            "gas": "0xf4240",
            "input": manifest.calldata
        });
        let receipt = serde_json::json!({
            "transactionHash": format!("{:#x}", transaction_hash),
            "transactionIndex": "0x0",
            "blockHash": format!("{:#x}", receipt_block_hash),
            "blockNumber": "0x64",
            "cumulativeGasUsed": "0x5208",
            "gasUsed": "0x5208",
            "contractAddress": null,
            "logs": [],
            "status": "0x1",
            "root": null,
            "logsBloom": format!("0x{}", "00".repeat(256))
        });
        let canonical_block = serde_json::json!({
            "hash": format!("{:#x}", canonical_block_hash),
            "parentHash": format!("0x{}", "22".repeat(32)),
            "sha3Uncles": format!("0x{}", "33".repeat(32)),
            "miner": format!("0x{}", "44".repeat(20)),
            "stateRoot": format!("0x{}", "55".repeat(32)),
            "transactionsRoot": format!("0x{}", "66".repeat(32)),
            "receiptsRoot": format!("0x{}", "77".repeat(32)),
            "number": "0x64",
            "gasUsed": "0x5208",
            "gasLimit": "0x1c9c380",
            "extraData": "0x",
            "logsBloom": null,
            "timestamp": "0x96",
            "difficulty": "0x0",
            "totalDifficulty": "0x0",
            "sealFields": [],
            "uncles": [],
            "transactions": [],
            "size": "0x0",
            "mixHash": null,
            "nonce": null
        });
        let port = find_free_port();
        let server = MBCSBuilder::new(port)
            .ok_response("0x2", 1)
            .ok_response(transaction, 1)
            .ok_response(receipt, 1)
            .ok_response("0xa4", 1)
            .ok_response(canonical_block, 1)
            .start();

        let observation = track_submission_transaction(
            &manifest,
            transaction_hash,
            64,
            &format!("http://{}:{}", Ipv4Addr::LOCALHOST, port),
        )
        .unwrap();

        assert_eq!(observation.lifecycle_state, "reorged");
        assert_eq!(observation.included_block_number, Some(100));
        assert_eq!(
            observation.included_block_hash,
            Some(format!("{:#x}", receipt_block_hash))
        );
        assert_eq!(observation.confirmation_depth, None);
        assert!(server
            .requests()
            .iter()
            .all(|request| !request.contains("eth_getCode")));
    }
}
