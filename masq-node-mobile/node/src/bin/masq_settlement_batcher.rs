// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use clap::{App, AppSettings, Arg, ArgGroup, ArgMatches, SubCommand};
use ethereum_types::{Address, H256, U256};
use ethsign_crypto::Keccak256;
use masq_lib::blockchains::chains::chain_from_chain_identifier_opt;
use node_lib::sub_lib::settlement_batcher::{
    broadcast_signed_eip1559_submission, prepare_rpc_bound_submission_manifest,
    prepare_submission_manifest, track_submission_transaction, verify_signed_eip1559_submission,
    SettlementBatcherError, SettlementBroadcastPolicy, SettlementSubmissionManifest,
    SettlementTransactionObservation, MAX_CONFIRMATION_DEPTH, MAX_SIGNED_EIP1559_TRANSACTION_BYTES,
    MIN_CONFIRMATION_DEPTH,
};
use rustc_hex::{FromHex, ToHex};
use serde_derive::{Deserialize, Serialize};
use serde_json::json;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BATCH_CBOR_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TRACKING_JSON_FILE_BYTES: u64 = 1024 * 1024;
const MAX_LIFECYCLE_TRANSITIONS: usize = 256;
const SUPPORTED_CHAINS: &[&str] = &[
    "base-mainnet",
    "base-sepolia",
    "polygon-mainnet",
    "polygon-amoy",
    "eth-mainnet",
    "eth-ropsten",
    "dev",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettlementLifecycleTransition {
    observed_at_unix_s: u64,
    observation: SettlementTransactionObservation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettlementTrackingState {
    state_version: u16,
    manifest_keccak256: String,
    transaction_hash: String,
    first_checked_at_unix_s: u64,
    last_checked_at_unix_s: u64,
    check_count: u64,
    lifecycle_transitions: Vec<SettlementLifecycleTransition>,
    observation: SettlementTransactionObservation,
}

struct TrackingStateLock {
    path: std::path::PathBuf,
    parent: std::path::PathBuf,
    active: bool,
}

impl TrackingStateLock {
    fn release(mut self) -> Result<(), String> {
        fs::remove_file(&self.path)
            .map_err(|error| format!("cannot release tracking state lock: {}", error))?;
        sync_parent_directory(&self.parent)?;
        self.active = false;
        Ok(())
    }
}

impl Drop for TrackingStateLock {
    fn drop(&mut self) {
        if self.active {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn app() -> App<'static, 'static> {
    App::new("masq-settlement-batcher")
        .about("Independently verifies a MASQ batch and emits RPC-preflighted unsigned calldata.")
        .setting(AppSettings::SubcommandsNegateReqs)
        .group(
            ArgGroup::with_name("sequence-source")
                .args(&["batch-sequence", "rpc-url"])
                .required(true)
                .multiple(false),
        )
        .arg(
            Arg::with_name("batch-cbor-file")
                .long("batch-cbor-file")
                .value_name("PATH")
                .takes_value(true)
                .required(true)
                .help("Raw canonical CBOR, or one trimmed 0x-prefixed CBOR hex value."),
        )
        .arg(
            Arg::with_name("chain")
                .long("chain")
                .value_name("CHAIN")
                .takes_value(true)
                .required(true)
                .possible_values(SUPPORTED_CHAINS)
                .help("Expected MASQ chain; its numeric ID must match the signed batch."),
        )
        .arg(
            Arg::with_name("batch-sequence")
                .long("batch-sequence")
                .value_name("UINT64")
                .takes_value(true)
                .validator(validate_u64)
                .help("Offline-only sequence; RPC preflight should normally derive this value."),
        )
        .arg(
            Arg::with_name("rpc-url")
                .long("rpc-url")
                .value_name("URL")
                .takes_value(true)
                .help("RPC used to bind code, role, sequence and simulation to one block hash."),
        )
        .arg(
            Arg::with_name("batcher-address")
                .long("batcher-address")
                .value_name("0x-ADDRESS")
                .takes_value(true)
                .requires("rpc-url")
                .validator(validate_address)
                .help("External signer/HSM address expected to hold BATCHER_ROLE."),
        )
        .arg(
            Arg::with_name("verification-time-unix-s")
                .long("verification-time-unix-s")
                .value_name("UINT64")
                .takes_value(true)
                .conflicts_with("rpc-url")
                .validator(validate_u64)
                .help("Offline audit time; RPC mode always uses its fresh block timestamp."),
        )
        .subcommand(
            SubCommand::with_name("track")
                .about("Tracks an externally broadcast settlement transaction without signing it.")
                .arg(
                    Arg::with_name("manifest-file")
                        .long("manifest-file")
                        .value_name("PATH")
                        .takes_value(true)
                        .required(true)
                        .help("RPC-bound submission manifest emitted before external signing."),
                )
                .arg(
                    Arg::with_name("tx-hash")
                        .long("tx-hash")
                        .value_name("0x-HASH")
                        .takes_value(true)
                        .required(true)
                        .validator(validate_hash)
                        .help("Hash returned by the external wallet/HSM broadcaster."),
                )
                .arg(
                    Arg::with_name("rpc-url")
                        .long("rpc-url")
                        .value_name("URL")
                        .takes_value(true)
                        .required(true)
                        .help("RPC used to verify transaction, receipt, code and canonicality."),
                )
                .arg(
                    Arg::with_name("confirmation-depth")
                        .long("confirmation-depth")
                        .value_name("BLOCKS")
                        .takes_value(true)
                        .default_value("64")
                        .validator(validate_confirmation_depth)
                        .help("Required canonical block depth before reporting finalized."),
                )
                .arg(
                    Arg::with_name("state-file")
                        .long("state-file")
                        .value_name("PATH")
                        .takes_value(true)
                        .required(true)
                        .help("Crash-durable local lifecycle record, atomically replaced."),
                ),
        )
        .subcommand(
            SubCommand::with_name("broadcast")
                .about(
                    "Revalidates and relays one externally signed EIP-1559 settlement transaction.",
                )
                .arg(
                    Arg::with_name("batch-cbor-file")
                        .long("batch-cbor-file")
                        .value_name("PATH")
                        .takes_value(true)
                        .required(true)
                        .help("The same canonical CBOR used to produce the RPC-bound manifest."),
                )
                .arg(
                    Arg::with_name("manifest-file")
                        .long("manifest-file")
                        .value_name("PATH")
                        .takes_value(true)
                        .required(true)
                        .help("RPC-bound submission manifest reviewed before external signing."),
                )
                .arg(
                    Arg::with_name("signed-transaction-file")
                        .long("signed-transaction-file")
                        .value_name("PATH")
                        .takes_value(true)
                        .required(true)
                        .help("Raw type-2 bytes, or one trimmed 0x-prefixed raw transaction."),
                )
                .arg(
                    Arg::with_name("chain")
                        .long("chain")
                        .value_name("CHAIN")
                        .takes_value(true)
                        .required(true)
                        .possible_values(SUPPORTED_CHAINS)
                        .help("Expected chain; it must match batch, manifest and signature."),
                )
                .arg(
                    Arg::with_name("rpc-url")
                        .long("rpc-url")
                        .value_name("URL")
                        .takes_value(true)
                        .required(true)
                        .help("RPC used for fresh canonical preflight, relay and observation."),
                )
                .arg(
                    Arg::with_name("expected-nonce")
                        .long("expected-nonce")
                        .value_name("UINT256")
                        .takes_value(true)
                        .required(true)
                        .validator(validate_u256)
                        .help("Exact externally coordinated nonce; RPC pending nonce must match."),
                )
                .arg(
                    Arg::with_name("max-gas-limit")
                        .long("max-gas-limit")
                        .value_name("UINT256")
                        .takes_value(true)
                        .required(true)
                        .validator(validate_nonzero_u256)
                        .help("Hard upper bound for the signed gas limit."),
                )
                .arg(
                    Arg::with_name("max-fee-per-gas-wei")
                        .long("max-fee-per-gas-wei")
                        .value_name("UINT256")
                        .takes_value(true)
                        .required(true)
                        .validator(validate_nonzero_u256)
                        .help("Hard upper bound for signed maxFeePerGas."),
                )
                .arg(
                    Arg::with_name("max-priority-fee-per-gas-wei")
                        .long("max-priority-fee-per-gas-wei")
                        .value_name("UINT256")
                        .takes_value(true)
                        .required(true)
                        .validator(validate_u256)
                        .help("Hard upper bound for signed maxPriorityFeePerGas; zero is allowed."),
                )
                .arg(
                    Arg::with_name("max-total-fee-wei")
                        .long("max-total-fee-wei")
                        .value_name("UINT256")
                        .takes_value(true)
                        .required(true)
                        .validator(validate_nonzero_u256)
                        .help("Hard upper bound for gasLimit multiplied by maxFeePerGas."),
                )
                .arg(
                    Arg::with_name("confirmation-depth")
                        .long("confirmation-depth")
                        .value_name("BLOCKS")
                        .takes_value(true)
                        .default_value("64")
                        .validator(validate_confirmation_depth)
                        .help("Canonical block depth recorded by immediate and later tracking."),
                )
                .arg(
                    Arg::with_name("state-file")
                        .long("state-file")
                        .value_name("PATH")
                        .takes_value(true)
                        .required(true)
                        .help("Crash-durable intent and lifecycle record, atomically replaced."),
                ),
        )
}

fn validate_u64(value: String) -> Result<(), String> {
    value
        .parse::<u64>()
        .map(|_| ())
        .map_err(|_| "must be an unsigned 64-bit integer".to_string())
}

fn validate_address(value: String) -> Result<(), String> {
    if value.len() != 42
        || !value.starts_with("0x")
        || !value[2..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err("must be exactly 20 bytes of 0x-prefixed hexadecimal".to_string());
    }
    Address::from_str(&value[2..])
        .map(|_| ())
        .map_err(|_| "must be a valid Ethereum address".to_string())
}

fn validate_hash(value: String) -> Result<(), String> {
    if value.len() != 66
        || !value.starts_with("0x")
        || !value[2..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err("must be exactly 32 bytes of 0x-prefixed hexadecimal".to_string());
    }
    let hash = H256::from_str(&value[2..]).map_err(|_| "must be a valid hash".to_string())?;
    if hash == H256::zero() {
        return Err("must not be the zero hash".to_string());
    }
    Ok(())
}

fn validate_confirmation_depth(value: String) -> Result<(), String> {
    let depth = value
        .parse::<u64>()
        .map_err(|_| "must be an unsigned 64-bit integer".to_string())?;
    if !(MIN_CONFIRMATION_DEPTH..=MAX_CONFIRMATION_DEPTH).contains(&depth) {
        return Err(format!(
            "must be between {} and {} blocks",
            MIN_CONFIRMATION_DEPTH, MAX_CONFIRMATION_DEPTH
        ));
    }
    Ok(())
}

fn validate_u256(value: String) -> Result<(), String> {
    U256::from_dec_str(&value)
        .map(|_| ())
        .map_err(|_| "must be a base-10 unsigned 256-bit integer".to_string())
}

fn validate_nonzero_u256(value: String) -> Result<(), String> {
    let parsed = U256::from_dec_str(&value)
        .map_err(|_| "must be a base-10 unsigned 256-bit integer".to_string())?;
    if parsed.is_zero() {
        Err("must be greater than zero".to_string())
    } else {
        Ok(())
    }
}

fn system_now_unix_s() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before Unix epoch".to_string())
        .map(|duration| duration.as_secs())
}

fn read_limited_regular_file(path: &Path, label: &str, limit: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {}", label, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("{} path is not a regular non-symlink file", label));
    }
    if metadata.len() > limit {
        return Err(format!("{} exceeds the {}-byte safety limit", label, limit));
    }
    let contents = fs::read(path).map_err(|error| format!("cannot read {}: {}", label, error))?;
    if contents.is_empty() {
        return Err(format!("{} is empty", label));
    }
    Ok(contents)
}

fn read_batch_cbor(path: &Path) -> Result<Vec<u8>, String> {
    let contents = read_limited_regular_file(path, "batch CBOR file", MAX_BATCH_CBOR_FILE_BYTES)?;
    if let Ok(text) = std::str::from_utf8(&contents) {
        let trimmed = text.trim();
        if let Some(hex) = trimmed.strip_prefix("0x") {
            if hex.is_empty() || hex.len() % 2 != 0 {
                return Err("0x-prefixed batch CBOR must contain whole bytes".to_string());
            }
            return hex
                .from_hex::<Vec<u8>>()
                .map_err(|_| "0x-prefixed batch CBOR contains non-hexadecimal data".to_string());
        }
    }
    Ok(contents)
}

fn read_submission_manifest(path: &Path) -> Result<(SettlementSubmissionManifest, String), String> {
    let manifest_bytes =
        read_limited_regular_file(path, "submission manifest", MAX_TRACKING_JSON_FILE_BYTES)?;
    let manifest = serde_json::from_slice::<SettlementSubmissionManifest>(&manifest_bytes)
        .map_err(|error| format!("cannot decode strict submission manifest JSON: {}", error))?;
    let canonical_manifest_bytes = serde_json::to_vec(&manifest)
        .map_err(|error| format!("cannot canonicalize submission manifest: {}", error))?;
    let manifest_keccak256 = format!(
        "0x{}",
        canonical_manifest_bytes.keccak256().to_hex::<String>()
    );
    Ok((manifest, manifest_keccak256))
}

fn read_signed_transaction(path: &Path) -> Result<Vec<u8>, String> {
    let contents = read_limited_regular_file(
        path,
        "signed transaction file",
        MAX_SIGNED_EIP1559_TRANSACTION_BYTES as u64,
    )?;
    if let Ok(text) = std::str::from_utf8(&contents) {
        let trimmed = text.trim();
        if let Some(hex) = trimmed.strip_prefix("0x") {
            if hex.is_empty() || hex.len() % 2 != 0 {
                return Err("0x-prefixed signed transaction must contain whole bytes".to_string());
            }
            return hex.from_hex::<Vec<u8>>().map_err(|_| {
                "0x-prefixed signed transaction contains non-hexadecimal data".to_string()
            });
        }
    }
    Ok(contents)
}

fn run_track(matches: &ArgMatches<'_>) -> Result<(), String> {
    let manifest_path = Path::new(
        matches
            .value_of("manifest-file")
            .expect("manifest-file is required"),
    );
    let (manifest, manifest_keccak256) = read_submission_manifest(manifest_path)?;
    let transaction_hash_text = matches.value_of("tx-hash").expect("tx-hash is required");
    let transaction_hash =
        H256::from_str(&transaction_hash_text[2..]).expect("tx-hash is validated");
    let normalized_transaction_hash = format!("{:#x}", transaction_hash);
    let required_confirmation_depth = matches
        .value_of("confirmation-depth")
        .expect("confirmation-depth has a default")
        .parse::<u64>()
        .expect("confirmation-depth is validated");
    let state_path = Path::new(
        matches
            .value_of("state-file")
            .expect("state-file is required"),
    );
    let state_lock = acquire_tracking_state_lock(state_path)?;
    let observation = track_submission_transaction(
        &manifest,
        transaction_hash,
        required_confirmation_depth,
        matches.value_of("rpc-url").expect("rpc-url is required"),
    )
    .map_err(|error| error.to_string())?;
    let checked_at_unix_s = system_now_unix_s()?;
    let previous_state_opt = read_tracking_state(state_path)?;
    let state = updated_tracking_state(
        previous_state_opt,
        manifest_keccak256,
        normalized_transaction_hash,
        observation.clone(),
        checked_at_unix_s,
    )?;
    persist_tracking_state(state_path, &state)?;
    state_lock.release()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&observation)
            .map_err(|error| format!("cannot serialize transaction observation: {}", error))?
    );
    Ok(())
}

fn run_broadcast(matches: &ArgMatches<'_>) -> Result<(), String> {
    let chain_name = matches.value_of("chain").expect("chain is required");
    let chain = chain_from_chain_identifier_opt(chain_name)
        .ok_or_else(|| format!("unsupported chain '{}'", chain_name))?;
    let canonical_batch_cbor = read_batch_cbor(Path::new(
        matches
            .value_of("batch-cbor-file")
            .expect("batch-cbor-file is required"),
    ))?;
    let (manifest, manifest_keccak256) = read_submission_manifest(Path::new(
        matches
            .value_of("manifest-file")
            .expect("manifest-file is required"),
    ))?;
    let raw_transaction = read_signed_transaction(Path::new(
        matches
            .value_of("signed-transaction-file")
            .expect("signed-transaction-file is required"),
    ))?;
    let parse_u256 = |name: &str| {
        U256::from_dec_str(
            matches
                .value_of(name)
                .expect("required uint256 argument is present"),
        )
        .expect("uint256 argument is validated")
    };
    let policy = SettlementBroadcastPolicy {
        expected_nonce: matches
            .value_of("expected-nonce")
            .map(|value| U256::from_dec_str(value).expect("expected-nonce is validated"))
            .expect("expected-nonce is required"),
        maximum_gas_limit: parse_u256("max-gas-limit"),
        maximum_fee_per_gas_wei: parse_u256("max-fee-per-gas-wei"),
        maximum_priority_fee_per_gas_wei: parse_u256("max-priority-fee-per-gas-wei"),
        maximum_total_fee_wei: parse_u256("max-total-fee-wei"),
    };
    let verified = verify_signed_eip1559_submission(&manifest, &raw_transaction, &policy)
        .map_err(|error| error.to_string())?;
    let transaction_hash = format!("{:#x}", verified.transaction_hash);
    let required_confirmation_depth = matches
        .value_of("confirmation-depth")
        .expect("confirmation-depth has a default")
        .parse::<u64>()
        .expect("confirmation-depth is validated");
    let rpc_url = matches.value_of("rpc-url").expect("rpc-url is required");
    let state_path = Path::new(
        matches
            .value_of("state-file")
            .expect("state-file is required"),
    );
    let state_lock = acquire_tracking_state_lock(state_path)?;
    let mut previous_state_opt = read_tracking_state(state_path)?;

    if previous_state_opt.is_some() {
        let observation = track_submission_transaction(
            &manifest,
            verified.transaction_hash,
            required_confirmation_depth,
            rpc_url,
        )
        .map_err(|error| error.to_string())?;
        let checked_at_unix_s = system_now_unix_s()?;
        let state = updated_tracking_state(
            previous_state_opt.take(),
            manifest_keccak256.clone(),
            transaction_hash.clone(),
            observation.clone(),
            checked_at_unix_s,
        )?;
        persist_tracking_state(state_path, &state)?;
        if observation.lifecycle_state != "not-found" && observation.lifecycle_state != "reorged" {
            state_lock.release()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "broadcast": null,
                    "observation": observation,
                    "reusedDurableIntent": true
                }))
                .map_err(|error| format!("cannot serialize broadcast outcome: {}", error))?
            );
            return Ok(());
        }
        previous_state_opt = Some(state);
    }

    let intent_time_unix_s = system_now_unix_s()?;
    let intent_observation = local_submission_observation(
        "ready-to-broadcast",
        &manifest,
        &verified,
        required_confirmation_depth,
    );
    let mut state = updated_tracking_state(
        previous_state_opt,
        manifest_keccak256.clone(),
        transaction_hash.clone(),
        intent_observation,
        intent_time_unix_s,
    )?;
    persist_tracking_state(state_path, &state)?;

    let broadcast_result = match broadcast_signed_eip1559_submission(
        &canonical_batch_cbor,
        chain,
        &manifest,
        &raw_transaction,
        &policy,
        system_now_unix_s()?,
        rpc_url,
    ) {
        Ok(result) => result,
        Err(error) => {
            if matches!(error, SettlementBatcherError::Broadcast(_)) {
                state = updated_tracking_state(
                    Some(state),
                    manifest_keccak256,
                    transaction_hash.clone(),
                    local_submission_observation(
                        "broadcast-uncertain",
                        &manifest,
                        &verified,
                        required_confirmation_depth,
                    ),
                    system_now_unix_s()?,
                )?;
                persist_tracking_state(state_path, &state)?;
            }
            state_lock.release()?;
            return Err(format!(
                "{}; deterministic transaction hash is {}",
                error, transaction_hash
            ));
        }
    };
    state = updated_tracking_state(
        Some(state),
        manifest_keccak256.clone(),
        transaction_hash.clone(),
        local_submission_observation(
            "broadcast-accepted",
            &manifest,
            &verified,
            required_confirmation_depth,
        ),
        system_now_unix_s()?,
    )?;
    persist_tracking_state(state_path, &state)?;

    let (observation_opt, tracking_error_opt) = match track_submission_transaction(
        &manifest,
        verified.transaction_hash,
        required_confirmation_depth,
        rpc_url,
    ) {
        Ok(observation) => {
            state = updated_tracking_state(
                Some(state),
                manifest_keccak256,
                transaction_hash,
                observation.clone(),
                system_now_unix_s()?,
            )?;
            persist_tracking_state(state_path, &state)?;
            (Some(observation), None)
        }
        Err(error) => (None, Some(error.to_string())),
    };
    state_lock.release()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "broadcast": broadcast_result,
            "observation": observation_opt,
            "trackingError": tracking_error_opt
        }))
        .map_err(|error| format!("cannot serialize broadcast outcome: {}", error))?
    );
    Ok(())
}

