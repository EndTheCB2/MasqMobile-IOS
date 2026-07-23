// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::command_context::CommandContext;
use crate::commands::commands_common::{
    dump_parameter_line, transaction, Command, CommandError, STANDARD_COMMAND_TIMEOUT_MILLIS,
};
use clap::{App, AppSettings, Arg, SubCommand};
use masq_lib::as_any_ref_in_trait_impl;
use masq_lib::messages::{
    UiProviderSettlementActivateRequest, UiProviderSettlementActivateResponse,
    UiProviderSettlementExportRequest, UiProviderSettlementExportResponse,
    UiProviderSettlementProposalRequest, UiProviderSettlementProposalResponse,
    UiProviderSettlementReconcileRequest, UiProviderSettlementReconcileResponse,
    UiProviderSettlementStatusRequest, UiProviderSettlementStatusResponse,
    UiProviderSettlementStopRequest, UiProviderSettlementStopResponse,
};
use masq_lib::short_writeln;
use std::fmt::{Debug, Formatter};
use std::io::Write;

const MIN_DURATION_SECONDS: u64 = 60;
const MAX_DURATION_SECONDS: u64 = 30 * 24 * 60 * 60;
const MAX_EXPORT_CLAIMS: usize = 128;
const MIN_CONFIRMATION_DEPTH: u64 = 12;
const MAX_CONFIRMATION_DEPTH: u64 = 100_000;

#[derive(PartialEq, Eq)]
pub enum ProviderSettlementAction {
    Propose {
        duration_seconds: u64,
    },
    Activate {
        proposal_id: String,
        wallet_signature: String,
    },
    Status,
    Stop,
    Export {
        start_after_claim_id_opt: Option<String>,
        max_claims: usize,
    },
    Reconcile {
        start_after_claim_id_opt: Option<String>,
        max_claims: usize,
        confirmation_depth: u64,
    },
}

impl Debug for ProviderSettlementAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Propose { duration_seconds } => f
                .debug_struct("Propose")
                .field("duration_seconds", duration_seconds)
                .finish(),
            Self::Activate { .. } => {
                f.write_str("Activate { proposal_id: [REDACTED], wallet_signature: [REDACTED] }")
            }
            Self::Status => f.write_str("Status"),
            Self::Stop => f.write_str("Stop"),
            Self::Export {
                start_after_claim_id_opt,
                max_claims,
            } => f
                .debug_struct("Export")
                .field(
                    "start_after_claim_id_opt",
                    &start_after_claim_id_opt.as_ref().map(|_| "[REDACTED]"),
                )
                .field("max_claims", max_claims)
                .finish(),
            Self::Reconcile {
                start_after_claim_id_opt,
                max_claims,
                confirmation_depth,
            } => f
                .debug_struct("Reconcile")
                .field(
                    "start_after_claim_id_opt",
                    &start_after_claim_id_opt.as_ref().map(|_| "[REDACTED]"),
                )
                .field("max_claims", max_claims)
                .field("confirmation_depth", confirmation_depth)
                .finish(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ProviderSettlementCommand {
    pub action: ProviderSettlementAction,
}

pub fn provider_settlement_subcommand() -> App<'static, 'static> {
    SubCommand::with_name("provider-settlement")
        .about("Authorizes provider payouts, exports batches, and reconciles confirmed claims.")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("propose")
                .about("Creates EIP-712 typed data for the configured earning wallet.")
                .arg(
                    Arg::with_name("duration-seconds")
                        .long("duration-seconds")
                        .value_name("SECONDS")
                        .takes_value(true)
                        .required(true)
                        .validator(validate_duration)
                        .help("Payout authorization lifetime from 60 through 2592000 seconds."),
                ),
        )
        .subcommand(
            SubCommand::with_name("activate")
                .about("Activates a proposal with an external earning-wallet signature.")
                .arg(
                    Arg::with_name("proposal-id")
                        .long("proposal-id")
                        .value_name("0x-NONCE")
                        .takes_value(true)
                        .required(true)
                        .validator(|value| validate_hex_bytes(value, 32, "proposal ID")),
                )
                .arg(
                    Arg::with_name("wallet-signature")
                        .long("wallet-signature")
                        .value_name("0x-R-S-V")
                        .takes_value(true)
                        .required(true)
                        .validator(|value| validate_hex_bytes(value, 65, "wallet signature")),
                ),
        )
        .subcommand(SubCommand::with_name("status").about("Displays payout and claim status."))
        .subcommand(
            SubCommand::with_name("stop").about("Revokes the locally active payout authority."),
        )
        .subcommand(
            SubCommand::with_name("export")
                .about("Exports a deterministic, contract-compatible pending-claim page.")
                .arg(
                    Arg::with_name("start-after-claim-id")
                        .long("start-after-claim-id")
                        .value_name("0x-CLAIM-ID")
                        .takes_value(true)
                        .validator(|value| validate_hex_bytes(value, 32, "claim cursor"))
                        .help("Exclusive opaque cursor returned by the previous export page."),
                )
                .arg(
                    Arg::with_name("max-claims")
                        .long("max-claims")
                        .value_name("COUNT")
                        .takes_value(true)
                        .default_value("128")
                        .validator(validate_max_claims)
                        .help("Maximum claims in this page; the escrow contract limit is 128."),
                ),
        )
        .subcommand(
            SubCommand::with_name("reconcile")
                .about("Checks pending and archived claims at one confirmation-deep block hash.")
                .arg(
                    Arg::with_name("start-after-claim-id")
                        .long("start-after-claim-id")
                        .value_name("0x-CLAIM-ID")
                        .takes_value(true)
                        .validator(|value| validate_hex_bytes(value, 32, "claim cursor"))
                        .help(
                            "Exclusive opaque cursor returned by the previous reconciliation page.",
                        ),
                )
                .arg(
                    Arg::with_name("max-claims")
                        .long("max-claims")
                        .value_name("COUNT")
                        .takes_value(true)
                        .default_value("128")
                        .validator(validate_max_claims)
                        .help("Maximum pending or archived claims checked in this page."),
                )
                .arg(
                    Arg::with_name("confirmation-depth")
                        .long("confirmation-depth")
                        .value_name("BLOCKS")
                        .takes_value(true)
                        .default_value("64")
                        .validator(validate_confirmation_depth)
                        .help("Historical depth from chain head; once used it cannot be lowered."),
                ),
        )
}

