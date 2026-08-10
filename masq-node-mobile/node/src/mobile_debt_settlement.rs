// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

//! Fail-closed, user-initiated settlement of consumer debts for the mobile client.
//!
//! This path deliberately does not reuse the periodic payable scanner. It reserves every
//! quoted payable and persists locally signed transactions before submitting them. An
//! ambiguous RPC response is never retried automatically.

use crate::accountant::db_access_objects::utils::to_unix_timestamp;
use crate::accountant::db_big_integer::big_int_divider::BigIntDivider;
use crate::accountant::scanners::payable_scanner::tx_templates::initial::new::{
    NewTxTemplate, NewTxTemplates,
};
use crate::accountant::scanners::payable_scanner::tx_templates::signable::SignableTxTemplates;
use crate::accountant::scanners::payable_scanner::tx_templates::BaseTxTemplate;
use crate::blockchain::bip32::Bip32EncryptionKeyProvider;
use crate::blockchain::blockchain_interface::blockchain_interface_web3::utils::sign_transaction;
use crate::blockchain::blockchain_interface::blockchain_interface_web3::{
    BlockchainInterfaceWeb3, REQUESTS_IN_PARALLEL,
};
use crate::blockchain::blockchain_interface::data_structures::errors::{
    BlockchainAgentBuildError, BlockchainInterfaceError,
};
use crate::blockchain::blockchain_interface::BlockchainInterface;
use crate::database::db_initializer::DATABASE_FILE;
use crate::sub_lib::wallet::Wallet;
use ethsign_crypto::Keccak256;
use futures::Future;
use itertools::Either;
use masq_lib::blockchains::chains::Chain;
use rusqlite::{params, Connection, OptionalExtension};
use serde_derive::Serialize;
use std::convert::TryFrom;
use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use web3::transports::{Batch, Http};
use web3::types::{Address, BlockNumber, Bytes, H256, U256};
use web3::Web3;