fn local_submission_observation(
    lifecycle_state: &str,
    manifest: &SettlementSubmissionManifest,
    verified: &node_lib::sub_lib::settlement_batcher::VerifiedSignedEip1559Submission,
    required_confirmation_depth: u64,
) -> SettlementTransactionObservation {
    SettlementTransactionObservation {
        lifecycle_state: lifecycle_state.to_string(),
        chain_id: manifest.chain_id,
        settlement_contract: manifest.settlement_contract.clone(),
        transaction_hash: format!("{:#x}", verified.transaction_hash),
        batcher_address: format!("{:#x}", verified.signer_address),
        batch_sequence: manifest.batch_sequence,
        required_confirmation_depth,
        latest_block_number: None,
        included_block_number: None,
        included_block_hash: None,
        confirmation_depth: None,
        gas_used: None,
        batch_total_delta_wei: None,
    }
}

fn persist_tracking_state(path: &Path, state: &SettlementTrackingState) -> Result<(), String> {
    let mut state_json = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("cannot serialize tracking state: {}", error))?;
    state_json.push(b'\n');
    atomic_write_tracking_state(path, &state_json)
}

fn acquire_tracking_state_lock(state_path: &Path) -> Result<TrackingStateLock, String> {
    let parent = state_path
        .parent()
        .filter(|candidate| !candidate.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_metadata = fs::metadata(parent)
        .map_err(|error| format!("cannot inspect tracking state directory: {}", error))?;
    if !parent_metadata.is_dir() {
        return Err("tracking state parent is not a directory".to_string());
    }
    let file_name = state_path
        .file_name()
        .ok_or_else(|| "tracking state path has no file name".to_string())?
        .to_string_lossy();
    let lock_path = parent.join(format!(".{}.lock", file_name));
    let started_at_unix_s = system_now_unix_s()?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut lock_file = options.open(&lock_path).map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            format!(
                "tracking state is locked; if no tracker is running, inspect and remove {}",
                lock_path.display()
            )
        } else {
            format!("cannot acquire tracking state lock: {}", error)
        }
    })?;
    let lock_contents = format!(
        "pid={}\nstartedAtUnixS={}\n",
        process::id(),
        started_at_unix_s
    );
    if let Err(error) = lock_file
        .write_all(lock_contents.as_bytes())
        .and_then(|_| lock_file.sync_all())
    {
        drop(lock_file);
        let _ = fs::remove_file(&lock_path);
        return Err(format!("cannot persist tracking state lock: {}", error));
    }
    drop(lock_file);
    if let Err(error) = sync_parent_directory(parent) {
        let _ = fs::remove_file(&lock_path);
        return Err(error);
    }
    Ok(TrackingStateLock {
        path: lock_path,
        parent: parent.to_path_buf(),
        active: true,
    })
}