fn validate_duration(value: String) -> Result<(), String> {
    match value.parse::<u64>() {
        Ok(seconds) if (MIN_DURATION_SECONDS..=MAX_DURATION_SECONDS).contains(&seconds) => Ok(()),
        _ => Err(format!(
            "must be an integer from {} through {}",
            MIN_DURATION_SECONDS, MAX_DURATION_SECONDS
        )),
    }
}

fn validate_max_claims(value: String) -> Result<(), String> {
    match value.parse::<usize>() {
        Ok(count) if (1..=MAX_EXPORT_CLAIMS).contains(&count) => Ok(()),
        _ => Err(format!(
            "must be an integer from 1 through {}",
            MAX_EXPORT_CLAIMS
        )),
    }
}

fn validate_confirmation_depth(value: String) -> Result<(), String> {
    match value.parse::<u64>() {
        Ok(depth) if (MIN_CONFIRMATION_DEPTH..=MAX_CONFIRMATION_DEPTH).contains(&depth) => Ok(()),
        _ => Err(format!(
            "must be an integer from {} through {}",
            MIN_CONFIRMATION_DEPTH, MAX_CONFIRMATION_DEPTH
        )),
    }
}

fn validate_hex_bytes(value: String, byte_count: usize, name: &str) -> Result<(), String> {
    let expected_len = 2 + byte_count * 2;
    if value.len() != expected_len
        || !value.starts_with("0x")
        || !value[2..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        Err(format!(
            "{} must be exactly {} bytes of 0x-prefixed hexadecimal",
            name, byte_count
        ))
    } else {
        Ok(())
    }
}