const MAX_CREDITORS_PER_SETTLEMENT: usize = 20;
const QUOTE_VALIDITY: Duration = Duration::from_secs(5 * 60);
const CONFIRMATION_DEPTH: u64 = 12;
const ACTIVE_PHASES: [&str; 3] = ["reserved", "submitted", "attention"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebtEntry {
    address: Address,
    amount_wei: u128,
    last_paid_timestamp: i64,
}

#[derive(Clone, Debug)]
pub struct PreparedDebtSettlement {
    pub quote_id: String,
    pub created_at: SystemTime,
    pub expires_at: SystemTime,
    pub total_masq_wei: u128,
    pub estimated_l2_fee_wei: u128,
    pub masq_balance_wei: u128,
    pub base_eth_balance_wei: u128,
    pub creditor_count: usize,
    pub has_more_creditors: bool,
    entries: Vec<DebtEntry>,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PublicDebtSummary {
    pub total_masq_wei: String,
    pub creditor_count: usize,
    pub settlement_in_progress: bool,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PublicSettlementStatus {
    pub operation_id: Option<String>,
    pub phase: String,
    pub total_masq_wei: String,
    pub estimated_l2_fee_wei: String,
    pub transaction_count: usize,
    pub confirmed_transaction_count: usize,
    pub transaction_hashes: Vec<String>,
    pub error_code: Option<String>,
}

#[derive(Debug)]
struct SignedDebtTransaction {
    hash: H256,
    receiver_address: Address,
    amount_wei: u128,
    gas_price_wei: u128,
    nonce: u64,
    raw_transaction: Bytes,
}

#[derive(Debug)]
struct StoredTransaction {
    hash: H256,
    receiver_address: Address,
    amount_wei: u128,
    nonce: u64,
    reservation_id: i64,
    raw_transaction: Bytes,
    status: String,
}

pub fn debt_summary(data_directory: &Path) -> Result<PublicDebtSummary, String> {
    let conn = open_database(data_directory)?;
    ensure_mobile_tables(&conn)?;
    let debts = read_debts(&conn, false)?;
    Ok(PublicDebtSummary {
        total_masq_wei: debts
            .iter()
            .try_fold(0_u128, |total, debt| total.checked_add(debt.amount_wei))
            .ok_or_else(|| "The debt total is too large to display safely.".to_owned())?
            .to_string(),
        creditor_count: debts.len(),
        settlement_in_progress: active_operation_id(&conn)?.is_some(),
    })
}

pub fn prepare_debt_settlement(
    data_directory: &Path,
    rpc_url: &str,
    chain_identifier: &str,
    wallet_secret: &[u8],
) -> Result<PreparedDebtSettlement, String> {
    let chain = settlement_chain(chain_identifier)?;
    let conn = open_database(data_directory)?;
    ensure_mobile_tables(&conn)?;
    if active_operation_id(&conn)?.is_some() {
        return Err("A debt settlement is already awaiting a final blockchain result.".to_owned());
    }

    let all_entries = read_debts(&conn, true)?;
    let has_more_creditors = all_entries.len() > MAX_CREDITORS_PER_SETTLEMENT;
    let entries = all_entries
        .into_iter()
        .take(MAX_CREDITORS_PER_SETTLEMENT)
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Err("There are no outstanding MASQ debts to settle.".to_owned());
    }

    let wallet = wallet_from_secret(wallet_secret)?;
    let interface = blockchain_interface(rpc_url, chain)?;
    let agent = interface
        .introduce_blockchain_agent(wallet)
        .wait()
        .map_err(|error| settlement_agent_error("read", error))?;
    let initial = NewTxTemplates::from(
        entries
            .iter()
            .map(|entry| NewTxTemplate {
                base: BaseTxTemplate {
                    receiver_address: entry.address,
                    amount_in_wei: entry.amount_wei,
                },
            })
            .collect::<Vec<_>>(),
    );
    let priced = agent.price_qualified_payables(Either::Left(initial));
    let estimated_l2_fee_wei = agent.estimate_transaction_fee_total(&priced);
    let balances = agent.consuming_wallet_balances();
    let total_masq_wei = entries
        .iter()
        .try_fold(0_u128, |total, entry| total.checked_add(entry.amount_wei))
        .ok_or_else(|| "The debt total is too large to settle safely.".to_owned())?;
    let masq_balance_wei = balances.masq_token_balance_in_minor_units.as_u128();
    let base_eth_balance_wei = balances.transaction_fee_balance_in_minor_units.as_u128();
    if masq_balance_wei < total_masq_wei {
        return Err(
            "The consumer wallet does not contain enough MASQ to settle these debts.".to_owned(),
        );
    }
    if base_eth_balance_wei < estimated_l2_fee_wei {
        return Err(
            "The consumer wallet does not contain enough Base ETH for the estimated network fee."
                .to_owned(),
        );
    }

    let created_at = SystemTime::now();
    let expires_at = created_at
        .checked_add(QUOTE_VALIDITY)
        .ok_or_else(|| "The settlement quote expiry could not be created.".to_owned())?;
    let quote_id = quote_identifier(chain, created_at, &entries);
    Ok(PreparedDebtSettlement {
        quote_id,
        created_at,
        expires_at,
        total_masq_wei,
        estimated_l2_fee_wei,
        masq_balance_wei,
        base_eth_balance_wei,
        creditor_count: entries.len(),
        has_more_creditors,
        entries,
    })
}

pub fn submit_prepared_debt_settlement(
    data_directory: &Path,
    rpc_url: &str,
    chain_identifier: &str,
    wallet_secret: &[u8],
    prepared: &PreparedDebtSettlement,
    maximum_masq_wei: u128,
    maximum_estimated_l2_fee_wei: u128,
) -> Result<PublicSettlementStatus, String> {
    let chain = settlement_chain(chain_identifier)?;
    if SystemTime::now() > prepared.expires_at {
        return Err("The settlement quote expired. Review the current debts again.".to_owned());
    }
    if prepared.total_masq_wei > maximum_masq_wei {
        return Err("The current MASQ debt exceeds the amount reviewed in the app.".to_owned());
    }
    if prepared.estimated_l2_fee_wei > maximum_estimated_l2_fee_wei {
        return Err(
            "The estimated Base network fee exceeds the amount reviewed in the app.".to_owned(),
        );
    }

    let mut conn = open_database(data_directory)?;
    ensure_mobile_tables(&conn)?;
    if active_operation_id(&conn)?.is_some() {
        return Err("A debt settlement is already awaiting a final blockchain result.".to_owned());
    }
    ensure_quote_still_matches(&conn, prepared)?;

    let wallet = wallet_from_secret(wallet_secret)?;
    let wallet_address = wallet.address();
    let interface = blockchain_interface(rpc_url, chain)?;
    let agent = interface
        .introduce_blockchain_agent(wallet.clone())
        .wait()
        .map_err(|error| settlement_agent_error("refresh", error))?;
    let initial = NewTxTemplates::from(
        prepared
            .entries
            .iter()
            .map(|entry| NewTxTemplate {
                base: BaseTxTemplate {
                    receiver_address: entry.address,
                    amount_in_wei: entry.amount_wei,
                },
            })
            .collect::<Vec<_>>(),
    );
    let priced = agent.price_qualified_payables(Either::Left(initial));
    let refreshed_fee = agent.estimate_transaction_fee_total(&priced);
    if refreshed_fee > maximum_estimated_l2_fee_wei {
        return Err(
            "The estimated Base network fee changed. Review a new quote before paying.".to_owned(),
        );
    }
    let balances = agent.consuming_wallet_balances();
    if balances.masq_token_balance_in_minor_units < U256::from(prepared.total_masq_wei) {
        return Err("The consumer wallet no longer contains enough MASQ.".to_owned());
    }
    if balances.transaction_fee_balance_in_minor_units < U256::from(refreshed_fee) {
        return Err(
            "The consumer wallet no longer contains enough Base ETH for the estimated fee."
                .to_owned(),
        );
    }

    let plain_web3 = Web3::new(interface_transport(&interface));
    let latest_nonce = plain_web3
        .eth()
        .transaction_count(wallet_address, Some(BlockNumber::Latest))
        .wait()
        .map_err(|_| "MASQ could not read the confirmed wallet nonce.".to_owned())?;
    let pending_nonce = plain_web3
        .eth()
        .transaction_count(wallet_address, Some(BlockNumber::Pending))
        .wait()
        .map_err(|_| "MASQ could not read the pending wallet nonce.".to_owned())?;
    if latest_nonce != pending_nonce {
        return Err(
            "Another wallet transaction is pending. Wait for it before settling MASQ debts."
                .to_owned(),
        );
    }

    let transport = interface_transport(&interface);
    // The legacy signing helper is typed for Batch<Http>, but signing itself performs no RPC.
    let signing_web3 = Web3::new(Batch::new(transport.clone()));
    let signable = SignableTxTemplates::new(priced, latest_nonce.as_u64());
    let signed = signable
        .iter()
        .map(|template| {
            let signed = sign_transaction(chain, &signing_web3, template, &wallet);
            SignedDebtTransaction {
                hash: signed.transaction_hash,
                receiver_address: template.receiver_address,
                amount_wei: template.amount_in_wei,
                gas_price_wei: template.gas_price_wei,
                nonce: template.nonce,
                raw_transaction: signed.raw_transaction,
            }
        })
        .collect::<Vec<_>>();

    let operation_id = operation_identifier(&prepared.quote_id, SystemTime::now());
    persist_reservation(&mut conn, &operation_id, prepared, refreshed_fee, &signed)?;

    // Public RPC providers commonly impose small JSON-RPC batch limits. More importantly, a
    // transport failure for a batch makes every member ambiguous at once. Submit each locally
    // persisted transaction separately and stop at the first uncertain answer. Nothing is ever
    // retried automatically; later reserved transactions remain unsubmitted for manual review.
    let submission_web3 = Web3::new(transport);
    for transaction in &signed {
        let result = submission_web3
            .eth()
            .send_raw_transaction(transaction.raw_transaction.clone())
            .wait();
        let exact_hash = matches!(result, Ok(hash) if hash == transaction.hash);
        record_submission_result(&conn, &operation_id, transaction, exact_hash)?;
        if !exact_hash {
            mark_operation_attention(&conn, &operation_id, "E_SETTLEMENT_RPC_AMBIGUOUS")?;
            return read_operation_status(&conn, Some(&operation_id));
        }
    }
    set_operation_phase(&conn, &operation_id, "submitted", None)?;
    read_operation_status(&conn, Some(&operation_id))
}

pub fn refresh_debt_settlement_status(
    data_directory: &Path,
    rpc_url: &str,
) -> Result<PublicSettlementStatus, String> {
    let conn = open_database(data_directory)?;
    ensure_mobile_tables(&conn)?;
    let operation_id = match active_operation_id(&conn)? {
        Some(id) => id,
        None => return read_operation_status(&conn, None),
    };
    let transactions = read_operation_transactions(&conn, &operation_id)?;
    if transactions.is_empty() {
        mark_operation_attention(&conn, &operation_id, "E_SETTLEMENT_STATE_INVALID")?;
        return read_operation_status(&conn, Some(&operation_id));
    }

    let (event_loop_handle, transport) = Http::with_max_parallel(rpc_url, REQUESTS_IN_PARALLEL)
        .map_err(|_| "The blockchain RPC URL could not be opened.".to_owned())?;
    let web3 = Web3::new(transport);
    let latest_block = web3
        .eth()
        .block_number()
        .wait()
        .map_err(|_| "MASQ could not refresh the settlement confirmations.".to_owned())?
        .as_u64();
    let _event_loop_handle = event_loop_handle;

    for transaction in transactions
        .iter()
        .filter(|transaction| transaction.status != "confirmed" && transaction.status != "reverted")
    {
        let receipt = match web3.eth().transaction_receipt(transaction.hash).wait() {
            Ok(receipt) => receipt,
            Err(_) => continue,
        };
        let Some(receipt) = receipt else { continue };
        if receipt.status == Some(0_u64.into()) {
            release_reverted_transaction(&conn, &operation_id, transaction)?;
            continue;
        }
        if receipt.status != Some(1_u64.into()) {
            continue;
        }
        let Some(block_number) = receipt.block_number.map(|number| number.as_u64()) else {
            continue;
        };
        if latest_block < block_number.saturating_add(CONFIRMATION_DEPTH) {
            continue;
        }
        confirm_transaction(&conn, &operation_id, transaction)?;
    }
    update_operation_phase(&conn, &operation_id)?;
    read_operation_status(&conn, Some(&operation_id))
}

/// Explicitly retries the exact signed transactions already persisted for an ambiguous
/// settlement. This never creates a new signature, changes a recipient or advances a nonce.
/// The caller must expose this as a deliberate user action; it must never run automatically.
pub fn retry_ambiguous_debt_settlement(
    data_directory: &Path,
    rpc_url: &str,
) -> Result<PublicSettlementStatus, String> {
    let conn = open_database(data_directory)?;
    ensure_mobile_tables(&conn)?;
    let operation_id = active_operation_id(&conn)?
        .ok_or_else(|| "There is no active MASQ settlement to retry.".to_owned())?;
    let (phase, error_code): (String, Option<String>) = conn
        .query_row(
            "SELECT phase, error_code FROM mobile_debt_settlement_operation
             WHERE operation_id = ?1",
            params![operation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "The settlement retry state could not be read.".to_owned())?;
    if phase != "attention" || error_code.as_deref() != Some("E_SETTLEMENT_RPC_AMBIGUOUS") {
        return Err("Only an ambiguous MASQ settlement can be retried.".to_owned());
    }
    let transactions = read_operation_transactions(&conn, &operation_id)?;
    if transactions.is_empty()
        || transactions.iter().any(|transaction| {
            transaction.status == "confirmed" || transaction.status == "reverted"
        })
    {
        return Err("The saved settlement is not safe to retry as one operation.".to_owned());
    }

    let (event_loop_handle, transport) = Http::with_max_parallel(rpc_url, REQUESTS_IN_PARALLEL)
        .map_err(|_| "The blockchain RPC URL could not be opened.".to_owned())?;
    let web3 = Web3::new(transport);
    let _event_loop_handle = event_loop_handle;
    set_operation_phase(&conn, &operation_id, "reserved", None)?;
    for transaction in &transactions {
        // A previous attempt may have received exact hashes for an initial prefix before
        // a later transaction became ambiguous. Those accepted transactions must only be
        // monitored for receipts; retry the uncertain and never-submitted suffix.
        if transaction.status == "submitted" {
            continue;
        }
        let result = web3
            .eth()
            .send_raw_transaction(transaction.raw_transaction.clone())
            .wait();
        let exact_hash = matches!(result, Ok(hash) if hash == transaction.hash);
        let signed = SignedDebtTransaction {
            hash: transaction.hash,
            receiver_address: transaction.receiver_address,
            amount_wei: transaction.amount_wei,
            gas_price_wei: 0,
            nonce: transaction.nonce,
            raw_transaction: transaction.raw_transaction.clone(),
        };
        record_submission_result(&conn, &operation_id, &signed, exact_hash)?;
        if !exact_hash {
            mark_operation_attention(&conn, &operation_id, "E_SETTLEMENT_RPC_AMBIGUOUS")?;
            return read_operation_status(&conn, Some(&operation_id));
        }
    }
    set_operation_phase(&conn, &operation_id, "submitted", None)?;
    read_operation_status(&conn, Some(&operation_id))
}

fn open_database(data_directory: &Path) -> Result<Connection, String> {
    let database_path = data_directory.join(DATABASE_FILE);
    if !database_path.exists() {
        return Err("The MASQ accounting database is not ready yet.".to_owned());
    }
    let conn = Connection::open(database_path)
        .map_err(|_| "The MASQ accounting database could not be opened.".to_owned())?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|_| "The MASQ accounting database is busy.".to_owned())?;
    Ok(conn)
}

fn ensure_mobile_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS mobile_debt_settlement_operation (
            operation_id TEXT PRIMARY KEY,
            quote_id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            phase TEXT NOT NULL,
            total_masq_wei TEXT NOT NULL,
            estimated_l2_fee_wei TEXT NOT NULL,
            transaction_count INTEGER NOT NULL,
            error_code TEXT NULL
        );
        CREATE TABLE IF NOT EXISTS mobile_debt_settlement_transaction (
            tx_hash TEXT PRIMARY KEY,
            operation_id TEXT NOT NULL,
            receiver_address TEXT NOT NULL,
            amount_high_b INTEGER NOT NULL,
            amount_low_b INTEGER NOT NULL,
            gas_price_high_b INTEGER NOT NULL,
            gas_price_low_b INTEGER NOT NULL,
            nonce INTEGER NOT NULL,
            reservation_id INTEGER NOT NULL,
            raw_transaction BLOB NOT NULL,
            status TEXT NOT NULL,
            FOREIGN KEY(operation_id) REFERENCES mobile_debt_settlement_operation(operation_id)
        );",
    )
    .map_err(|_| "The mobile settlement database could not be prepared.".to_owned())
}