fn read_tracking_state(path: &Path) -> Result<Option<SettlementTrackingState>, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot inspect tracking state: {}", error)),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err("tracking state path is not a regular non-symlink file".to_string());
            }
            if metadata.len() > MAX_TRACKING_JSON_FILE_BYTES {
                return Err(format!(
                    "tracking state exceeds the {}-byte safety limit",
                    MAX_TRACKING_JSON_FILE_BYTES
                ));
            }
            let contents =
                fs::read(path).map_err(|error| format!("cannot read tracking state: {}", error))?;
            let state = serde_json::from_slice::<SettlementTrackingState>(&contents)
                .map_err(|error| format!("cannot decode strict tracking state JSON: {}", error))?;
            Ok(Some(state))
        }
    }
}

fn updated_tracking_state(
    previous_state_opt: Option<SettlementTrackingState>,
    manifest_keccak256: String,
    transaction_hash: String,
    observation: SettlementTransactionObservation,
    checked_at_unix_s: u64,
) -> Result<SettlementTrackingState, String> {
    match previous_state_opt {
        None => Ok(SettlementTrackingState {
            state_version: 1,
            manifest_keccak256,
            transaction_hash,
            first_checked_at_unix_s: checked_at_unix_s,
            last_checked_at_unix_s: checked_at_unix_s,
            check_count: 1,
            lifecycle_transitions: vec![SettlementLifecycleTransition {
                observed_at_unix_s: checked_at_unix_s,
                observation: observation.clone(),
            }],
            observation,
        }),
        Some(mut state) => {
            validate_previous_tracking_state(
                &state,
                &manifest_keccak256,
                &transaction_hash,
                &observation,
            )?;
            if checked_at_unix_s < state.last_checked_at_unix_s {
                return Err("system clock moved behind the last tracking check".to_string());
            }
            if is_material_lifecycle_transition(&state.observation, &observation) {
                if state.lifecycle_transitions.len() >= MAX_LIFECYCLE_TRANSITIONS {
                    return Err(format!(
                        "tracking state reached the {}-transition safety limit",
                        MAX_LIFECYCLE_TRANSITIONS
                    ));
                }
                state
                    .lifecycle_transitions
                    .push(SettlementLifecycleTransition {
                        observed_at_unix_s: checked_at_unix_s,
                        observation: observation.clone(),
                    });
            }
            state.last_checked_at_unix_s = checked_at_unix_s;
            state.check_count = state
                .check_count
                .checked_add(1)
                .ok_or_else(|| "tracking check counter overflowed".to_string())?;
            state.observation = observation;
            Ok(state)
        }
    }
}