impl ProviderSettlementCommand {
    pub fn new(pieces: &[String]) -> Result<Self, String> {
        let matches = provider_settlement_subcommand()
            .get_matches_from_safe(pieces)
            .map_err(|error| format!("{}", error))?;
        let action = match matches.subcommand() {
            ("propose", Some(propose)) => ProviderSettlementAction::Propose {
                duration_seconds: propose
                    .value_of("duration-seconds")
                    .expect("duration-seconds is not properly required")
                    .parse()
                    .expect("duration-seconds is not properly validated"),
            },
            ("activate", Some(activate)) => ProviderSettlementAction::Activate {
                proposal_id: activate
                    .value_of("proposal-id")
                    .expect("proposal-id is not properly required")
                    .to_string(),
                wallet_signature: activate
                    .value_of("wallet-signature")
                    .expect("wallet-signature is not properly required")
                    .to_string(),
            },
            ("status", Some(_)) => ProviderSettlementAction::Status,
            ("stop", Some(_)) => ProviderSettlementAction::Stop,
            ("export", Some(export)) => ProviderSettlementAction::Export {
                start_after_claim_id_opt: export
                    .value_of("start-after-claim-id")
                    .map(str::to_string),
                max_claims: export
                    .value_of("max-claims")
                    .expect("max-claims has a default")
                    .parse()
                    .expect("max-claims is not properly validated"),
            },
            ("reconcile", Some(reconcile)) => ProviderSettlementAction::Reconcile {
                start_after_claim_id_opt: reconcile
                    .value_of("start-after-claim-id")
                    .map(str::to_string),
                max_claims: reconcile
                    .value_of("max-claims")
                    .expect("max-claims has a default")
                    .parse()
                    .expect("max-claims is not properly validated"),
                confirmation_depth: reconcile
                    .value_of("confirmation-depth")
                    .expect("confirmation-depth has a default")
                    .parse()
                    .expect("confirmation-depth is not properly validated"),
            },
            _ => unreachable!("provider-settlement subcommand is not properly required"),
        };
        Ok(Self { action })
    }

    fn dump_status(stream: &mut dyn Write, status: &UiProviderSettlementStatusResponse) {
        dump_parameter_line(
            stream,
            "Provider payout authorization:",
            if status.active { "active" } else { "inactive" },
        );
        dump_parameter_line(
            stream,
            "Pending settlement claims:",
            &status.pending_claim_count.to_string(),
        );
        if !status.active {
            return;
        }
        Self::dump_optional(stream, "Protocol version:", &status.protocol_version_opt);
        Self::dump_optional(stream, "Chain:", &status.chain_name_opt);
        Self::dump_optional(stream, "Chain ID:", &status.chain_id_opt);
        Self::dump_optional(
            stream,
            "MASQ token contract:",
            &status.masq_token_contract_opt,
        );
        Self::dump_optional(
            stream,
            "Settlement verifier:",
            &status.settlement_contract_opt,
        );
        Self::dump_optional(stream, "Payout wallet:", &status.payout_wallet_address_opt);
        Self::dump_optional(
            stream,
            "Provider public key:",
            &status.provider_public_key_opt,
        );
        Self::dump_optional(stream, "Authorization ID:", &status.authorization_id_opt);
        Self::dump_optional(
            stream,
            "Valid from (Unix seconds):",
            &status.valid_from_unix_s_opt,
        );
        Self::dump_optional(
            stream,
            "Expires at (Unix seconds):",
            &status.expires_at_unix_s_opt,
        );
    }

    fn dump_optional<T: ToString>(stream: &mut dyn Write, name: &str, value_opt: &Option<T>) {
        let value = value_opt
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "[missing]".to_string());
        dump_parameter_line(stream, name, &value);
    }
}

