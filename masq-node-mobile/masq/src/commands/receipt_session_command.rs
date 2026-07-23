// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::command_context::CommandContext;
use crate::commands::commands_common::{
    dump_parameter_line, transaction, Command, CommandError, STANDARD_COMMAND_TIMEOUT_MILLIS,
};
use clap::{App, AppSettings, Arg, SubCommand};
use masq_lib::as_any_ref_in_trait_impl;
use masq_lib::messages::{
    UiReceiptSessionActivateRequest, UiReceiptSessionActivateResponse,
    UiReceiptSessionProposalRequest, UiReceiptSessionProposalResponse,
    UiReceiptSessionStatusRequest, UiReceiptSessionStatusResponse, UiReceiptSessionStopRequest,
    UiReceiptSessionStopResponse,
};
use masq_lib::short_writeln;
use std::fmt::{Debug, Formatter};
use std::io::Write;

const RECEIPT_SESSION_ABOUT: &str =
    "Creates and manages wallet-authorized, exactly metered MASQ receipt sessions.";
const PROPOSE_ABOUT: &str =
    "Creates EIP-712 typed data that the configured consuming wallet must sign.";
const ACTIVATE_ABOUT: &str = "Activates the pending proposal with its wallet signature.";
const STATUS_ABOUT: &str = "Displays the active receipt session and settlement identities.";
const STOP_ABOUT: &str = "Stops the active receipt session and clears its local recovery state.";

const MIN_DURATION_SECONDS: u64 = 60;
const MAX_DURATION_SECONDS: u64 = 24 * 60 * 60;
const MAX_TOTAL_CHARGE_WEI: u128 = (1u128 << 126) - 1;

#[derive(PartialEq, Eq)]
pub enum ReceiptSessionAction {
    Propose {
        max_total_charge_wei: String,
        duration_seconds: u64,
    },
    Activate {
        proposal_id: String,
        wallet_signature: String,
    },
    Status,
    Stop,
}

impl Debug for ReceiptSessionAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Propose {
                max_total_charge_wei,
                duration_seconds,
            } => f
                .debug_struct("Propose")
                .field("max_total_charge_wei", max_total_charge_wei)
                .field("duration_seconds", duration_seconds)
                .finish(),
            Self::Activate { .. } => {
                f.write_str("Activate { proposal_id: [REDACTED], wallet_signature: [REDACTED] }")
            }
            Self::Status => f.write_str("Status"),
            Self::Stop => f.write_str("Stop"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ReceiptSessionCommand {
    pub action: ReceiptSessionAction,
}

pub fn receipt_session_subcommand() -> App<'static, 'static> {
    SubCommand::with_name("receipt-session")
        .about(RECEIPT_SESSION_ABOUT)
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("propose")
                .about(PROPOSE_ABOUT)
                .arg(
                    Arg::with_name("max-total-charge-wei")
                        .long("max-total-charge-wei")
                        .value_name("WEI")
                        .takes_value(true)
                        .required(true)
                        .validator(validate_positive_decimal)
                        .help("Maximum aggregate MASQ charge authorized for this session, in wei."),
                )
                .arg(
                    Arg::with_name("duration-seconds")
                        .long("duration-seconds")
                        .value_name("SECONDS")
                        .takes_value(true)
                        .required(true)
                        .validator(validate_duration)
                        .help("Authorization lifetime from 60 through 86400 seconds."),
                ),
        )
        .subcommand(
            SubCommand::with_name("activate")
                .about(ACTIVATE_ABOUT)
                .arg(
                    Arg::with_name("proposal-id")
                        .long("proposal-id")
                        .value_name("0x-NONCE")
                        .takes_value(true)
                        .required(true)
                        .validator(|value| validate_hex_bytes(value, 32, "proposal ID"))
                        .help("The proposalId returned by receipt-session propose."),
                )
                .arg(
                    Arg::with_name("wallet-signature")
                        .long("wallet-signature")
                        .value_name("0x-R-S-V")
                        .takes_value(true)
                        .required(true)
                        .validator(|value| validate_hex_bytes(value, 65, "wallet signature"))
                        .help(
                            "Canonical 65-byte EIP-712 signature encoded as 0x-prefixed r||s||v.",
                        ),
                ),
        )
        .subcommand(SubCommand::with_name("status").about(STATUS_ABOUT))
        .subcommand(SubCommand::with_name("stop").about(STOP_ABOUT))
}