fn validate_previous_tracking_state(
    state: &SettlementTrackingState,
    manifest_keccak256: &str,
    transaction_hash: &str,
    current_observation: &SettlementTransactionObservation,
) -> Result<(), String> {
    if state.state_version != 1 {
        return Err("tracking state has an unsupported version".to_string());
    }
    if state.manifest_keccak256 != manifest_keccak256 {
        return Err("tracking state belongs to a different submission manifest".to_string());
    }
    if state.transaction_hash != transaction_hash {
        return Err("tracking state belongs to a different transaction".to_string());
    }
    if state.first_checked_at_unix_s > state.last_checked_at_unix_s
        || state.check_count == 0
        || state.lifecycle_transitions.is_empty()
        || state.lifecycle_transitions.len() > MAX_LIFECYCLE_TRANSITIONS
        || state.check_count < state.lifecycle_transitions.len() as u64
    {
        return Err("tracking state counters or lifecycle history are invalid".to_string());
    }
    let mut prior_transition_time = state.first_checked_at_unix_s;
    for transition in &state.lifecycle_transitions {
        if transition.observed_at_unix_s < prior_transition_time
            || transition.observed_at_unix_s > state.last_checked_at_unix_s
        {
            return Err("tracking state lifecycle timestamps are invalid".to_string());
        }
        validate_observation_identity(&transition.observation, current_observation)?;
        prior_transition_time = transition.observed_at_unix_s;
    }
    validate_observation_identity(&state.observation, current_observation)
}