impl Command for ProviderSettlementCommand {
    fn execute(&self, context: &mut dyn CommandContext) -> Result<(), CommandError> {
        match &self.action {
            ProviderSettlementAction::Propose { duration_seconds } => {
                let response = transaction::<_, UiProviderSettlementProposalResponse>(
                    UiProviderSettlementProposalRequest {
                        duration_seconds: *duration_seconds,
                    },
                    context,
                    STANDARD_COMMAND_TIMEOUT_MILLIS,
                )?;
                let output = context.stdout();
                dump_parameter_line(
                    output,
                    "Provider payout authorization:",
                    "proposal pending signature",
                );
                dump_parameter_line(
                    output,
                    "Protocol version:",
                    &response.protocol_version.to_string(),
                );
                dump_parameter_line(output, "Chain:", &response.chain_name);
                dump_parameter_line(output, "Chain ID:", &response.chain_id.to_string());
                dump_parameter_line(
                    output,
                    "MASQ token contract:",
                    &response.masq_token_contract,
                );
                dump_parameter_line(
                    output,
                    "Settlement verifier:",
                    &response.settlement_contract,
                );
                dump_parameter_line(output, "Payout wallet:", &response.payout_wallet_address);
                dump_parameter_line(
                    output,
                    "Provider public key:",
                    &response.provider_public_key,
                );
                dump_parameter_line(output, "Authorization ID:", &response.authorization_id);
                dump_parameter_line(output, "Proposal ID:", &response.proposal_id);
                dump_parameter_line(
                    output,
                    "Valid from (Unix seconds):",
                    &response.valid_from_unix_s.to_string(),
                );
                dump_parameter_line(
                    output,
                    "Expires at (Unix seconds):",
                    &response.expires_at_unix_s.to_string(),
                );
                short_writeln!(output, "EIP-712 typed data:");
                short_writeln!(output, "{}", response.eip712_typed_data);
                Ok(())
            }
            ProviderSettlementAction::Activate {
                proposal_id,
                wallet_signature,
            } => {
                let response = transaction::<_, UiProviderSettlementActivateResponse>(
                    UiProviderSettlementActivateRequest {
                        proposal_id: proposal_id.clone(),
                        wallet_signature: wallet_signature.clone(),
                    },
                    context,
                    STANDARD_COMMAND_TIMEOUT_MILLIS,
                )?;
                Self::dump_status(context.stdout(), &response.status);
                Ok(())
            }
            ProviderSettlementAction::Status => {
                let response = transaction::<_, UiProviderSettlementStatusResponse>(
                    UiProviderSettlementStatusRequest {},
                    context,
                    STANDARD_COMMAND_TIMEOUT_MILLIS,
                )?;
                Self::dump_status(context.stdout(), &response);
                Ok(())
            }
            ProviderSettlementAction::Stop => {
                let response = transaction::<_, UiProviderSettlementStopResponse>(
                    UiProviderSettlementStopRequest {},
                    context,
                    STANDARD_COMMAND_TIMEOUT_MILLIS,
                )?;
                Self::dump_status(context.stdout(), &response.status);
                Ok(())
            }
            ProviderSettlementAction::Export {
                start_after_claim_id_opt,
                max_claims,
            } => {
                let response = transaction::<_, UiProviderSettlementExportResponse>(
                    UiProviderSettlementExportRequest {
                        start_after_claim_id_opt: start_after_claim_id_opt.clone(),
                        max_claims: *max_claims,
                    },
                    context,
                    STANDARD_COMMAND_TIMEOUT_MILLIS,
                )?;
                let output = context.stdout();
                dump_parameter_line(
                    output,
                    "Total pending claims:",
                    &response.total_pending_claims.to_string(),
                );
                dump_parameter_line(
                    output,
                    "Start-after claim ID:",
                    response
                        .start_after_claim_id_opt
                        .as_deref()
                        .unwrap_or("[first page]"),
                );
                dump_parameter_line(output, "Next cursor:", &response.next_cursor);
                dump_parameter_line(
                    output,
                    "Exported claims:",
                    &response.exported_claim_count.to_string(),
                );
                dump_parameter_line(output, "Chain ID:", &response.chain_id.to_string());
                dump_parameter_line(
                    output,
                    "Settlement verifier:",
                    &response.settlement_contract,
                );
                dump_parameter_line(output, "Portable Merkle root:", &response.merkle_root);
                dump_parameter_line(
                    output,
                    "Contract Merkle root:",
                    &response.contract_merkle_root,
                );
                dump_parameter_line(output, "Total claimed (wei):", &response.total_claimed_wei);
                short_writeln!(output, "Contract claims:");
                for (index, claim) in response.contract_claims.iter().enumerate() {
                    short_writeln!(
                        output,
                        "[{}] claimId={} sessionId={} payerWallet={} payoutWallet={} cumulativeChargeWei={}",
                        index,
                        claim.claim_id,
                        claim.session_id,
                        claim.payer_wallet_address,
                        claim.payout_wallet_address,
                        claim.cumulative_charge_wei
                    );
                }
                short_writeln!(output, "Canonical batch CBOR:");
                short_writeln!(output, "{}", response.batch_cbor);
                Ok(())
            }
            ProviderSettlementAction::Reconcile {
                start_after_claim_id_opt,
                max_claims,
                confirmation_depth,
            } => {
                let response = transaction::<_, UiProviderSettlementReconcileResponse>(
                    UiProviderSettlementReconcileRequest {
                        start_after_claim_id_opt: start_after_claim_id_opt.clone(),
                        max_claims: *max_claims,
                        confirmation_depth: *confirmation_depth,
                    },
                    context,
                    STANDARD_COMMAND_TIMEOUT_MILLIS,
                )?;
                let output = context.stdout();
                dump_parameter_line(
                    output,
                    "Start-after claim ID:",
                    response
                        .start_after_claim_id_opt
                        .as_deref()
                        .unwrap_or("[first page]"),
                );
                dump_parameter_line(output, "Next cursor:", &response.next_cursor);
                dump_parameter_line(
                    output,
                    "Queried claims:",
                    &response.queried_claim_count.to_string(),
                );
                dump_parameter_line(output, "Chain ID:", &response.chain_id.to_string());
                dump_parameter_line(
                    output,
                    "Settlement verifier:",
                    &response.settlement_contract,
                );
                dump_parameter_line(
                    output,
                    "Confirmation depth:",
                    &response.confirmation_depth.to_string(),
                );
                dump_parameter_line(
                    output,
                    "Latest block:",
                    &response.latest_block_number.to_string(),
                );
                dump_parameter_line(
                    output,
                    "Observed block:",
                    &response.observed_block_number.to_string(),
                );
                dump_parameter_line(
                    output,
                    "Observed block hash:",
                    &response.observed_block_hash,
                );
                dump_parameter_line(
                    output,
                    "Newly archived claims:",
                    &response.archived_claim_count.to_string(),
                );
                dump_parameter_line(
                    output,
                    "Reorg-restored claims:",
                    &response.restored_claim_count.to_string(),
                );
                dump_parameter_line(
                    output,
                    "Still pending claims:",
                    &response.still_pending_claim_count.to_string(),
                );
                dump_parameter_line(
                    output,
                    "Revalidated archived claims:",
                    &response.revalidated_archive_count.to_string(),
                );
                dump_parameter_line(
                    output,
                    "Unknown claims:",
                    &response.unknown_claim_count.to_string(),
                );
                Ok(())
            }
        }
    }