fn validate_positive_decimal(value: String) -> Result<(), String> {
    match value.parse::<u128>() {
        Ok(number) if (1..=MAX_TOTAL_CHARGE_WEI).contains(&number) => Ok(()),
        _ => Err(format!(
            "must be a positive base-10 integer no greater than {}",
            MAX_TOTAL_CHARGE_WEI
        )),
    }
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

impl ReceiptSessionCommand {
    pub fn new(pieces: &[String]) -> Result<Self, String> {
        let matches = receipt_session_subcommand()
            .get_matches_from_safe(pieces)
            .map_err(|error| format!("{}", error))?;
        let action = match matches.subcommand() {
            ("propose", Some(propose)) => ReceiptSessionAction::Propose {
                max_total_charge_wei: propose
                    .value_of("max-total-charge-wei")
                    .expect("max-total-charge-wei is not properly required")
                    .to_string(),
                duration_seconds: propose
                    .value_of("duration-seconds")
                    .expect("duration-seconds is not properly required")
                    .parse::<u64>()
                    .expect("duration-seconds is not properly validated"),
            },
            ("activate", Some(activate)) => ReceiptSessionAction::Activate {
                proposal_id: activate
                    .value_of("proposal-id")
                    .expect("proposal-id is not properly required")
                    .to_string(),
                wallet_signature: activate
                    .value_of("wallet-signature")
                    .expect("wallet-signature is not properly required")
                    .to_string(),
            },
            ("status", Some(_)) => ReceiptSessionAction::Status,
            ("stop", Some(_)) => ReceiptSessionAction::Stop,
            _ => unreachable!("receipt-session subcommand is not properly required"),
        };
        Ok(Self { action })
    }

    fn execute_propose(
        max_total_charge_wei: &str,
        duration_seconds: u64,
        context: &mut dyn CommandContext,
    ) -> Result<(), CommandError> {
        let response = transaction::<_, UiReceiptSessionProposalResponse>(
            UiReceiptSessionProposalRequest {
                max_total_charge_wei: max_total_charge_wei.to_string(),
                duration_seconds,
            },
            context,
            STANDARD_COMMAND_TIMEOUT_MILLIS,
        )?;
        Self::dump_proposal(context.stdout(), &response);
        Ok(())
    }

    fn execute_activate(
        proposal_id: &str,
        wallet_signature: &str,
        context: &mut dyn CommandContext,
    ) -> Result<(), CommandError> {
        let response = transaction::<_, UiReceiptSessionActivateResponse>(
            UiReceiptSessionActivateRequest {
                proposal_id: proposal_id.to_string(),
                wallet_signature: wallet_signature.to_string(),
            },
            context,
            STANDARD_COMMAND_TIMEOUT_MILLIS,
        )?;
        Self::dump_status(context.stdout(), &response.status);
        Ok(())
    }

    fn execute_status(context: &mut dyn CommandContext) -> Result<(), CommandError> {
        let response = transaction::<_, UiReceiptSessionStatusResponse>(
            UiReceiptSessionStatusRequest {},
            context,
            STANDARD_COMMAND_TIMEOUT_MILLIS,
        )?;
        Self::dump_status(context.stdout(), &response);
        Ok(())
    }

    fn execute_stop(context: &mut dyn CommandContext) -> Result<(), CommandError> {
        let response = transaction::<_, UiReceiptSessionStopResponse>(
            UiReceiptSessionStopRequest {},
            context,
            STANDARD_COMMAND_TIMEOUT_MILLIS,
        )?;
        Self::dump_status(context.stdout(), &response.status);
        Ok(())
    }

    fn dump_proposal(stream: &mut dyn Write, proposal: &UiReceiptSessionProposalResponse) {
        dump_parameter_line(stream, "Receipt session:", "proposal pending signature");
        dump_parameter_line(
            stream,
            "Protocol version:",
            &proposal.protocol_version.to_string(),
        );
        dump_parameter_line(stream, "Chain:", &proposal.chain_name);
        dump_parameter_line(stream, "Chain ID:", &proposal.chain_id.to_string());
        dump_parameter_line(
            stream,
            "MASQ token contract:",
            &proposal.masq_token_contract,
        );
        dump_parameter_line(
            stream,
            "Settlement verifier:",
            &proposal.settlement_contract,
        );
        dump_parameter_line(stream, "Payer wallet:", &proposal.payer_wallet_address);
        dump_parameter_line(
            stream,
            "Payer session public key:",
            &proposal.payer_session_public_key,
        );
        dump_parameter_line(stream, "Authorization ID:", &proposal.authorization_id);
        dump_parameter_line(stream, "Proposal ID:", &proposal.proposal_id);
        dump_parameter_line(
            stream,
            "Maximum total charge (wei):",
            &proposal.max_total_charge_wei,
        );
        dump_parameter_line(
            stream,
            "Valid from (Unix seconds):",
            &proposal.valid_from_unix_s.to_string(),
        );
        dump_parameter_line(
            stream,
            "Expires at (Unix seconds):",
            &proposal.expires_at_unix_s.to_string(),
        );
        short_writeln!(stream, "EIP-712 typed data:");
        short_writeln!(stream, "{}", proposal.eip712_typed_data);
    }

    fn dump_status(stream: &mut dyn Write, status: &UiReceiptSessionStatusResponse) {
        if !status.active {
            dump_parameter_line(stream, "Receipt session:", "inactive");
            return;
        }
        dump_parameter_line(stream, "Receipt session:", "active");
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
        Self::dump_optional(stream, "Payer wallet:", &status.payer_wallet_address_opt);
        Self::dump_optional(
            stream,
            "Payer session public key:",
            &status.payer_session_public_key_opt,
        );
        Self::dump_optional(stream, "Authorization ID:", &status.authorization_id_opt);
        Self::dump_optional(
            stream,
            "Maximum total charge (wei):",
            &status.max_total_charge_wei_opt,
        );
        Self::dump_optional(stream, "Spent charge (wei):", &status.spent_charge_wei_opt);
        Self::dump_optional(
            stream,
            "Remaining charge (wei):",
            &status.remaining_charge_wei_opt,
        );
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

impl Command for ReceiptSessionCommand {
    fn execute(&self, context: &mut dyn CommandContext) -> Result<(), CommandError> {
        match &self.action {
            ReceiptSessionAction::Propose {
                max_total_charge_wei,
                duration_seconds,
            } => Self::execute_propose(max_total_charge_wei, *duration_seconds, context),
            ReceiptSessionAction::Activate {
                proposal_id,
                wallet_signature,
            } => Self::execute_activate(proposal_id, wallet_signature, context),
            ReceiptSessionAction::Status => Self::execute_status(context),
            ReceiptSessionAction::Stop => Self::execute_stop(context),
        }
    }

    as_any_ref_in_trait_impl!();
}

#[cfg(test)]
mod tests {
    #[test]
    fn debug_redacts_receipt_activation_identity_and_signature() {
        let subject = ReceiptSessionAction::Activate {
            proposal_id: "SENSITIVE_PROPOSAL_ID".to_string(),
            wallet_signature: "SENSITIVE_WALLET_SIGNATURE".to_string(),
        };

        let debug = format!("{:?}", subject);

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("SENSITIVE_PROPOSAL_ID"));
        assert!(!debug.contains("SENSITIVE_WALLET_SIGNATURE"));
    }

    use super::*;
    use crate::command_context::ContextError;
    use crate::command_factory::{CommandFactory, CommandFactoryReal};
    use crate::commands::commands_common::CommandError::ConnectionProblem;
    use crate::test_utils::mocks::CommandContextMock;
    use masq_lib::messages::ToMessageBody;
    use std::sync::{Arc, Mutex};

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn active_status() -> UiReceiptSessionStatusResponse {
        UiReceiptSessionStatusResponse {
            active: true,
            protocol_version_opt: Some(1),
            chain_name_opt: Some("base-sepolia".to_string()),
            chain_id_opt: Some(84532),
            masq_token_contract_opt: Some("0x1111111111111111111111111111111111111111".to_string()),
            settlement_contract_opt: Some("0x2222222222222222222222222222222222222222".to_string()),
            payer_wallet_address_opt: Some(
                "0x3333333333333333333333333333333333333333".to_string(),
            ),
            payer_session_public_key_opt: Some(format!("0x{}", "44".repeat(32))),
            authorization_id_opt: Some(format!("0x{}", "55".repeat(32))),
            max_total_charge_wei_opt: Some("1000".to_string()),
            spent_charge_wei_opt: Some("250".to_string()),
            remaining_charge_wei_opt: Some("750".to_string()),
            valid_from_unix_s_opt: Some(100),
            expires_at_unix_s_opt: Some(3700),
        }
    }

    #[test]
    fn factory_parses_all_receipt_session_actions() {
        let factory = CommandFactoryReal::new();
        let proposal = factory
            .make(&strings(&[
                "receipt-session",
                "propose",
                "--max-total-charge-wei",
                "1000",
                "--duration-seconds",
                "3600",
            ]))
            .unwrap();
        assert_eq!(
            proposal
                .as_any()
                .downcast_ref::<ReceiptSessionCommand>()
                .unwrap(),
            &ReceiptSessionCommand {
                action: ReceiptSessionAction::Propose {
                    max_total_charge_wei: "1000".to_string(),
                    duration_seconds: 3600,
                }
            }
        );

        let proposal_id = format!("0x{}", "aa".repeat(32));
        let signature = format!("0x{}", "bb".repeat(65));
        let activation = factory
            .make(&strings(&[
                "receipt-session",
                "activate",
                "--proposal-id",
                &proposal_id,
                "--wallet-signature",
                &signature,
            ]))
            .unwrap();
        assert_eq!(
            activation
                .as_any()
                .downcast_ref::<ReceiptSessionCommand>()
                .unwrap(),
            &ReceiptSessionCommand {
                action: ReceiptSessionAction::Activate {
                    proposal_id,
                    wallet_signature: signature,
                }
            }
        );

        for (name, action) in &[
            ("status", ReceiptSessionAction::Status),
            ("stop", ReceiptSessionAction::Stop),
        ] {
            let command = factory.make(&strings(&["receipt-session", name])).unwrap();
            assert_eq!(
                &command
                    .as_any()
                    .downcast_ref::<ReceiptSessionCommand>()
                    .unwrap()
                    .action,
                action
            );
        }
    }

    #[test]
    fn parser_rejects_unsafe_or_ambiguous_values() {
        for pieces in &[
            strings(&[
                "receipt-session",
                "propose",
                "--max-total-charge-wei",
                "0",
                "--duration-seconds",
                "3600",
            ]),
            strings(&[
                "receipt-session",
                "propose",
                "--max-total-charge-wei",
                &MAX_TOTAL_CHARGE_WEI.saturating_add(1).to_string(),
                "--duration-seconds",
                "3600",
            ]),
            strings(&[
                "receipt-session",
                "propose",
                "--max-total-charge-wei",
                "1000",
                "--duration-seconds",
                "59",
            ]),
            strings(&[
                "receipt-session",
                "activate",
                "--proposal-id",
                "0x12",
                "--wallet-signature",
                &format!("0x{}", "bb".repeat(65)),
            ]),
        ] {
            assert!(ReceiptSessionCommand::new(pieces).is_err());
        }
    }

    #[test]
    fn proposal_transaction_prints_all_security_identities_and_typed_data() {
        let proposal_id = format!("0x{}", "aa".repeat(32));
        let authorization_id = format!("0x{}", "55".repeat(32));
        let response = UiReceiptSessionProposalResponse {
            proposal_id: proposal_id.clone(),
            protocol_version: 1,
            chain_name: "base-sepolia".to_string(),
            chain_id: 84532,
            masq_token_contract: "0x1111111111111111111111111111111111111111".to_string(),
            settlement_contract: "0x2222222222222222222222222222222222222222".to_string(),
            payer_wallet_address: "0x3333333333333333333333333333333333333333".to_string(),
            payer_session_public_key: format!("0x{}", "44".repeat(32)),
            max_total_charge_wei: "1000".to_string(),
            valid_from_unix_s: 100,
            expires_at_unix_s: 3700,
            authorization_id: authorization_id.clone(),
            eip712_typed_data:
                r#"{"domain":{"verifyingContract":"0x2222222222222222222222222222222222222222"}}"#
                    .parse()
                    .unwrap(),
        };
        let params = Arc::new(Mutex::new(vec![]));
        let mut context = CommandContextMock::new()
            .transact_params(&params)
            .transact_result(Ok(response.tmb(42)));
        let stdout = context.stdout_arc();
        let command = ReceiptSessionCommand {
            action: ReceiptSessionAction::Propose {
                max_total_charge_wei: "1000".to_string(),
                duration_seconds: 3600,
            },
        };

        assert_eq!(command.execute(&mut context), Ok(()));

        assert_eq!(
            *params.lock().unwrap(),
            vec![(
                UiReceiptSessionProposalRequest {
                    max_total_charge_wei: "1000".to_string(),
                    duration_seconds: 3600,
                }
                .tmb(0),
                STANDARD_COMMAND_TIMEOUT_MILLIS,
            )]
        );
        let output = stdout.lock().unwrap().get_string();
        for required in &[
            "base-sepolia",
            "84532",
            "0x1111111111111111111111111111111111111111",
            "0x2222222222222222222222222222222222222222",
            &authorization_id,
            &proposal_id,
            "EIP-712 typed data:",
            "verifyingContract",
        ] {
            assert!(
                output.contains(required),
                "missing {} in {}",
                required,
                output
            );
        }
    }

    #[test]
    fn activate_status_and_stop_send_the_right_messages_and_print_status() {
        let proposal_id = format!("0x{}", "aa".repeat(32));
        let signature = format!("0x{}", "bb".repeat(65));
        let cases = vec![
            (
                ReceiptSessionAction::Activate {
                    proposal_id: proposal_id.clone(),
                    wallet_signature: signature.clone(),
                },
                UiReceiptSessionActivateResponse {
                    status: active_status(),
                }
                .tmb(1),
                UiReceiptSessionActivateRequest {
                    proposal_id,
                    wallet_signature: signature,
                }
                .tmb(0),
            ),
            (
                ReceiptSessionAction::Status,
                active_status().tmb(1),
                UiReceiptSessionStatusRequest {}.tmb(0),
            ),
            (
                ReceiptSessionAction::Stop,
                UiReceiptSessionStopResponse {
                    status: active_status(),
                }
                .tmb(1),
                UiReceiptSessionStopRequest {}.tmb(0),
            ),
        ];

        for (action, response, expected_request) in cases {
            let params = Arc::new(Mutex::new(vec![]));
            let mut context = CommandContextMock::new()
                .transact_params(&params)
                .transact_result(Ok(response));
            let stdout = context.stdout_arc();
            assert_eq!(
                ReceiptSessionCommand { action }.execute(&mut context),
                Ok(())
            );
            assert_eq!(
                *params.lock().unwrap(),
                vec![(expected_request, STANDARD_COMMAND_TIMEOUT_MILLIS)]
            );
            let output = stdout.lock().unwrap().get_string();
            assert!(output.contains("active"));
            assert!(output.contains("MASQ token contract:"));
            assert!(output.contains("Settlement verifier:"));
            assert!(output.contains("Authorization ID:"));
            assert!(output.contains("Remaining charge (wei):"));
        }
    }

    #[test]
    fn inactive_status_is_unambiguous_and_transaction_errors_propagate() {
        let mut inactive_context =
            CommandContextMock::new().transact_result(Ok(UiReceiptSessionStatusResponse {
                active: false,
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
                remaining_charge_wei_opt: None,
                valid_from_unix_s_opt: None,
                expires_at_unix_s_opt: None,
            }
            .tmb(1)));
        let stdout = inactive_context.stdout_arc();
        assert_eq!(
            ReceiptSessionCommand {
                action: ReceiptSessionAction::Status
            }
            .execute(&mut inactive_context),
            Ok(())
        );
        assert!(stdout.lock().unwrap().get_string().contains("inactive"));

        let mut failed_context = CommandContextMock::new()
            .transact_result(Err(ContextError::ConnectionDropped("gone".to_string())));
        assert_eq!(
            ReceiptSessionCommand {
                action: ReceiptSessionAction::Status
            }
            .execute(&mut failed_context),
            Err(ConnectionProblem("gone".to_string()))
        );
    }
}