fn validate_observation_identity(
    prior: &SettlementTransactionObservation,
    current: &SettlementTransactionObservation,
) -> Result<(), String> {
    if prior.chain_id != current.chain_id
        || prior.settlement_contract != current.settlement_contract
        || prior.transaction_hash != current.transaction_hash
        || prior.batcher_address != current.batcher_address
        || prior.batch_sequence != current.batch_sequence
        || prior.required_confirmation_depth != current.required_confirmation_depth
    {
        return Err("tracking state observation identity does not match this check".to_string());
    }
    Ok(())
}

fn is_material_lifecycle_transition(
    prior: &SettlementTransactionObservation,
    current: &SettlementTransactionObservation,
) -> bool {
    prior.lifecycle_state != current.lifecycle_state
        || prior.included_block_number != current.included_block_number
        || prior.included_block_hash != current.included_block_hash
        || prior.gas_used != current.gas_used
        || prior.batch_total_delta_wei != current.batch_total_delta_wei
}

fn atomic_write_tracking_state(path: &Path, contents: &[u8]) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err("tracking state destination is not a regular non-symlink file".to_string())
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot inspect tracking state destination: {}",
                error
            ))
        }
    }
    let parent = path
        .parent()
        .filter(|candidate| !candidate.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_metadata = fs::metadata(parent)
        .map_err(|error| format!("cannot inspect tracking state directory: {}", error))?;
    if !parent_metadata.is_dir() {
        return Err("tracking state parent is not a directory".to_string());
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| "tracking state path has no file name".to_string())?
        .to_string_lossy();
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before Unix epoch".to_string())?
        .as_nanos();
    let temporary_path = parent.join(format!(
        ".{}.tmp.{}.{}",
        file_name,
        process::id(),
        unique_suffix
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let write_result = (|| -> Result<(), String> {
        let mut file = options
            .open(&temporary_path)
            .map_err(|error| format!("cannot create temporary tracking state: {}", error))?;
        file.write_all(contents)
            .map_err(|error| format!("cannot write temporary tracking state: {}", error))?;
        file.sync_all()
            .map_err(|error| format!("cannot sync temporary tracking state: {}", error))?;
        drop(file);
        replace_file(&temporary_path, path)?;
        sync_parent_directory(parent)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result
}

#[cfg(unix)]
fn replace_file(temporary_path: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(temporary_path, destination)
        .map_err(|error| format!("cannot atomically replace tracking state: {}", error))
}

#[cfg(target_os = "windows")]
fn replace_file(temporary_path: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let existing = temporary_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replacement = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            replacement.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(format!(
            "cannot atomically replace tracking state: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<(), String> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync tracking state directory: {}", error))
}

#[cfg(target_os = "windows")]
fn sync_parent_directory(_parent: &Path) -> Result<(), String> {
    Ok(())
}

fn run() -> Result<(), String> {
    let matches = app().get_matches();
    if let Some(broadcast_matches) = matches.subcommand_matches("broadcast") {
        return run_broadcast(broadcast_matches);
    }
    if let Some(track_matches) = matches.subcommand_matches("track") {
        return run_track(track_matches);
    }
    let path = Path::new(
        matches
            .value_of("batch-cbor-file")
            .expect("batch-cbor-file is required"),
    );
    let chain_name = matches.value_of("chain").expect("chain is required");
    let chain = chain_from_chain_identifier_opt(chain_name)
        .ok_or_else(|| format!("unsupported chain '{}'", chain_name))?;
    let system_now_unix_s = system_now_unix_s()?;
    let offline_verification_time_unix_s = matches
        .value_of("verification-time-unix-s")
        .map(|value| {
            value
                .parse::<u64>()
                .expect("verification-time-unix-s is validated")
        })
        .unwrap_or(system_now_unix_s);
    let canonical_batch_cbor = read_batch_cbor(path)?;
    let manifest = match matches.value_of("rpc-url") {
        Some(rpc_url) => {
            let batcher_address_text = matches
                .value_of("batcher-address")
                .ok_or_else(|| "--batcher-address is required with --rpc-url".to_string())?;
            let batcher_address = Address::from_str(&batcher_address_text[2..])
                .expect("batcher-address is validated");
            prepare_rpc_bound_submission_manifest(
                &canonical_batch_cbor,
                chain,
                batcher_address,
                system_now_unix_s,
                rpc_url,
            )
        }
        None => prepare_submission_manifest(
            &canonical_batch_cbor,
            chain,
            matches
                .value_of("batch-sequence")
                .expect("batch-sequence is required without rpc-url")
                .parse::<u64>()
                .expect("batch-sequence is validated"),
            offline_verification_time_unix_s,
        ),
    }
    .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&manifest)
            .map_err(|error| format!("cannot serialize submission manifest: {}", error))?
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("masq-settlement-batcher: {}", error);
        process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(state: &str, depth: Option<u64>) -> SettlementTransactionObservation {
        SettlementTransactionObservation {
            lifecycle_state: state.to_string(),
            chain_id: 84532,
            settlement_contract: format!("0x{}", "11".repeat(20)),
            transaction_hash: format!("0x{}", "22".repeat(32)),
            batcher_address: format!("0x{}", "33".repeat(20)),
            batch_sequence: 7,
            required_confirmation_depth: 64,
            latest_block_number: Some(100 + depth.unwrap_or(0)),
            included_block_number: depth.map(|_| 100),
            included_block_hash: depth.map(|_| format!("0x{}", "44".repeat(32))),
            confirmation_depth: depth,
            gas_used: depth.map(|_| "21000".to_string()),
            batch_total_delta_wei: depth.map(|_| "17".to_string()),
        }
    }

    #[test]
    fn track_subcommand_does_not_require_prepare_arguments() {
        let result = app().get_matches_from_safe(vec![
            "masq-settlement-batcher",
            "track",
            "--manifest-file",
            "manifest.json",
            "--tx-hash",
            &format!("0x{}", "aa".repeat(32)),
            "--rpc-url",
            "http://127.0.0.1:8545",
            "--state-file",
            "tracking.json",
        ]);

        assert!(result.is_ok());
    }

    #[test]
    fn broadcast_subcommand_requires_explicit_fee_policy_without_a_private_key() {
        let result = app().get_matches_from_safe(vec![
            "masq-settlement-batcher",
            "broadcast",
            "--batch-cbor-file",
            "batch.cbor",
            "--manifest-file",
            "manifest.json",
            "--signed-transaction-file",
            "signed.raw",
            "--chain",
            "base-sepolia",
            "--rpc-url",
            "http://127.0.0.1:8545",
            "--expected-nonce",
            "7",
            "--max-gas-limit",
            "500000",
            "--max-fee-per-gas-wei",
            "2000000000",
            "--max-priority-fee-per-gas-wei",
            "0",
            "--max-total-fee-wei",
            "1000000000000000",
            "--state-file",
            "tracking.json",
        ]);

        assert!(result.is_ok());
        let debug = format!("{:?}", result.unwrap());
        assert!(!debug.contains("private-key"));
        assert!(!debug.contains("secret-key"));
    }

    #[test]
    fn tracking_state_records_material_transitions_but_not_every_confirmation() {
        let manifest_hash = format!("0x{}", "55".repeat(32));
        let transaction_hash = format!("0x{}", "22".repeat(32));
        let initial = updated_tracking_state(
            None,
            manifest_hash.clone(),
            transaction_hash.clone(),
            observation("included", Some(2)),
            1_000,
        )
        .unwrap();
        let more_confirmations = updated_tracking_state(
            Some(initial),
            manifest_hash.clone(),
            transaction_hash.clone(),
            observation("included", Some(30)),
            1_010,
        )
        .unwrap();

        assert_eq!(more_confirmations.check_count, 2);
        assert_eq!(more_confirmations.lifecycle_transitions.len(), 1);
        let finalized = updated_tracking_state(
            Some(more_confirmations),
            manifest_hash,
            transaction_hash,
            observation("finalized", Some(64)),
            1_020,
        )
        .unwrap();
        assert_eq!(finalized.check_count, 3);
        assert_eq!(finalized.lifecycle_transitions.len(), 2);
        assert_eq!(
            finalized.lifecycle_transitions[1]
                .observation
                .lifecycle_state,
            "finalized"
        );
    }

    #[test]
    fn tracking_state_is_atomically_replaced_without_temporary_debris() {
        let directory = std::env::temp_dir().join(format!(
            "masq-settlement-batcher-state-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let state_path = directory.join("state.json");

        atomic_write_tracking_state(&state_path, b"one\n").unwrap();
        atomic_write_tracking_state(&state_path, b"two\n").unwrap();

        assert_eq!(fs::read(&state_path).unwrap(), b"two\n");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn tracking_state_lock_enforces_one_writer_and_can_be_reacquired() {
        let directory = std::env::temp_dir().join(format!(
            "masq-settlement-batcher-lock-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let state_path = directory.join("state.json");

        let first = acquire_tracking_state_lock(&state_path).unwrap();
        assert!(acquire_tracking_state_lock(&state_path)
            .err()
            .unwrap()
            .contains("is locked"));
        first.release().unwrap();
        acquire_tracking_state_lock(&state_path)
            .unwrap()
            .release()
            .unwrap();

        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        fs::remove_dir_all(directory).unwrap();
    }
}