    as_any_ref_in_trait_impl!();
}

#[cfg(test)]
mod tests {
    #[test]
    fn debug_redacts_provider_activation_identity_and_signature() {
        let subject = ProviderSettlementAction::Activate {
            proposal_id: "SENSITIVE_PROPOSAL_ID".to_string(),
            wallet_signature: "SENSITIVE_WALLET_SIGNATURE".to_string(),
        };

        let debug = format!("{:?}", subject);

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("SENSITIVE_PROPOSAL_ID"));
        assert!(!debug.contains("SENSITIVE_WALLET_SIGNATURE"));
    }

    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_all_provider_settlement_actions_and_contract_cap() {
        assert_eq!(
            ProviderSettlementCommand::new(&strings(&[
                "provider-settlement",
                "propose",
                "--duration-seconds",
                "3600"
            ]))
            .unwrap()
            .action,
            ProviderSettlementAction::Propose {
                duration_seconds: 3600
            }
        );
        let cursor = format!("0x{}", "ab".repeat(32));
        assert_eq!(
            ProviderSettlementCommand::new(&vec![
                "provider-settlement".to_string(),
                "export".to_string(),
                "--start-after-claim-id".to_string(),
                cursor.clone(),
                "--max-claims".to_string(),
                "64".to_string(),
            ])
            .unwrap()
            .action,
            ProviderSettlementAction::Export {
                start_after_claim_id_opt: Some(cursor.clone()),
                max_claims: 64
            }
        );
        assert!(ProviderSettlementCommand::new(&strings(&[
            "provider-settlement",
            "export",
            "--max-claims",
            "129"
        ]))
        .is_err());
        assert_eq!(
            ProviderSettlementCommand::new(&vec![
                "provider-settlement".to_string(),
                "reconcile".to_string(),
                "--start-after-claim-id".to_string(),
                cursor.clone(),
                "--max-claims".to_string(),
                "32".to_string(),
                "--confirmation-depth".to_string(),
                "128".to_string(),
            ])
            .unwrap()
            .action,
            ProviderSettlementAction::Reconcile {
                start_after_claim_id_opt: Some(cursor),
                max_claims: 32,
                confirmation_depth: 128,
            }
        );
        assert!(ProviderSettlementCommand::new(&strings(&[
            "provider-settlement",
            "reconcile",
            "--confirmation-depth",
            "11"
        ]))
        .is_err());

        let proposal_id = format!("0x{}", "aa".repeat(32));
        let signature = format!("0x{}", "bb".repeat(65));
        assert_eq!(
            ProviderSettlementCommand::new(&vec![
                "provider-settlement".to_string(),
                "activate".to_string(),
                "--proposal-id".to_string(),
                proposal_id.clone(),
                "--wallet-signature".to_string(),
                signature.clone(),
            ])
            .unwrap()
            .action,
            ProviderSettlementAction::Activate {
                proposal_id,
                wallet_signature: signature
            }
        );
        assert_eq!(
            ProviderSettlementCommand::new(&strings(&["provider-settlement", "status"]))
                .unwrap()
                .action,
            ProviderSettlementAction::Status
        );
        assert_eq!(
            ProviderSettlementCommand::new(&strings(&["provider-settlement", "stop"]))
                .unwrap()
                .action,
            ProviderSettlementAction::Stop
        );
    }
}