fn read_debts(conn: &Connection, only_unreserved: bool) -> Result<Vec<DebtEntry>, String> {
    let pending_filter = if only_unreserved {
        " AND pending_payable_rowid IS NULL"
    } else {
        ""
    };
    let sql = format!(
        "SELECT wallet_address, balance_high_b, balance_low_b, last_paid_timestamp
         FROM payable
         WHERE (balance_high_b > 0 OR (balance_high_b = 0 AND balance_low_b > 0)){}
         ORDER BY last_paid_timestamp ASC, wallet_address ASC",
        pending_filter
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|_| "The MASQ debt list could not be prepared.".to_owned())?;
    let rows = statement
        .query_map([], |row| {
            let address_text: String = row.get(0)?;
            let high: i64 = row.get(1)?;
            let low: i64 = row.get(2)?;
            let timestamp: i64 = row.get(3)?;
            Ok((address_text, high, low, timestamp))
        })
        .map_err(|_| "The MASQ debt list could not be read.".to_owned())?;
    rows.map(|row| {
        let (address_text, high, low, last_paid_timestamp) =
            row.map_err(|_| "The MASQ debt database contains an invalid row.".to_owned())?;
        let address = Address::from_str(address_text.trim_start_matches("0x"))
            .map_err(|_| "The MASQ debt database contains an invalid recipient.".to_owned())?;
        let amount = BigIntDivider::reconstitute(high, low);
        if amount <= 0 {
            return Err("The MASQ debt database contains an invalid amount.".to_owned());
        }
        Ok(DebtEntry {
            address,
            amount_wei: u128::try_from(amount)
                .map_err(|_| "The MASQ debt amount is outside the supported range.".to_owned())?,
            last_paid_timestamp,
        })
    })
    .collect()
}

fn active_operation_id(conn: &Connection) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT operation_id FROM mobile_debt_settlement_operation
         WHERE phase IN (?1, ?2, ?3) ORDER BY created_at DESC LIMIT 1",
        params![ACTIVE_PHASES[0], ACTIVE_PHASES[1], ACTIVE_PHASES[2]],
        |row| row.get(0),
    )
    .optional()
    .map_err(|_| "The current settlement state could not be read.".to_owned())
}

fn wallet_from_secret(secret: &[u8]) -> Result<Wallet, String> {
    Bip32EncryptionKeyProvider::from_raw_secret(secret)
        .map(Wallet::from)
        .map_err(|_| "The consumer wallet could not be prepared for settlement.".to_owned())
}

fn settlement_chain(identifier: &str) -> Result<Chain, String> {
    match identifier {
        "base-mainnet" => Ok(Chain::BaseMainnet),
        "base-sepolia" => Ok(Chain::BaseSepolia),
        _ => Err("Mobile debt settlement is supported only on Base chains.".to_owned()),
    }
}

fn blockchain_interface(rpc_url: &str, chain: Chain) -> Result<BlockchainInterfaceWeb3, String> {
    let (event_loop_handle, transport) = Http::with_max_parallel(rpc_url, REQUESTS_IN_PARALLEL)
        .map_err(|_| "The blockchain RPC URL could not be opened.".to_owned())?;
    Ok(BlockchainInterfaceWeb3::new(
        transport,
        event_loop_handle,
        chain,
    ))
}

fn settlement_agent_error(action: &str, error: BlockchainAgentBuildError) -> String {
    let (stage, code, cause) = match error {
        BlockchainAgentBuildError::GasPrice(cause) => {
            ("current Base network fee", "E_SETTLEMENT_RPC_GAS_PRICE", cause)
        }
        BlockchainAgentBuildError::TransactionFeeBalance(_, cause) => (
            "Base ETH balance",
            "E_SETTLEMENT_RPC_ETH_BALANCE",
            cause,
        ),
        BlockchainAgentBuildError::ServiceFeeBalance(_, cause) => {
            ("MASQ balance", "E_SETTLEMENT_RPC_MASQ_BALANCE", cause)
        }
        BlockchainAgentBuildError::UninitializedInterface => {
            return "The blockchain RPC is not initialized. Diagnostic code: E_SETTLEMENT_RPC_UNINITIALIZED."
                .to_owned()
        }
    };
    // Do not include the wallet address or RPC URL in the user-facing result. The
    // low-level classification is still useful in local Android logs while testing.
    eprintln!(
        "MASQ mobile settlement could not {} {}: {}",
        action,
        stage,
        safe_blockchain_error(&cause)
    );
    format!(
        "MASQ could not {} the {}. Diagnostic code: {}.",
        action, stage, code
    )
}

fn safe_blockchain_error(error: &BlockchainInterfaceError) -> &'static str {
    match error {
        BlockchainInterfaceError::InvalidUrl => "invalid RPC URL",
        BlockchainInterfaceError::InvalidAddress => "invalid address",
        BlockchainInterfaceError::InvalidResponse => "invalid RPC response",
        BlockchainInterfaceError::QueryFailed(_) => "RPC transport or query failure",
        BlockchainInterfaceError::UninitializedInterface => "uninitialized RPC interface",
    }
}

fn interface_transport(interface: &BlockchainInterfaceWeb3) -> Http {
    interface.http_transport()
}

fn ensure_quote_still_matches(
    conn: &Connection,
    prepared: &PreparedDebtSettlement,
) -> Result<(), String> {
    let current = read_debts(conn, true)?;
    let selected = current
        .into_iter()
        .take(prepared.entries.len())
        .collect::<Vec<_>>();
    if selected != prepared.entries {
        return Err(
            "The MASQ debts changed. Review a new settlement quote before paying.".to_owned(),
        );
    }
    Ok(())
}

fn persist_reservation(
    conn: &mut Connection,
    operation_id: &str,
    prepared: &PreparedDebtSettlement,
    estimated_l2_fee_wei: u128,
    signed: &[SignedDebtTransaction],
) -> Result<(), String> {
    if signed.is_empty() || signed.len() != prepared.entries.len() {
        return Err("The settlement transaction set is incomplete.".to_owned());
    }
    let db_tx = conn
        .transaction()
        .map_err(|_| "The settlement reservation could not be started.".to_owned())?;
    let now = to_unix_timestamp(SystemTime::now());
    db_tx
        .execute(
            "INSERT INTO mobile_debt_settlement_operation
             (operation_id, quote_id, created_at, updated_at, phase, total_masq_wei,
              estimated_l2_fee_wei, transaction_count, error_code)
             VALUES (?1, ?2, ?3, ?3, 'reserved', ?4, ?5, ?6, NULL)",
            params![
                operation_id,
                prepared.quote_id,
                now,
                prepared.total_masq_wei.to_string(),
                estimated_l2_fee_wei.to_string(),
                i64::try_from(signed.len()).unwrap_or(i64::MAX),
            ],
        )
        .map_err(|_| "The settlement operation could not be reserved.".to_owned())?;

    for transaction in signed {
        let reservation_id = reservation_id(transaction.nonce)?;
        let (amount_high, amount_low) = split_u128(transaction.amount_wei)?;
        let (gas_high, gas_low) = split_u128(transaction.gas_price_wei)?;
        db_tx
            .execute(
                "INSERT INTO mobile_debt_settlement_transaction
                 (tx_hash, operation_id, receiver_address, amount_high_b, amount_low_b,
                  gas_price_high_b, gas_price_low_b, nonce, reservation_id, raw_transaction, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'reserved')",
                params![
                    format!("{:?}", transaction.hash),
                    operation_id,
                    format!("{:?}", transaction.receiver_address),
                    amount_high,
                    amount_low,
                    gas_high,
                    gas_low,
                    i64::try_from(transaction.nonce).map_err(|_| {
                        "The settlement nonce is outside the supported range.".to_owned()
                    })?,
                    reservation_id,
                    transaction.raw_transaction.0,
                ],
            )
            .map_err(|_| "A signed settlement transaction could not be stored.".to_owned())?;
        let changed = db_tx
            .execute(
                "UPDATE payable SET pending_payable_rowid = ?1
                 WHERE wallet_address = ?2 AND pending_payable_rowid IS NULL
                   AND balance_high_b = ?3 AND balance_low_b = ?4",
                params![
                    reservation_id,
                    format!("{:?}", transaction.receiver_address),
                    amount_high,
                    amount_low,
                ],
            )
            .map_err(|_| "A MASQ debt could not be reserved.".to_owned())?;
        // Never silently advance a persisted operation if its reserved row disappeared.
        if changed != 1 {
            return Err("A MASQ debt changed while the settlement was being reserved.".to_owned());
        }
    }
    db_tx
        .commit()
        .map_err(|_| "The signed settlement reservation could not be committed.".to_owned())
}

fn record_submission_result(
    conn: &Connection,
    operation_id: &str,
    transaction: &SignedDebtTransaction,
    exact_hash: bool,
) -> Result<(), String> {
    let status = if exact_hash { "submitted" } else { "ambiguous" };
    let changed = conn
        .execute(
            "UPDATE mobile_debt_settlement_transaction SET status = ?1
             WHERE operation_id = ?2 AND tx_hash = ?3",
            params![status, operation_id, format!("{:?}", transaction.hash)],
        )
        .map_err(|_| "The settlement submission result could not be stored.".to_owned())?;
    if changed != 1 {
        return Err("A settlement submission record could not be found.".to_owned());
    }
    Ok(())
}

fn read_operation_transactions(
    conn: &Connection,
    operation_id: &str,
) -> Result<Vec<StoredTransaction>, String> {
    let mut statement = conn
        .prepare(
            "SELECT tx_hash, receiver_address, amount_high_b, amount_low_b, nonce,
                    reservation_id, raw_transaction, status
             FROM mobile_debt_settlement_transaction WHERE operation_id = ?1 ORDER BY nonce ASC",
        )
        .map_err(|_| "The settlement transactions could not be prepared.".to_owned())?;
    let rows = statement
        .query_map(params![operation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Vec<u8>>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(|_| "The settlement transactions could not be read.".to_owned())?;
    rows.map(|row| {
        let (hash, receiver, high, low, nonce, reservation_id, raw_transaction, status) =
            row.map_err(|_| "A settlement transaction record is invalid.".to_owned())?;
        Ok(StoredTransaction {
            hash: H256::from_str(hash.trim_start_matches("0x"))
                .map_err(|_| "A settlement transaction hash is invalid.".to_owned())?,
            receiver_address: Address::from_str(receiver.trim_start_matches("0x"))
                .map_err(|_| "A settlement recipient record is invalid.".to_owned())?,
            amount_wei: u128::try_from(BigIntDivider::reconstitute(high, low))
                .map_err(|_| "A settlement amount record is invalid.".to_owned())?,
            nonce: u64::try_from(nonce)
                .map_err(|_| "A settlement nonce record is invalid.".to_owned())?,
            reservation_id,
            raw_transaction: Bytes(raw_transaction),
            status,
        })
    })
    .collect()
}

fn confirm_transaction(
    conn: &Connection,
    operation_id: &str,
    transaction: &StoredTransaction,
) -> Result<(), String> {
    let db_tx = conn
        .unchecked_transaction()
        .map_err(|_| "The confirmed settlement could not be started.".to_owned())?;
    let (current_high, current_low): (i64, i64) = db_tx
        .query_row(
            "SELECT balance_high_b, balance_low_b FROM payable
             WHERE wallet_address = ?1 AND pending_payable_rowid = ?2",
            params![
                format!("{:?}", transaction.receiver_address),
                transaction.reservation_id
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "The reserved MASQ debt could not be found.".to_owned())?;
    let current = BigIntDivider::reconstitute(current_high, current_low);
    let paid = i128::try_from(transaction.amount_wei)
        .map_err(|_| "The confirmed MASQ amount is outside the supported range.".to_owned())?;
    let remaining = current
        .checked_sub(paid)
        .ok_or_else(|| "The confirmed MASQ debt subtraction overflowed.".to_owned())?;
    if remaining < 0 {
        return Err("The confirmed MASQ payment exceeds the stored debt.".to_owned());
    }
    let (remaining_high, remaining_low) = BigIntDivider::deconstruct(remaining);
    let payable_changed = db_tx
        .execute(
            "UPDATE payable SET balance_high_b = ?1, balance_low_b = ?2,
             last_paid_timestamp = ?3, pending_payable_rowid = NULL
             WHERE wallet_address = ?4 AND pending_payable_rowid = ?5",
            params![
                remaining_high,
                remaining_low,
                to_unix_timestamp(SystemTime::now()),
                format!("{:?}", transaction.receiver_address),
                transaction.reservation_id,
            ],
        )
        .map_err(|_| "The confirmed MASQ debt could not be updated.".to_owned())?;
    if payable_changed != 1 {
        return Err("The reserved MASQ debt changed before confirmation.".to_owned());
    }
    let transaction_changed = db_tx
        .execute(
            "UPDATE mobile_debt_settlement_transaction SET status = 'confirmed'
             WHERE operation_id = ?1 AND tx_hash = ?2",
            params![operation_id, format!("{:?}", transaction.hash)],
        )
        .map_err(|_| "The confirmed settlement status could not be stored.".to_owned())?;
    if transaction_changed != 1 {
        return Err("The confirmed settlement transaction could not be found.".to_owned());
    }
    db_tx
        .commit()
        .map_err(|_| "The confirmed settlement could not be committed.".to_owned())
}

fn release_reverted_transaction(
    conn: &Connection,
    operation_id: &str,
    transaction: &StoredTransaction,
) -> Result<(), String> {
    let db_tx = conn
        .unchecked_transaction()
        .map_err(|_| "The reverted settlement could not be started.".to_owned())?;
    let payable_changed = db_tx
        .execute(
            "UPDATE payable SET pending_payable_rowid = NULL
             WHERE wallet_address = ?1 AND pending_payable_rowid = ?2",
            params![
                format!("{:?}", transaction.receiver_address),
                transaction.reservation_id
            ],
        )
        .map_err(|_| "The reverted debt reservation could not be released.".to_owned())?;
    if payable_changed != 1 {
        return Err("The reverted MASQ debt reservation could not be found.".to_owned());
    }
    let transaction_changed = db_tx
        .execute(
            "UPDATE mobile_debt_settlement_transaction SET status = 'reverted'
             WHERE operation_id = ?1 AND tx_hash = ?2",
            params![operation_id, format!("{:?}", transaction.hash)],
        )
        .map_err(|_| "The reverted settlement status could not be stored.".to_owned())?;
    if transaction_changed != 1 {
        return Err("The reverted settlement transaction could not be found.".to_owned());
    }
    db_tx
        .commit()
        .map_err(|_| "The reverted settlement could not be committed.".to_owned())
}

fn update_operation_phase(conn: &Connection, operation_id: &str) -> Result<(), String> {
    let (total, confirmed, reverted): (i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*),
                    SUM(CASE WHEN status = 'confirmed' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status = 'reverted' THEN 1 ELSE 0 END)
             FROM mobile_debt_settlement_transaction WHERE operation_id = ?1",
            params![operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| "The settlement completion state could not be calculated.".to_owned())?;
    if total > 0 && confirmed == total {
        set_operation_phase(conn, operation_id, "completed", None)
    } else if total > 0 && confirmed + reverted == total {
        set_operation_phase(conn, operation_id, "failed", Some("E_SETTLEMENT_REVERTED"))
    } else if reverted > 0 {
        set_operation_phase(
            conn,
            operation_id,
            "attention",
            Some("E_SETTLEMENT_REVERTED"),
        )
    } else {
        Ok(())
    }
}

fn mark_operation_attention(
    conn: &Connection,
    operation_id: &str,
    error_code: &str,
) -> Result<(), String> {
    set_operation_phase(conn, operation_id, "attention", Some(error_code))
}

fn set_operation_phase(
    conn: &Connection,
    operation_id: &str,
    phase: &str,
    error_code: Option<&str>,
) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE mobile_debt_settlement_operation
         SET phase = ?1, updated_at = ?2, error_code = ?3 WHERE operation_id = ?4",
            params![
                phase,
                to_unix_timestamp(SystemTime::now()),
                error_code,
                operation_id
            ],
        )
        .map_err(|_| "The settlement operation status could not be stored.".to_owned())?;
    if changed == 1 {
        Ok(())
    } else {
        Err("The settlement operation could not be found.".to_owned())
    }
}

fn read_operation_status(
    conn: &Connection,
    requested_operation_id: Option<&str>,
) -> Result<PublicSettlementStatus, String> {
    let row = if let Some(operation_id) = requested_operation_id {
        conn.query_row(
            "SELECT operation_id, phase, total_masq_wei, estimated_l2_fee_wei,
                    transaction_count, error_code
             FROM mobile_debt_settlement_operation WHERE operation_id = ?1",
            params![operation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()
    } else {
        conn.query_row(
            "SELECT operation_id, phase, total_masq_wei, estimated_l2_fee_wei,
                    transaction_count, error_code
             FROM mobile_debt_settlement_operation ORDER BY created_at DESC LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()
    }
    .map_err(|_| "The settlement operation could not be read.".to_owned())?;

    let Some((operation_id, phase, total, fee, transaction_count, error_code)) = row else {
        return Ok(PublicSettlementStatus {
            operation_id: None,
            phase: "idle".to_owned(),
            total_masq_wei: "0".to_owned(),
            estimated_l2_fee_wei: "0".to_owned(),
            transaction_count: 0,
            confirmed_transaction_count: 0,
            transaction_hashes: vec![],
            error_code: None,
        });
    };
    let mut statement = conn
        .prepare(
            "SELECT tx_hash, status FROM mobile_debt_settlement_transaction
             WHERE operation_id = ?1 ORDER BY nonce ASC",
        )
        .map_err(|_| "The settlement transaction status could not be prepared.".to_owned())?;
    let transactions = statement
        .query_map(params![operation_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| "The settlement transaction status could not be read.".to_owned())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "A settlement transaction status is invalid.".to_owned())?;
    Ok(PublicSettlementStatus {
        operation_id: Some(operation_id),
        phase,
        total_masq_wei: total,
        estimated_l2_fee_wei: fee,
        transaction_count: usize::try_from(transaction_count).unwrap_or(usize::MAX),
        confirmed_transaction_count: transactions
            .iter()
            .filter(|(_, status)| status == "confirmed")
            .count(),
        transaction_hashes: transactions.into_iter().map(|(hash, _)| hash).collect(),
        error_code,
    })
}

fn split_u128(value: u128) -> Result<(i64, i64), String> {
    i128::try_from(value)
        .map(BigIntDivider::deconstruct)
        .map_err(|_| "A settlement value is outside the supported database range.".to_owned())
}

fn reservation_id(nonce: u64) -> Result<i64, String> {
    let nonce = i64::try_from(nonce)
        .map_err(|_| "The settlement nonce is outside the supported range.".to_owned())?;
    nonce
        .checked_add(1)
        .and_then(i64::checked_neg)
        .ok_or_else(|| "The settlement reservation identifier overflowed.".to_owned())
}

fn quote_identifier(chain: Chain, created_at: SystemTime, entries: &[DebtEntry]) -> String {
    let mut value = Vec::new();
    value.extend_from_slice(chain.rec().literal_identifier.as_bytes());
    value.extend_from_slice(&unix_nanos(created_at).to_be_bytes());
    for entry in entries {
        value.extend_from_slice(entry.address.as_bytes());
        value.extend_from_slice(&entry.amount_wei.to_be_bytes());
        value.extend_from_slice(&entry.last_paid_timestamp.to_be_bytes());
    }
    let output = value.keccak256();
    hex_identifier(&output[..16])
}

fn operation_identifier(quote_id: &str, created_at: SystemTime) -> String {
    let mut value = Vec::new();
    value.extend_from_slice(quote_id.as_bytes());
    value.extend_from_slice(&unix_nanos(created_at).to_be_bytes());
    let output = value.keccak256();
    hex_identifier(&output[..16])
}

fn unix_nanos(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn hex_identifier(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(HEX[(byte >> 4) as usize] as char);
        value.push(HEX[(byte & 0x0f) as usize] as char);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_database() -> (TempDir, Connection) {
        let directory = TempDir::new().unwrap();
        let conn = Connection::open(directory.path().join(DATABASE_FILE)).unwrap();
        conn.execute_batch(
            "CREATE TABLE payable (
                wallet_address TEXT PRIMARY KEY,
                balance_high_b INTEGER NOT NULL,
                balance_low_b INTEGER NOT NULL,
                last_paid_timestamp INTEGER NOT NULL,
                pending_payable_rowid INTEGER NULL
             ) STRICT;",
        )
        .unwrap();
        ensure_mobile_tables(&conn).unwrap();
        (directory, conn)
    }

    #[test]
    fn summary_includes_reserved_debts_but_marks_an_active_operation() {
        let (directory, conn) = setup_database();
        conn.execute(
            "INSERT INTO payable VALUES (?1, 0, 42, 1, -1)",
            params!["0x0000000000000000000000000000000000000001"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mobile_debt_settlement_operation VALUES
             ('operation', 'quote', 1, 1, 'submitted', '42', '7', 1, NULL)",
            [],
        )
        .unwrap();
        drop(conn);

        let summary = debt_summary(directory.path()).unwrap();
        assert_eq!(summary.total_masq_wei, "42");
        assert_eq!(summary.creditor_count, 1);
        assert!(summary.settlement_in_progress);
    }

    #[test]
    fn summary_ignores_zero_and_negative_payable_rows() {
        let (directory, conn) = setup_database();
        conn.execute(
            "INSERT INTO payable VALUES (?1, -1, ?2, 1, NULL)",
            params!["0x0000000000000000000000000000000000000001", i64::MAX],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO payable VALUES (?1, 0, 0, 2, NULL)",
            params!["0x0000000000000000000000000000000000000002"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO payable VALUES (?1, 0, 42, 3, NULL)",
            params!["0x0000000000000000000000000000000000000003"],
        )
        .unwrap();
        drop(conn);

        let summary = debt_summary(directory.path()).unwrap();
        assert_eq!(summary.total_masq_wei, "42");
        assert_eq!(summary.creditor_count, 1);
    }

    #[test]
    fn quote_matching_is_invalidated_by_new_or_changed_oldest_debt() {
        let (_directory, conn) = setup_database();
        conn.execute(
            "INSERT INTO payable VALUES (?1, 0, 42, 1, NULL)",
            params!["0x0000000000000000000000000000000000000001"],
        )
        .unwrap();
        let entries = read_debts(&conn, true).unwrap();
        let prepared = PreparedDebtSettlement {
            quote_id: "quote".to_owned(),
            created_at: SystemTime::now(),
            expires_at: SystemTime::now() + QUOTE_VALIDITY,
            total_masq_wei: 42,
            estimated_l2_fee_wei: 7,
            masq_balance_wei: 100,
            base_eth_balance_wei: 100,
            creditor_count: 1,
            has_more_creditors: false,
            entries,
        };
        assert!(ensure_quote_still_matches(&conn, &prepared).is_ok());
        conn.execute(
            "UPDATE payable SET balance_low_b = 43 WHERE wallet_address = ?1",
            params!["0x0000000000000000000000000000000000000001"],
        )
        .unwrap();
        assert!(ensure_quote_still_matches(&conn, &prepared).is_err());
    }

    #[test]
    fn identifiers_do_not_reveal_wallet_addresses() {
        let entry = DebtEntry {
            address: Address::from_low_u64_be(0x1234),
            amount_wei: 99,
            last_paid_timestamp: 1,
        };
        let quote = quote_identifier(Chain::BaseMainnet, UNIX_EPOCH, &[entry]);
        assert_eq!(quote.len(), 32);
        assert!(!quote.contains("1234"));
    }

    #[test]
    fn reservation_is_atomic_and_confirmation_preserves_newly_accrued_debt() {
        let (_directory, mut conn) = setup_database();
        let address = Address::from_low_u64_be(1);
        conn.execute(
            "INSERT INTO payable VALUES (?1, 0, 42, 1, NULL)",
            params![format!("{:?}", address)],
        )
        .unwrap();
        let entry = DebtEntry {
            address,
            amount_wei: 42,
            last_paid_timestamp: 1,
        };
        let prepared = PreparedDebtSettlement {
            quote_id: "quote".to_owned(),
            created_at: SystemTime::now(),
            expires_at: SystemTime::now() + QUOTE_VALIDITY,
            total_masq_wei: 42,
            estimated_l2_fee_wei: 7,
            masq_balance_wei: 100,
            base_eth_balance_wei: 100,
            creditor_count: 1,
            has_more_creditors: false,
            entries: vec![entry],
        };
        let hash = H256::from_low_u64_be(9);
        let signed = SignedDebtTransaction {
            hash,
            receiver_address: address,
            amount_wei: 42,
            gas_price_wei: 7,
            nonce: 3,
            raw_transaction: Bytes(vec![1, 2, 3]),
        };

        persist_reservation(&mut conn, "operation", &prepared, 7, &[signed]).unwrap();
        let pending: i64 = conn
            .query_row(
                "SELECT pending_payable_rowid FROM payable WHERE wallet_address = ?1",
                params![format!("{:?}", address)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, -4);
        assert_eq!(
            read_operation_transactions(&conn, "operation")
                .unwrap()
                .len(),
            1
        );

        // Routing can accrue another eight wei while the already-signed amount is pending.
        conn.execute(
            "UPDATE payable SET balance_low_b = 50 WHERE wallet_address = ?1",
            params![format!("{:?}", address)],
        )
        .unwrap();
        let stored = read_operation_transactions(&conn, "operation")
            .unwrap()
            .remove(0);
        confirm_transaction(&conn, "operation", &stored).unwrap();
        let (remaining, released): (i64, Option<i64>) = conn
            .query_row(
                "SELECT balance_low_b, pending_payable_rowid FROM payable WHERE wallet_address = ?1",
                params![format!("{:?}", address)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(remaining, 8);
        assert_eq!(released, None);
    }

    #[test]
    fn nonmatching_rpc_hash_is_recorded_as_ambiguous_and_never_as_submitted() {
        let (_directory, conn) = setup_database();
        conn.execute(
            "INSERT INTO mobile_debt_settlement_operation VALUES
             ('operation', 'quote', 1, 1, 'reserved', '42', '7', 1, NULL)",
            [],
        )
        .unwrap();
        let address = Address::from_low_u64_be(1);
        let hash = H256::from_low_u64_be(9);
        conn.execute(
            "INSERT INTO mobile_debt_settlement_transaction VALUES
             (?1, 'operation', ?2, 0, 42, 0, 7, 3, -4, X'01', 'reserved')",
            params![format!("{:?}", hash), format!("{:?}", address)],
        )
        .unwrap();
        let signed = SignedDebtTransaction {
            hash,
            receiver_address: address,
            amount_wei: 42,
            gas_price_wei: 7,
            nonce: 3,
            raw_transaction: Bytes(vec![1]),
        };

        record_submission_result(&conn, "operation", &signed, false).unwrap();
        mark_operation_attention(&conn, "operation", "E_SETTLEMENT_RPC_AMBIGUOUS").unwrap();
        let status = read_operation_status(&conn, Some("operation")).unwrap();
        assert_eq!(status.phase, "attention");
        assert_eq!(
            status.error_code.as_deref(),
            Some("E_SETTLEMENT_RPC_AMBIGUOUS")
        );
        let stored_status: String = conn
            .query_row(
                "SELECT status FROM mobile_debt_settlement_transaction WHERE operation_id = 'operation'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_status, "ambiguous");
    }

    #[test]
    fn exact_rpc_hash_is_recorded_as_submitted() {
        let (_directory, conn) = setup_database();
        conn.execute(
            "INSERT INTO mobile_debt_settlement_operation VALUES
             ('operation', 'quote', 1, 1, 'reserved', '42', '7', 1, NULL)",
            [],
        )
        .unwrap();
        let address = Address::from_low_u64_be(1);
        let hash = H256::from_low_u64_be(9);
        conn.execute(
            "INSERT INTO mobile_debt_settlement_transaction VALUES
             (?1, 'operation', ?2, 0, 42, 0, 7, 3, -4, X'01', 'reserved')",
            params![format!("{:?}", hash), format!("{:?}", address)],
        )
        .unwrap();
        let signed = SignedDebtTransaction {
            hash,
            receiver_address: address,
            amount_wei: 42,
            gas_price_wei: 7,
            nonce: 3,
            raw_transaction: Bytes(vec![1]),
        };

        record_submission_result(&conn, "operation", &signed, true).unwrap();

        let stored_status: String = conn
            .query_row(
                "SELECT status FROM mobile_debt_settlement_transaction WHERE operation_id = 'operation'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_status, "submitted");
    }

    #[test]
    fn a_fully_reverted_operation_releases_the_debt_for_a_new_review() {
        let (directory, conn) = setup_database();
        let address = Address::from_low_u64_be(1);
        let hash = H256::from_low_u64_be(9);
        conn.execute(
            "INSERT INTO payable VALUES (?1, 0, 42, 1, -4)",
            params![format!("{:?}", address)],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mobile_debt_settlement_operation VALUES
             ('operation', 'quote', 1, 1, 'submitted', '42', '7', 1, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mobile_debt_settlement_transaction VALUES
             (?1, 'operation', ?2, 0, 42, 0, 7, 3, -4, X'01', 'submitted')",
            params![format!("{:?}", hash), format!("{:?}", address)],
        )
        .unwrap();

        let stored = read_operation_transactions(&conn, "operation")
            .unwrap()
            .remove(0);
        release_reverted_transaction(&conn, "operation", &stored).unwrap();
        update_operation_phase(&conn, "operation").unwrap();

        let status = read_operation_status(&conn, Some("operation")).unwrap();
        assert_eq!(status.phase, "failed");
        assert_eq!(status.error_code.as_deref(), Some("E_SETTLEMENT_REVERTED"));
        assert_eq!(active_operation_id(&conn).unwrap(), None);
        drop(conn);

        let summary = debt_summary(directory.path()).unwrap();
        assert_eq!(summary.total_masq_wei, "42");
        assert!(!summary.settlement_in_progress);
    }
}
