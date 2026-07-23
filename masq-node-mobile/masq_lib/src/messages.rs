// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::messages::UiMessageError::{DeserializationError, PayloadError, UnexpectedMessage};
use crate::shared_schema::ConfiguratorError;
use crate::ui_gateway::MessageBody;
use crate::ui_gateway::MessagePath::{Conversation, FireAndForget};
use crate::utils::to_string;
use core::fmt::Display;
use core::fmt::Formatter;
use itertools::Itertools;
use serde::de::DeserializeOwned;
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fmt::Debug;
use std::str::FromStr;

pub const NODE_UI_PROTOCOL: &str = "MASQNode-UIv2";

fn redacted_option<T>(value: &Option<T>) -> Option<&'static str> {
    value.as_ref().map(|_| "[REDACTED]")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiMessageError {
    UnexpectedMessage(MessageBody),
    PayloadError(MessageBody),
    DeserializationError(String, MessageBody),
}

impl UiMessageError {
    fn payload_summary(message_body: &MessageBody) -> String {
        match &message_body.payload {
            Ok(json) => format!("payload contents redacted ({} bytes)", json.len()),
            Err((code, _)) => format!("error payload code {}; message contents redacted", code),
        }
    }
}

impl fmt::Display for UiMessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            UnexpectedMessage(message_body) if message_body.path == FireAndForget => {
                write!(
                    f,
                    "Unexpected one-way message with opcode '{}'; {}",
                    message_body.opcode,
                    Self::payload_summary(message_body)
                )
            }
            UnexpectedMessage(message_body) => {
                let context_id = if let Conversation(context_id) = message_body.path {
                    context_id
                }
                else {
                    panic! ("MessageBody::Path suddenly switched from Conversation to FireAndForget")
                };
                write!(
                    f,
                    "Unexpected two-way message from context {} with opcode '{}'; {}",
                    context_id,
                    message_body.opcode,
                    Self::payload_summary(message_body)
                )
            },
            PayloadError(message_body) => {
                match &message_body.payload {
                    Ok (json) => write! (
                        f,
                        "Daemon or Node is acting erratically: PayloadError received for '{}' message with path '{:?}', but payload contained no error; payload contents redacted ({} bytes)",
                        message_body.opcode,
                        message_body.path,
                        json.len()
                    ),
                    Err ((code, message)) => write!(
                        f,
                        "Daemon or Node complained about your command with opcode '{}'. Error code {}: {}",
                        message_body.opcode, code, message
                    ),
                }
            },
            DeserializationError(message, message_body) => write!(
                f,
                "Could not deserialize message from Daemon or Node: {}; {}",
                message,
                Self::payload_summary(message_body)
            ),
        }
    }
}

pub trait ToMessageBody: serde::Serialize {
    fn tmb(self, context_id: u64) -> MessageBody;
    fn opcode(&self) -> &str;
    fn is_conversational(&self) -> bool;
}

pub trait FromMessageBody: DeserializeOwned + Debug {
    fn fmb(body: MessageBody) -> Result<(Self, u64), UiMessageError>;
}

macro_rules! fire_and_forget_message {
    ($message_type: ty, $opcode: expr) => {
        impl ToMessageBody for $message_type {
            fn tmb(self, _irrelevant: u64) -> MessageBody {
                let json = serde_json::to_string(&self).expect("Serialization problem");
                MessageBody {
                    opcode: $opcode.to_string(),
                    path: FireAndForget,
                    payload: Ok(json),
                }
            }

            fn opcode(&self) -> &'static str {
                Self::type_opcode()
            }

            fn is_conversational(&self) -> bool {
                Self::type_is_conversational()
            }
        }

        impl FromMessageBody for $message_type {
            fn fmb(body: MessageBody) -> Result<(Self, u64), UiMessageError> {
                if body.opcode != $opcode {
                    return Err(UiMessageError::UnexpectedMessage(body));
                };
                let payload = match &body.payload {
                    Ok(json) => match serde_json::from_str::<Self>(json) {
                        Ok(item) => item,
                        Err(e) => return Err(DeserializationError(format!("{:?}", e), body)),
                    },
                    Err(_) => return Err(PayloadError(body)),
                };
                if let Conversation(_) = &body.path {
                    return Err(UiMessageError::UnexpectedMessage(body));
                }
                Ok((payload, 0))
            }
        }

        impl $message_type {
            pub fn type_opcode() -> &'static str {
                $opcode
            }

            pub fn type_is_conversational() -> bool {
                false
            }
        }
    };
}

macro_rules! conversation_message {
    ($message_type: ty, $opcode: expr) => {
        impl ToMessageBody for $message_type {
            fn tmb(self, context_id: u64) -> MessageBody {
                let json = serde_json::to_string(&self).expect("Serialization problem");
                MessageBody {
                    opcode: $opcode.to_string(),
                    path: Conversation(context_id),
                    payload: Ok(json),
                }
            }

            fn opcode(&self) -> &'static str {
                Self::type_opcode()
            }

            fn is_conversational(&self) -> bool {
                Self::type_is_conversational()
            }
        }

        impl FromMessageBody for $message_type {
            fn fmb(body: MessageBody) -> Result<(Self, u64), UiMessageError> {
                if body.opcode != $opcode {
                    return Err(UiMessageError::UnexpectedMessage(body));
                };
                let payload = match &body.payload {
                    Ok(json) => match serde_json::from_str::<Self>(json) {
                        Ok(item) => item,
                        Err(e) => return Err(DeserializationError(format!("{:?}", e), body)),
                    },
                    Err(_) => return Err(PayloadError(body)),
                };
                let context_id = match &body.path {
                    Conversation(context_id) => context_id,
                    FireAndForget => return Err(UiMessageError::UnexpectedMessage(body)),
                };
                Ok((payload, *context_id))
            }
        }

        impl $message_type {
            pub fn type_opcode() -> &'static str {
                $opcode
            }

            pub fn type_is_conversational() -> bool {
                true
            }
        }
    };
}

///////////////////////////////////////////////////////////////////////
// These messages are sent only to and/or by the Daemon, not the Node
///////////////////////////////////////////////////////////////////////
// if a fire-and-forget message for the Node was detected but the Node is down
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiUndeliveredFireAndForget {
    pub opcode: String,
}
fire_and_forget_message!(UiUndeliveredFireAndForget, "undelivered");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiCrashRequest {
    pub actor: String,
    #[serde(rename = "panicMessage")]
    pub panic_message: String,
}
fire_and_forget_message!(UiCrashRequest, "crash");

impl UiCrashRequest {
    pub fn new(actor: &str, panic_message: &str) -> Self {
        Self {
            actor: actor.to_string(),
            panic_message: panic_message.to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiSetupRequestValue {
    pub name: String,
    pub value: Option<String>,
}

impl Debug for UiSetupRequestValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiSetupRequestValue")
            .field("name", &self.name)
            .field("value", &redacted_option(&self.value))
            .finish()
    }
}

impl UiSetupRequestValue {
    pub fn new(name: &str, value: &str) -> Self {
        UiSetupRequestValue {
            name: name.to_string(),
            value: Some(value.to_string()),
        }
    }

    pub fn clear(name: &str) -> Self {
        UiSetupRequestValue {
            name: name.to_string(),
            value: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiSetupRequest {
    pub values: Vec<UiSetupRequestValue>,
}
conversation_message!(UiSetupRequest, "setup");

impl UiSetupRequest {
    pub fn new(pairs: Vec<(&str, Option<&str>)>) -> UiSetupRequest {
        UiSetupRequest {
            values: pairs
                .into_iter()
                .map(|(name, value)| UiSetupRequestValue {
                    name: name.to_string(),
                    value: value.map(to_string),
                })
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum UiSetupResponseValueStatus {
    Default,
    Configured,
    Set,
    Blank,
    Required,
}

impl UiSetupResponseValueStatus {
    pub fn priority(self) -> u8 {
        match self {
            UiSetupResponseValueStatus::Blank => 0,
            UiSetupResponseValueStatus::Required => 0,
            UiSetupResponseValueStatus::Default => 1,
            UiSetupResponseValueStatus::Configured => 2,
            UiSetupResponseValueStatus::Set => 3,
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiSetupResponseValue {
    pub name: String,
    pub value: String,
    pub status: UiSetupResponseValueStatus,
}

impl Debug for UiSetupResponseValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiSetupResponseValue")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .field("status", &self.status)
            .finish()
    }
}

impl UiSetupResponseValue {
    pub fn new(
        name: &str,
        value: &str,
        status: UiSetupResponseValueStatus,
    ) -> UiSetupResponseValue {
        UiSetupResponseValue {
            name: name.to_string(),
            value: value.to_string(),
            status,
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiSetupResponse {
    pub running: bool,
    pub values: Vec<UiSetupResponseValue>,
    pub errors: Vec<(String, String)>,
}

impl Debug for UiSetupResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiSetupResponse")
            .field("running", &self.running)
            .field("values", &self.values)
            .field("error_count", &self.errors.len())
            .finish()
    }
}
conversation_message!(UiSetupResponse, "setup");
impl UiSetupResponse {
    pub fn new(
        running: bool,
        values: HashMap<String, UiSetupResponseValue>,
        errors: ConfiguratorError,
    ) -> UiSetupResponse {
        UiSetupResponse {
            running,
            values: values
                .into_iter()
                .sorted_by(|a, b| Ord::cmp(&a.0, &b.0))
                .map(|(_, v)| v)
                .collect(),
            errors: errors
                .param_errors
                .into_iter()
                .map(|pe| (pe.parameter, pe.reason))
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiSetupBroadcast {
    pub running: bool,
    pub values: Vec<UiSetupResponseValue>,
    pub errors: Vec<(String, String)>,
}

impl Debug for UiSetupBroadcast {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiSetupBroadcast")
            .field("running", &self.running)
            .field("values", &self.values)
            .field("error_count", &self.errors.len())
            .finish()
    }
}
fire_and_forget_message!(UiSetupBroadcast, "setup");
impl UiSetupBroadcast {
    pub fn new(
        running: bool,
        values: HashMap<String, UiSetupResponseValue>,
        errors: ConfiguratorError,
    ) -> UiSetupBroadcast {
        UiSetupBroadcast {
            running,
            values: values
                .into_iter()
                .sorted_by(|a, b| Ord::cmp(&a.0, &b.0))
                .map(|(_, v)| v)
                .collect(),
            errors: errors
                .param_errors
                .into_iter()
                .map(|pe| (pe.parameter, pe.reason))
                .collect(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UiSetupInner {
    pub running: bool,
    pub values: Vec<UiSetupResponseValue>,
    pub errors: Vec<(String, String)>,
}

impl From<UiSetupResponse> for UiSetupInner {
    fn from(input: UiSetupResponse) -> Self {
        Self {
            running: input.running,
            values: input.values,
            errors: input.errors,
        }
    }
}

impl From<UiSetupBroadcast> for UiSetupInner {
    fn from(input: UiSetupBroadcast) -> Self {
        Self {
            running: input.running,
            values: input.values,
            errors: input.errors,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct UiStartOrder {}
conversation_message!(UiStartOrder, "start");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct UiStartResponse {
    #[serde(rename = "newProcessId")]
    pub new_process_id: u32,
    #[serde(rename = "redirectUiPort")]
    pub redirect_ui_port: u16,
}
conversation_message!(UiStartResponse, "start");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum CrashReason {
    ChildWaitFailure(String),
    NoInformation,
    Unrecognized(String),
    DaemonCrashed,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiNodeCrashedBroadcast {
    #[serde(rename = "processId")]
    pub process_id: u32,
    #[serde(rename = "crashReason")]
    pub crash_reason: CrashReason,
}
fire_and_forget_message!(UiNodeCrashedBroadcast, "crashed");

#[derive(Serialize, Deserialize, PartialEq, Eq)]
pub struct UiRedirect {
    pub port: u16,
    pub opcode: String,
    #[serde(rename = "contextId")]
    pub context_id: Option<u64>,
    pub payload: String,
}
impl Debug for UiRedirect {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiRedirect")
            .field("port", &self.port)
            .field("opcode", &self.opcode)
            .field("context_id", &self.context_id)
            .field("payload", &"[REDACTED]")
            .field("payload_bytes", &self.payload.len())
            .finish()
    }
}
fire_and_forget_message!(UiRedirect, "redirect");

///////////////////////////////////////////////////////////////////
// These messages are sent to or by both the Daemon and the Node
///////////////////////////////////////////////////////////////////

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct UiUnmarshalError {
    pub message: String,
    #[serde(rename = "badData")]
    pub bad_data: String,
}
impl Debug for UiUnmarshalError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiUnmarshalError")
            .field("message", &"[REDACTED]")
            .field("bad_data_bytes", &self.bad_data.len())
            .finish()
    }
}
fire_and_forget_message!(UiUnmarshalError, "unmarshalError");

///////////////////////////////////////////////////////////////////
// These messages are sent to or by the Node only
///////////////////////////////////////////////////////////////////

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct UiChangePasswordRequest {
    #[serde(rename = "oldPasswordOpt")]
    pub old_password_opt: Option<String>,
    #[serde(rename = "newPassword")]
    pub new_password: String,
}

impl Debug for UiChangePasswordRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiChangePasswordRequest")
            .field("old_password_opt", &redacted_option(&self.old_password_opt))
            .field("new_password", &"[REDACTED]")
            .finish()
    }
}
conversation_message!(UiChangePasswordRequest, "changePassword");

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiChangePasswordResponse {}
conversation_message!(UiChangePasswordResponse, "changePassword");

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct UiCheckPasswordRequest {
    #[serde(rename = "dbPasswordOpt")]
    pub db_password_opt: Option<String>,
}

impl Debug for UiCheckPasswordRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiCheckPasswordRequest")
            .field("db_password_opt", &redacted_option(&self.db_password_opt))
            .finish()
    }
}
conversation_message!(UiCheckPasswordRequest, "checkPassword");

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiCheckPasswordResponse {
    pub matches: bool,
}
conversation_message!(UiCheckPasswordResponse, "checkPassword");

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiConfigurationChangedBroadcast {}
fire_and_forget_message!(UiConfigurationChangedBroadcast, "configurationChanged");

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct UiConfigurationRequest {
    #[serde(rename = "dbPasswordOpt")]
    pub db_password_opt: Option<String>,
}

impl Debug for UiConfigurationRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiConfigurationRequest")
            .field("db_password_opt", &redacted_option(&self.db_password_opt))
            .finish()
    }
}
conversation_message!(UiConfigurationRequest, "configuration");

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct UiConfigurationResponse {
    #[serde(rename = "blockchainServiceUrlOpt")]
    pub blockchain_service_url_opt: Option<String>,
    #[serde(rename = "chainName")]
    pub chain_name: String,
    #[serde(rename = "clandestinePort")]
    pub clandestine_port: u16,
    #[serde(rename = "currentSchemaVersion")]
    pub current_schema_version: String,
    #[serde(rename = "earningWalletAddressOpt")]
    pub earning_wallet_address_opt: Option<String>,
    #[serde(rename = "gasPrice")]
    pub gas_price: u64,
    #[serde(rename = "maxBlockCount")]
    pub max_block_count_opt: Option<u64>,
    #[serde(rename = "neighborhoodMode")]
    pub neighborhood_mode: String,
    #[serde(rename = "portMappingProtocol")]
    pub port_mapping_protocol_opt: Option<String>,
    #[serde(rename = "startBlock")]
    pub start_block_opt: Option<u64>,
    #[serde(rename = "consumingWalletPrivateKeyOpt")]
    pub consuming_wallet_private_key_opt: Option<String>,
    // This item is calculated from the private key, not stored in the database, so that
    // the UI doesn't need the code to derive address from private key.
    #[serde(rename = "consumingWalletAddressOpt")]
    pub consuming_wallet_address_opt: Option<String>,
    #[serde(rename = "pastNeighbors")]
    pub past_neighbors: Vec<String>,
    #[serde(rename = "paymentThresholds")]
    pub payment_thresholds: UiPaymentThresholds,
    #[serde(rename = "ratePack")]
    pub rate_pack: UiRatePack,
    #[serde(rename = "scanIntervals")]
    pub scan_intervals: UiScanIntervals,
}

impl Debug for UiConfigurationResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiConfigurationResponse")
            .field(
                "blockchain_service_url_opt",
                &redacted_option(&self.blockchain_service_url_opt),
            )
            .field("chain_name", &self.chain_name)
            .field("clandestine_port", &self.clandestine_port)
            .field("current_schema_version", &self.current_schema_version)
            .field(
                "earning_wallet_address_opt",
                &redacted_option(&self.earning_wallet_address_opt),
            )
            .field("gas_price", &self.gas_price)
            .field("max_block_count_opt", &self.max_block_count_opt)
            .field("neighborhood_mode", &self.neighborhood_mode)
            .field("port_mapping_protocol_opt", &self.port_mapping_protocol_opt)
            .field("start_block_opt", &self.start_block_opt)
            .field(
                "consuming_wallet_private_key_opt",
                &redacted_option(&self.consuming_wallet_private_key_opt),
            )
            .field(
                "consuming_wallet_address_opt",
                &redacted_option(&self.consuming_wallet_address_opt),
            )
            .field("past_neighbors_count", &self.past_neighbors.len())
            .field("payment_thresholds", &self.payment_thresholds)
            .field("rate_pack", &self.rate_pack)
            .field("scan_intervals", &self.scan_intervals)
            .finish()
    }
}

conversation_message!(UiConfigurationResponse, "configuration");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiRatePack {
    #[serde(rename = "routingByteRate")]
    pub routing_byte_rate: u64,
    #[serde(rename = "routingServiceRate")]
    pub routing_service_rate: u64,
    #[serde(rename = "exitByteRate")]
    pub exit_byte_rate: u64,
    #[serde(rename = "exitServiceRate")]
    pub exit_service_rate: u64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiScanIntervals {
    #[serde(rename = "payableSec")]
    pub payable_sec: u64,
    #[serde(rename = "pendingPayableSec")]
    pub pending_payable_sec: u64,
    #[serde(rename = "receivableSec")]
    pub receivable_sec: u64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiPaymentThresholds {
    #[serde(rename = "thresholdIntervalSec")]
    pub threshold_interval_sec: u64,
    #[serde(rename = "debtThresholdGwei")]
    pub debt_threshold_gwei: u64,
    #[serde(rename = "paymentGracePeriodSec")]
    pub payment_grace_period_sec: u64,
    #[serde(rename = "maturityThresholdSec")]
    pub maturity_threshold_sec: u64,
    #[serde(rename = "permanentDebtAllowedGwei")]
    pub permanent_debt_allowed_gwei: u64,
    #[serde(rename = "unbanBelowGwei")]
    pub unban_below_gwei: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum UiConnectionStage {
    NotConnected,
    ConnectedToNeighbor,
    RouteFound,
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UiConnectionStatusReason {
    EntryNodesUnreachable,
    RouteNotReady,
    RouteProofStale,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiConnectionChangeBroadcast {
    pub stage: UiConnectionStage,
}
fire_and_forget_message!(UiConnectionChangeBroadcast, "connectionChange");

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiConnectionStatusRequest {}

conversation_message!(UiConnectionStatusRequest, "connectionStatus");

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiConnectionStatusResponse {
    pub stage: UiConnectionStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<UiConnectionStatusReason>,
}

conversation_message!(UiConnectionStatusResponse, "connectionStatus");

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiDescriptorRequest {}
conversation_message!(UiDescriptorRequest, "descriptor");

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiDescriptorResponse {
    #[serde(rename = "nodeDescriptorOpt")]
    pub node_descriptor_opt: Option<String>,
}
conversation_message!(UiDescriptorResponse, "descriptor");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiFinancialsRequest {
    #[serde(rename = "statsRequired")]
    pub stats_required: bool,
    #[serde(rename = "topRecordsOpt")]
    pub top_records_opt: Option<TopRecordsConfig>,
    #[serde(rename = "customQueriesOpt")]
    pub custom_queries_opt: Option<CustomQueries>,
}
conversation_message!(UiFinancialsRequest, "financials");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
pub struct TopRecordsConfig {
    pub count: u16,
    #[serde(rename = "orderedBy")]
    pub ordered_by: TopRecordsOrdering,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
pub enum TopRecordsOrdering {
    Age,
    Balance,
}

impl TryFrom<&str> for TopRecordsOrdering {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Ok(match value {
            "balance" => Self::Balance,
            "age" => Self::Age,
            x => return Err(format!("Unrecognized ordering: '{}'", x)),
        })
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct CustomQueries {
    #[serde(rename = "payableOpt")]
    pub payable_opt: Option<RangeQuery<u64>>,
    #[serde(rename = "receivableOpt")]
    pub receivable_opt: Option<RangeQuery<i64>>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct RangeQuery<T> {
    #[serde(rename = "minAgeS")]
    pub min_age_s: u64,
    #[serde(rename = "maxAgeS")]
    pub max_age_s: u64,
    #[serde(rename = "minAmountGwei")]
    pub min_amount_gwei: T,
    #[serde(rename = "maxAmountGwei")]
    pub max_amount_gwei: T,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct UiFinancialsResponse {
    #[serde(rename = "statsOpt")]
    pub stats_opt: Option<UiFinancialStatistics>,
    #[serde(rename = "queryResultsOpt")]
    pub query_results_opt: Option<QueryResults>,
}
conversation_message!(UiFinancialsResponse, "financials");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiReceiptSessionProposalRequest {
    #[serde(rename = "maxTotalChargeWei")]
    pub max_total_charge_wei: String,
    #[serde(rename = "durationSeconds")]
    pub duration_seconds: u64,
}
conversation_message!(UiReceiptSessionProposalRequest, "receiptSessionProposal");

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiReceiptSessionProposalResponse {
    #[serde(rename = "proposalId")]
    pub proposal_id: String,
    #[serde(rename = "protocolVersion")]
    pub protocol_version: u16,
    #[serde(rename = "chainName")]
    pub chain_name: String,
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    #[serde(rename = "masqTokenContract")]
    pub masq_token_contract: String,
    #[serde(rename = "settlementContract")]
    pub settlement_contract: String,
    #[serde(rename = "payerWalletAddress")]
    pub payer_wallet_address: String,
    #[serde(rename = "payerSessionPublicKey")]
    pub payer_session_public_key: String,
    #[serde(rename = "maxTotalChargeWei")]
    pub max_total_charge_wei: String,
    #[serde(rename = "validFromUnixS")]
    pub valid_from_unix_s: u64,
    #[serde(rename = "expiresAtUnixS")]
    pub expires_at_unix_s: u64,
    #[serde(rename = "authorizationId")]
    pub authorization_id: String,
    #[serde(rename = "eip712TypedData")]
    pub eip712_typed_data: serde_json::Value,
}
impl Debug for UiReceiptSessionProposalResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiReceiptSessionProposalResponse")
            .field("protocol_version", &self.protocol_version)
            .field("chain_name", &self.chain_name)
            .field("chain_id", &self.chain_id)
            .field("proposal_and_authorization", &"[REDACTED]")
            .finish()
    }
}
conversation_message!(UiReceiptSessionProposalResponse, "receiptSessionProposal");

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiReceiptSessionActivateRequest {
    #[serde(rename = "proposalId")]
    pub proposal_id: String,
    /// A canonical 65-byte secp256k1 signature encoded as 0x-prefixed r || s || v.
    #[serde(rename = "walletSignature")]
    pub wallet_signature: String,
}
impl Debug for UiReceiptSessionActivateRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(
            "UiReceiptSessionActivateRequest { proposal_id: [REDACTED], wallet_signature: [REDACTED] }",
        )
    }
}
conversation_message!(UiReceiptSessionActivateRequest, "receiptSessionActivate");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiReceiptSessionStatusRequest {}
conversation_message!(UiReceiptSessionStatusRequest, "receiptSessionStatus");

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiReceiptSessionStatusResponse {
    pub active: bool,
    #[serde(rename = "protocolVersionOpt")]
    pub protocol_version_opt: Option<u16>,
    #[serde(rename = "chainNameOpt")]
    pub chain_name_opt: Option<String>,
    #[serde(rename = "chainIdOpt")]
    pub chain_id_opt: Option<u64>,
    #[serde(rename = "masqTokenContractOpt")]
    pub masq_token_contract_opt: Option<String>,
    #[serde(rename = "settlementContractOpt")]
    pub settlement_contract_opt: Option<String>,
    #[serde(rename = "payerWalletAddressOpt")]
    pub payer_wallet_address_opt: Option<String>,
    #[serde(rename = "payerSessionPublicKeyOpt")]
    pub payer_session_public_key_opt: Option<String>,
    #[serde(rename = "authorizationIdOpt")]
    pub authorization_id_opt: Option<String>,
    #[serde(rename = "maxTotalChargeWeiOpt")]
    pub max_total_charge_wei_opt: Option<String>,
    #[serde(rename = "spentChargeWeiOpt")]
    pub spent_charge_wei_opt: Option<String>,
    #[serde(rename = "remainingChargeWeiOpt")]
    pub remaining_charge_wei_opt: Option<String>,
    #[serde(rename = "validFromUnixSOpt")]
    pub valid_from_unix_s_opt: Option<u64>,
    #[serde(rename = "expiresAtUnixSOpt")]
    pub expires_at_unix_s_opt: Option<u64>,
}
impl Debug for UiReceiptSessionStatusResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiReceiptSessionStatusResponse")
            .field("active", &self.active)
            .field("authorization_and_accounting", &"[REDACTED]")
            .finish()
    }
}
conversation_message!(UiReceiptSessionStatusResponse, "receiptSessionStatus");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiReceiptSessionActivateResponse {
    pub status: UiReceiptSessionStatusResponse,
}
conversation_message!(UiReceiptSessionActivateResponse, "receiptSessionActivate");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiReceiptSessionStopRequest {}
conversation_message!(UiReceiptSessionStopRequest, "receiptSessionStop");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiReceiptSessionStopResponse {
    pub status: UiReceiptSessionStatusResponse,
}
conversation_message!(UiReceiptSessionStopResponse, "receiptSessionStop");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementProposalRequest {
    #[serde(rename = "durationSeconds")]
    pub duration_seconds: u64,
}
conversation_message!(
    UiProviderSettlementProposalRequest,
    "providerSettlementProposal"
);

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementProposalResponse {
    #[serde(rename = "proposalId")]
    pub proposal_id: String,
    #[serde(rename = "protocolVersion")]
    pub protocol_version: u16,
    #[serde(rename = "chainName")]
    pub chain_name: String,
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    #[serde(rename = "masqTokenContract")]
    pub masq_token_contract: String,
    #[serde(rename = "settlementContract")]
    pub settlement_contract: String,
    #[serde(rename = "payoutWalletAddress")]
    pub payout_wallet_address: String,
    #[serde(rename = "providerPublicKey")]
    pub provider_public_key: String,
    #[serde(rename = "authorizationId")]
    pub authorization_id: String,
    #[serde(rename = "validFromUnixS")]
    pub valid_from_unix_s: u64,
    #[serde(rename = "expiresAtUnixS")]
    pub expires_at_unix_s: u64,
    #[serde(rename = "eip712TypedData")]
    pub eip712_typed_data: serde_json::Value,
}
impl Debug for UiProviderSettlementProposalResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiProviderSettlementProposalResponse")
            .field("protocol_version", &self.protocol_version)
            .field("chain_name", &self.chain_name)
            .field("chain_id", &self.chain_id)
            .field("proposal_and_authorization", &"[REDACTED]")
            .finish()
    }
}
conversation_message!(
    UiProviderSettlementProposalResponse,
    "providerSettlementProposal"
);

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementActivateRequest {
    #[serde(rename = "proposalId")]
    pub proposal_id: String,
    #[serde(rename = "walletSignature")]
    pub wallet_signature: String,
}
impl Debug for UiProviderSettlementActivateRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(
            "UiProviderSettlementActivateRequest { proposal_id: [REDACTED], wallet_signature: [REDACTED] }",
        )
    }
}
conversation_message!(
    UiProviderSettlementActivateRequest,
    "providerSettlementActivate"
);

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementStatusRequest {}
conversation_message!(
    UiProviderSettlementStatusRequest,
    "providerSettlementStatus"
);

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementStatusResponse {
    pub active: bool,
    #[serde(rename = "protocolVersionOpt")]
    pub protocol_version_opt: Option<u16>,
    #[serde(rename = "chainNameOpt")]
    pub chain_name_opt: Option<String>,
    #[serde(rename = "chainIdOpt")]
    pub chain_id_opt: Option<u64>,
    #[serde(rename = "masqTokenContractOpt")]
    pub masq_token_contract_opt: Option<String>,
    #[serde(rename = "settlementContractOpt")]
    pub settlement_contract_opt: Option<String>,
    #[serde(rename = "payoutWalletAddressOpt")]
    pub payout_wallet_address_opt: Option<String>,
    #[serde(rename = "providerPublicKeyOpt")]
    pub provider_public_key_opt: Option<String>,
    #[serde(rename = "authorizationIdOpt")]
    pub authorization_id_opt: Option<String>,
    #[serde(rename = "validFromUnixSOpt")]
    pub valid_from_unix_s_opt: Option<u64>,
    #[serde(rename = "expiresAtUnixSOpt")]
    pub expires_at_unix_s_opt: Option<u64>,
    #[serde(rename = "pendingClaimCount")]
    pub pending_claim_count: usize,
}
impl Debug for UiProviderSettlementStatusResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiProviderSettlementStatusResponse")
            .field("active", &self.active)
            .field("pending_claim_count", &self.pending_claim_count)
            .field("identity_and_authorization", &"[REDACTED]")
            .finish()
    }
}
conversation_message!(
    UiProviderSettlementStatusResponse,
    "providerSettlementStatus"
);

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementActivateResponse {
    pub status: UiProviderSettlementStatusResponse,
}
conversation_message!(
    UiProviderSettlementActivateResponse,
    "providerSettlementActivate"
);

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementStopRequest {}
conversation_message!(UiProviderSettlementStopRequest, "providerSettlementStop");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementStopResponse {
    pub status: UiProviderSettlementStatusResponse,
}
conversation_message!(UiProviderSettlementStopResponse, "providerSettlementStop");

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementExportRequest {
    #[serde(rename = "startAfterClaimIdOpt")]
    pub start_after_claim_id_opt: Option<String>,
    #[serde(rename = "maxClaims")]
    pub max_claims: usize,
}
impl Debug for UiProviderSettlementExportRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiProviderSettlementExportRequest")
            .field(
                "start_after_claim_id_opt",
                &redacted_option(&self.start_after_claim_id_opt),
            )
            .field("max_claims", &self.max_claims)
            .finish()
    }
}
conversation_message!(
    UiProviderSettlementExportRequest,
    "providerSettlementExport"
);

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementContractClaim {
    #[serde(rename = "claimId")]
    pub claim_id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "payerWalletAddress")]
    pub payer_wallet_address: String,
    #[serde(rename = "payoutWalletAddress")]
    pub payout_wallet_address: String,
    #[serde(rename = "cumulativeChargeWei")]
    pub cumulative_charge_wei: String,
}
impl Debug for UiProviderSettlementContractClaim {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(
            "UiProviderSettlementContractClaim { claim_id: [REDACTED], session_id: [REDACTED], payer_wallet_address: [REDACTED], payout_wallet_address: [REDACTED], cumulative_charge_wei: [REDACTED] }",
        )
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementExportResponse {
    #[serde(rename = "totalPendingClaims")]
    pub total_pending_claims: usize,
    #[serde(rename = "startAfterClaimIdOpt")]
    pub start_after_claim_id_opt: Option<String>,
    #[serde(rename = "nextCursor")]
    pub next_cursor: String,
    #[serde(rename = "exportedClaimCount")]
    pub exported_claim_count: usize,
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    #[serde(rename = "settlementContract")]
    pub settlement_contract: String,
    #[serde(rename = "merkleRoot")]
    pub merkle_root: String,
    #[serde(rename = "contractMerkleRoot")]
    pub contract_merkle_root: String,
    #[serde(rename = "totalClaimedWei")]
    pub total_claimed_wei: String,
    #[serde(rename = "batchCbor")]
    pub batch_cbor: String,
    #[serde(rename = "contractClaims")]
    pub contract_claims: Vec<UiProviderSettlementContractClaim>,
}
impl Debug for UiProviderSettlementExportResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiProviderSettlementExportResponse")
            .field("total_pending_claims", &self.total_pending_claims)
            .field("exported_claim_count", &self.exported_claim_count)
            .field("chain_id", &self.chain_id)
            .field("private_batch_and_cursor_data", &"[REDACTED]")
            .finish()
    }
}
conversation_message!(
    UiProviderSettlementExportResponse,
    "providerSettlementExport"
);

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementReconcileRequest {
    #[serde(rename = "startAfterClaimIdOpt")]
    pub start_after_claim_id_opt: Option<String>,
    #[serde(rename = "maxClaims")]
    pub max_claims: usize,
    #[serde(rename = "confirmationDepth")]
    pub confirmation_depth: u64,
}
impl Debug for UiProviderSettlementReconcileRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiProviderSettlementReconcileRequest")
            .field(
                "start_after_claim_id_opt",
                &redacted_option(&self.start_after_claim_id_opt),
            )
            .field("max_claims", &self.max_claims)
            .field("confirmation_depth", &self.confirmation_depth)
            .finish()
    }
}
conversation_message!(
    UiProviderSettlementReconcileRequest,
    "providerSettlementReconcile"
);

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiProviderSettlementReconcileResponse {
    #[serde(rename = "startAfterClaimIdOpt")]
    pub start_after_claim_id_opt: Option<String>,
    #[serde(rename = "nextCursor")]
    pub next_cursor: String,
    #[serde(rename = "queriedClaimCount")]
    pub queried_claim_count: usize,
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    #[serde(rename = "settlementContract")]
    pub settlement_contract: String,
    #[serde(rename = "confirmationDepth")]
    pub confirmation_depth: u64,
    #[serde(rename = "latestBlockNumber")]
    pub latest_block_number: u64,
    #[serde(rename = "observedBlockNumber")]
    pub observed_block_number: u64,
    #[serde(rename = "observedBlockHash")]
    pub observed_block_hash: String,
    #[serde(rename = "archivedClaimCount")]
    pub archived_claim_count: usize,
    #[serde(rename = "restoredClaimCount")]
    pub restored_claim_count: usize,
    #[serde(rename = "stillPendingClaimCount")]
    pub still_pending_claim_count: usize,
    #[serde(rename = "revalidatedArchiveCount")]
    pub revalidated_archive_count: usize,
    #[serde(rename = "unknownClaimCount")]
    pub unknown_claim_count: usize,
}
impl Debug for UiProviderSettlementReconcileResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiProviderSettlementReconcileResponse")
            .field("queried_claim_count", &self.queried_claim_count)
            .field("chain_id", &self.chain_id)
            .field("confirmation_depth", &self.confirmation_depth)
            .field("latest_block_number", &self.latest_block_number)
            .field("observed_block_number", &self.observed_block_number)
            .field("archived_claim_count", &self.archived_claim_count)
            .field("restored_claim_count", &self.restored_claim_count)
            .field("still_pending_claim_count", &self.still_pending_claim_count)
            .field("revalidated_archive_count", &self.revalidated_archive_count)
            .field("unknown_claim_count", &self.unknown_claim_count)
            .field("cursor_contract_and_block_hash", &"[REDACTED]")
            .finish()
    }
}
conversation_message!(
    UiProviderSettlementReconcileResponse,
    "providerSettlementReconcile"
);

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct UiFinancialStatistics {
    #[serde(rename = "totalUnpaidAndPendingPayableGwei")]
    pub total_unpaid_and_pending_payable_gwei: u64,
    #[serde(rename = "totalPaidPayableGwei")]
    pub total_paid_payable_gwei: u64,
    #[serde(rename = "totalUnpaidReceivableGwei")]
    pub total_unpaid_receivable_gwei: i64,
    #[serde(rename = "totalPaidReceivableGwei")]
    pub total_paid_receivable_gwei: u64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct QueryResults {
    #[serde(rename = "payableOpt")]
    pub payable_opt: Option<Vec<UiPayableAccount>>,
    #[serde(rename = "receivableOpt")]
    pub receivable_opt: Option<Vec<UiReceivableAccount>>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct UiPayableAccount {
    pub wallet: String,
    #[serde(rename = "ageS")]
    pub age_s: u64,
    #[serde(rename = "balanceGwei")]
    pub balance_gwei: u64,
    #[serde(rename = "pendingPayableHashOpt")]
    pub pending_payable_hash_opt: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct UiReceivableAccount {
    pub wallet: String,
    #[serde(rename = "ageS")]
    pub age_s: u64,
    #[serde(rename = "balanceGwei")]
    pub balance_gwei: i64,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiGenerateSeedSpec {
    #[serde(rename = "mnemonicPhraseSizeOpt")]
    pub mnemonic_phrase_size_opt: Option<usize>,
    #[serde(rename = "mnemonicPhraseLanguageOpt")]
    pub mnemonic_phrase_language_opt: Option<String>,
    #[serde(rename = "mnemonicPassphraseOpt")]
    pub mnemonic_passphrase_opt: Option<String>,
}

impl Debug for UiGenerateSeedSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiGenerateSeedSpec")
            .field("mnemonic_phrase_size_opt", &self.mnemonic_phrase_size_opt)
            .field(
                "mnemonic_phrase_language_opt",
                &self.mnemonic_phrase_language_opt,
            )
            .field(
                "mnemonic_passphrase_opt",
                &redacted_option(&self.mnemonic_passphrase_opt),
            )
            .finish()
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct UiGenerateWalletsRequest {
    #[serde(rename = "dbPassword")]
    pub db_password: String,
    #[serde(rename = "seedSpecOpt")]
    pub seed_spec_opt: Option<UiGenerateSeedSpec>,
    #[serde(rename = "consumingDerivationPathOpt")]
    pub consuming_derivation_path_opt: Option<String>,
    #[serde(rename = "earningDerivationPathOpt")]
    pub earning_derivation_path_opt: Option<String>,
}

impl Debug for UiGenerateWalletsRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiGenerateWalletsRequest")
            .field("db_password", &"[REDACTED]")
            .field("seed_spec_opt", &self.seed_spec_opt)
            .field(
                "consuming_derivation_path_opt",
                &redacted_option(&self.consuming_derivation_path_opt),
            )
            .field(
                "earning_derivation_path_opt",
                &redacted_option(&self.earning_derivation_path_opt),
            )
            .finish()
    }
}
conversation_message!(UiGenerateWalletsRequest, "generateWallets");

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct UiGenerateWalletsResponse {
    #[serde(rename = "mnemonicPhraseOpt")]
    pub mnemonic_phrase_opt: Option<Vec<String>>,
    #[serde(rename = "consumingWalletAddress")]
    pub consuming_wallet_address: String,
    #[serde(rename = "consumingWalletPrivateKey")]
    pub consuming_wallet_private_key: String,
    #[serde(rename = "earningWalletAddress")]
    pub earning_wallet_address: String,
    #[serde(rename = "earningWalletPrivateKey")]
    pub earning_wallet_private_key: String,
}

impl Debug for UiGenerateWalletsResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiGenerateWalletsResponse")
            .field(
                "mnemonic_phrase_opt",
                &redacted_option(&self.mnemonic_phrase_opt),
            )
            .field("consuming_wallet_address", &"[REDACTED]")
            .field("consuming_wallet_private_key", &"[REDACTED]")
            .field("earning_wallet_address", &"[REDACTED]")
            .field("earning_wallet_private_key", &"[REDACTED]")
            .finish()
    }
}
conversation_message!(UiGenerateWalletsResponse, "generateWallets");

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiLogBroadcast {
    pub msg: String,
    #[serde(rename = "logLevel")]
    pub log_level: SerializableLogLevel,
}
fire_and_forget_message!(UiLogBroadcast, "logBroadcast");

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum SerializableLogLevel {
    Error,
    Warn,
    Info,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiNewPasswordBroadcast {}
fire_and_forget_message!(UiNewPasswordBroadcast, "newPassword");

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiRecoverSeedSpec {
    #[serde(rename = "mnemonicPhrase")]
    pub mnemonic_phrase: Vec<String>,
    #[serde(rename = "mnemonicPhraseLanguageOpt")]
    pub mnemonic_phrase_language_opt: Option<String>,
    #[serde(rename = "mnemonicPassphraseOpt")]
    pub mnemonic_passphrase_opt: Option<String>,
}

impl Debug for UiRecoverSeedSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiRecoverSeedSpec")
            .field("mnemonic_phrase_words", &self.mnemonic_phrase.len())
            .field(
                "mnemonic_phrase_language_opt",
                &self.mnemonic_phrase_language_opt,
            )
            .field(
                "mnemonic_passphrase_opt",
                &redacted_option(&self.mnemonic_passphrase_opt),
            )
            .finish()
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiRecoverWalletsRequest {
    #[serde(rename = "dbPassword")]
    pub db_password: String,
    #[serde(rename = "seedSpecOpt")]
    pub seed_spec_opt: Option<UiRecoverSeedSpec>,
    #[serde(rename = "consumingDerivationPathOpt")]
    pub consuming_derivation_path_opt: Option<String>,
    #[serde(rename = "consumingPrivateKeyOpt")]
    pub consuming_private_key_opt: Option<String>,
    #[serde(rename = "earningDerivationPathOpt")]
    pub earning_derivation_path_opt: Option<String>,
    #[serde(rename = "earningAddressOpt")]
    pub earning_address_opt: Option<String>,
}

impl Debug for UiRecoverWalletsRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiRecoverWalletsRequest")
            .field("db_password", &"[REDACTED]")
            .field("seed_spec_opt", &self.seed_spec_opt)
            .field(
                "consuming_derivation_path_opt",
                &redacted_option(&self.consuming_derivation_path_opt),
            )
            .field(
                "consuming_private_key_opt",
                &redacted_option(&self.consuming_private_key_opt),
            )
            .field(
                "earning_derivation_path_opt",
                &redacted_option(&self.earning_derivation_path_opt),
            )
            .field(
                "earning_address_opt",
                &redacted_option(&self.earning_address_opt),
            )
            .finish()
    }
}
conversation_message!(UiRecoverWalletsRequest, "recoverWallets");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiRecoverWalletsResponse {}
conversation_message!(UiRecoverWalletsResponse, "recoverWallets");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum ScanType {
    Payables,
    PendingPayables,
    Receivables,
}

impl FromStr for ScanType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            s if &s.to_lowercase() == "payables" => Ok(ScanType::Payables),
            s if &s.to_lowercase() == "pendingpayables" => Ok(ScanType::PendingPayables),
            s if &s.to_lowercase() == "receivables" => Ok(ScanType::Receivables),
            s => Err(format!("Unrecognized ScanType: '{}'", s)),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiScanRequest {
    #[serde(rename = "scanType")]
    pub scan_type: ScanType,
}
conversation_message!(UiScanRequest, "scan");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiScanResponse {}
conversation_message!(UiScanResponse, "scan");

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct UiSetConfigurationRequest {
    pub name: String,
    pub value: String,
}
impl Debug for UiSetConfigurationRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiSetConfigurationRequest")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
}
conversation_message!(UiSetConfigurationRequest, "setConfiguration");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiSetConfigurationResponse {}

conversation_message!(UiSetConfigurationResponse, "setConfiguration");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiShutdownRequest {}
conversation_message!(UiShutdownRequest, "shutdown");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct UiShutdownResponse {}
conversation_message!(UiShutdownResponse, "shutdown");

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct UiWalletAddressesRequest {
    #[serde(rename = "dbPassword")]
    pub db_password: String,
}

impl Debug for UiWalletAddressesRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiWalletAddressesRequest")
            .field("db_password", &"[REDACTED]")
            .finish()
    }
}

conversation_message!(UiWalletAddressesRequest, "walletAddresses");

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct UiWalletAddressesResponse {
    #[serde(rename = "consumingWalletAddress")]
    pub consuming_wallet_address: String,
    #[serde(rename = "earningWalletAddress")]
    pub earning_wallet_address: String,
}
impl Debug for UiWalletAddressesResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(
            "UiWalletAddressesResponse { consuming_wallet_address: [REDACTED], earning_wallet_address: [REDACTED] }",
        )
    }
}
conversation_message!(UiWalletAddressesResponse, "walletAddresses");

// CountryGroups are inbound data for ExitLocations from UI. These data structures could be enriched
// in the future according to future user interface needs of more specification
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CountryGroups {
    #[serde(rename = "countryCodes")]
    pub country_codes: Vec<String>,
    pub priority: usize,
}

impl From<(String, usize)> for CountryGroups {
    fn from((country, priority): (String, usize)) -> Self {
        CountryGroups {
            country_codes: country
                .split(',')
                .into_iter()
                .map(|x| x.to_string())
                .collect::<Vec<String>>(),
            priority: priority + 1,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiSetExitLocationRequest {
    #[serde(rename = "fallbackRouting")]
    pub fallback_routing: bool,
    #[serde(rename = "exitLocations")]
    pub exit_locations: Vec<CountryGroups>,
    #[serde(rename = "showCountries")]
    pub show_countries: bool,
}
conversation_message!(UiSetExitLocationRequest, "exitLocation");

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ExitLocation {
    #[serde(rename = "countryCodes")]
    pub country_codes: Vec<String>,
    pub priority: usize,
}

impl Display for ExitLocation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Country Codes: {:?}, Priority: {};",
            self.country_codes, self.priority
        )
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiSetExitLocationResponse {
    #[serde(rename = "fallbackRouting")]
    pub fallback_routing: bool,
    #[serde(rename = "exitCountrySelection")]
    pub exit_country_selection: Vec<ExitLocation>,
    #[serde(rename = "exitCountries")]
    pub exit_countries: Option<Vec<String>>,
    #[serde(rename = "missingCountries")]
    pub missing_countries: Vec<String>,
}
conversation_message!(UiSetExitLocationResponse, "exitLocation");

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiGetNeighborhoodGraphRequest {}

conversation_message!(UiGetNeighborhoodGraphRequest, "neighborhoodGraph");

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UiGetNeighborhoodGraphResponse {
    pub graph: String,
}

conversation_message!(UiGetNeighborhoodGraphResponse, "neighborhoodGraph");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::UiMessageError::{DeserializationError, PayloadError, UnexpectedMessage};
    use crate::ui_gateway::MessagePath::{Conversation, FireAndForget};

    #[test]
    fn constants_have_correct_values() {
        assert_eq!(NODE_UI_PROTOCOL, "MASQNode-UIv2");
    }

    #[test]
    fn sensitive_wallet_and_configuration_dtos_have_content_free_debug_output() {
        let sensitive = "SENSITIVE_UI_SECRET";
        let payment_thresholds = UiPaymentThresholds {
            threshold_interval_sec: 1,
            debt_threshold_gwei: 2,
            payment_grace_period_sec: 3,
            maturity_threshold_sec: 4,
            permanent_debt_allowed_gwei: 5,
            unban_below_gwei: 6,
        };
        let rate_pack = UiRatePack {
            routing_byte_rate: 1,
            routing_service_rate: 2,
            exit_byte_rate: 3,
            exit_service_rate: 4,
        };
        let scan_intervals = UiScanIntervals {
            payable_sec: 1,
            pending_payable_sec: 2,
            receivable_sec: 3,
        };
        let debug_outputs = vec![
            format!(
                "{:?}",
                UiSetupRequest::new(vec![("db-password", Some(sensitive))])
            ),
            format!(
                "{:?}",
                UiSetupResponse {
                    running: false,
                    values: vec![UiSetupResponseValue::new(
                        "blockchain-service-url",
                        sensitive,
                        UiSetupResponseValueStatus::Set,
                    )],
                    errors: vec![("db-password".to_string(), sensitive.to_string())],
                }
            ),
            format!(
                "{:?}",
                UiSetupBroadcast {
                    running: false,
                    values: vec![UiSetupResponseValue::new(
                        "consuming-private-key",
                        sensitive,
                        UiSetupResponseValueStatus::Set,
                    )],
                    errors: vec![("consuming-private-key".to_string(), sensitive.to_string())],
                }
            ),
            format!(
                "{:?}",
                UiRedirect {
                    port: 1234,
                    opcode: "setup".to_string(),
                    context_id: Some(1),
                    payload: sensitive.to_string(),
                }
            ),
            format!(
                "{:?}",
                UiUnmarshalError {
                    message: sensitive.to_string(),
                    bad_data: sensitive.to_string(),
                }
            ),
            format!(
                "{:?}",
                UiChangePasswordRequest {
                    old_password_opt: Some(sensitive.to_string()),
                    new_password: sensitive.to_string(),
                }
            ),
            format!(
                "{:?}",
                UiCheckPasswordRequest {
                    db_password_opt: Some(sensitive.to_string()),
                }
            ),
            format!(
                "{:?}",
                UiConfigurationRequest {
                    db_password_opt: Some(sensitive.to_string()),
                }
            ),
            format!(
                "{:?}",
                UiConfigurationResponse {
                    blockchain_service_url_opt: Some(sensitive.to_string()),
                    chain_name: "base-mainnet".to_string(),
                    clandestine_port: 1234,
                    current_schema_version: "19".to_string(),
                    earning_wallet_address_opt: Some(sensitive.to_string()),
                    gas_price: 1,
                    max_block_count_opt: Some(2),
                    neighborhood_mode: "standard".to_string(),
                    port_mapping_protocol_opt: Some("pcp".to_string()),
                    start_block_opt: Some(3),
                    consuming_wallet_private_key_opt: Some(sensitive.to_string()),
                    consuming_wallet_address_opt: Some(sensitive.to_string()),
                    past_neighbors: vec![sensitive.to_string()],
                    payment_thresholds,
                    rate_pack,
                    scan_intervals,
                }
            ),
            format!(
                "{:?}",
                UiGenerateWalletsRequest {
                    db_password: sensitive.to_string(),
                    seed_spec_opt: Some(UiGenerateSeedSpec {
                        mnemonic_phrase_size_opt: Some(12),
                        mnemonic_phrase_language_opt: Some("English".to_string()),
                        mnemonic_passphrase_opt: Some(sensitive.to_string()),
                    }),
                    consuming_derivation_path_opt: Some(sensitive.to_string()),
                    earning_derivation_path_opt: Some(sensitive.to_string()),
                }
            ),
            format!(
                "{:?}",
                UiGenerateWalletsResponse {
                    mnemonic_phrase_opt: Some(vec![sensitive.to_string()]),
                    consuming_wallet_address: sensitive.to_string(),
                    consuming_wallet_private_key: sensitive.to_string(),
                    earning_wallet_address: sensitive.to_string(),
                    earning_wallet_private_key: sensitive.to_string(),
                }
            ),
            format!(
                "{:?}",
                UiRecoverWalletsRequest {
                    db_password: sensitive.to_string(),
                    seed_spec_opt: Some(UiRecoverSeedSpec {
                        mnemonic_phrase: vec![sensitive.to_string()],
                        mnemonic_phrase_language_opt: Some("English".to_string()),
                        mnemonic_passphrase_opt: Some(sensitive.to_string()),
                    }),
                    consuming_derivation_path_opt: Some(sensitive.to_string()),
                    consuming_private_key_opt: Some(sensitive.to_string()),
                    earning_derivation_path_opt: Some(sensitive.to_string()),
                    earning_address_opt: Some(sensitive.to_string()),
                }
            ),
            format!(
                "{:?}",
                UiSetConfigurationRequest {
                    name: "db-password".to_string(),
                    value: sensitive.to_string(),
                }
            ),
            format!(
                "{:?}",
                UiWalletAddressesRequest {
                    db_password: sensitive.to_string(),
                }
            ),
            format!(
                "{:?}",
                UiWalletAddressesResponse {
                    consuming_wallet_address: sensitive.to_string(),
                    earning_wallet_address: sensitive.to_string(),
                }
            ),
        ];

        for debug_output in debug_outputs {
            assert!(debug_output.contains("[REDACTED]"), "{}", debug_output);
            assert!(!debug_output.contains(sensitive), "{}", debug_output);
        }
    }

    #[test]
    fn settlement_activation_dtos_have_content_free_debug_output() {
        let sensitive = "SENSITIVE_SETTLEMENT_IDENTITY";
        let debug_outputs = [
            format!(
                "{:?}",
                UiReceiptSessionActivateRequest {
                    proposal_id: sensitive.to_string(),
                    wallet_signature: sensitive.to_string(),
                }
            ),
            format!(
                "{:?}",
                UiProviderSettlementActivateRequest {
                    proposal_id: sensitive.to_string(),
                    wallet_signature: sensitive.to_string(),
                }
            ),
        ];

        for debug_output in debug_outputs {
            assert!(debug_output.contains("[REDACTED]"), "{}", debug_output);
            assert!(!debug_output.contains(sensitive), "{}", debug_output);
        }
    }

    #[test]
    fn provider_settlement_batch_dtos_have_content_free_debug_output() {
        let sensitive = "SENSITIVE_PROVIDER_BATCH_IDENTITY";
        let contract_claim = UiProviderSettlementContractClaim {
            claim_id: sensitive.to_string(),
            session_id: sensitive.to_string(),
            payer_wallet_address: sensitive.to_string(),
            payout_wallet_address: sensitive.to_string(),
            cumulative_charge_wei: sensitive.to_string(),
        };
        let debug_outputs = [
            format!(
                "{:?}",
                UiProviderSettlementExportRequest {
                    start_after_claim_id_opt: Some(sensitive.to_string()),
                    max_claims: 10,
                }
            ),
            format!("{:?}", contract_claim),
            format!(
                "{:?}",
                UiProviderSettlementExportResponse {
                    total_pending_claims: 10,
                    start_after_claim_id_opt: Some(sensitive.to_string()),
                    next_cursor: sensitive.to_string(),
                    exported_claim_count: 1,
                    chain_id: 84532,
                    settlement_contract: sensitive.to_string(),
                    merkle_root: sensitive.to_string(),
                    contract_merkle_root: sensitive.to_string(),
                    total_claimed_wei: sensitive.to_string(),
                    batch_cbor: sensitive.to_string(),
                    contract_claims: vec![],
                }
            ),
            format!(
                "{:?}",
                UiProviderSettlementReconcileRequest {
                    start_after_claim_id_opt: Some(sensitive.to_string()),
                    max_claims: 10,
                    confirmation_depth: 12,
                }
            ),
            format!(
                "{:?}",
                UiProviderSettlementReconcileResponse {
                    start_after_claim_id_opt: Some(sensitive.to_string()),
                    next_cursor: sensitive.to_string(),
                    queried_claim_count: 1,
                    chain_id: 84532,
                    settlement_contract: sensitive.to_string(),
                    confirmation_depth: 12,
                    latest_block_number: 100,
                    observed_block_number: 88,
                    observed_block_hash: sensitive.to_string(),
                    archived_claim_count: 1,
                    restored_claim_count: 0,
                    still_pending_claim_count: 0,
                    revalidated_archive_count: 0,
                    unknown_claim_count: 0,
                }
            ),
        ];

        for debug_output in debug_outputs {
            assert!(debug_output.contains("[REDACTED]"), "{}", debug_output);
            assert!(!debug_output.contains(sensitive), "{}", debug_output);
        }
    }

    #[test]
    fn connection_status_reason_is_additive_and_uses_stable_wire_codes() {
        let with_reason = serde_json::to_string(&UiConnectionStatusResponse {
            stage: UiConnectionStage::ConnectedToNeighbor,
            reason: Some(UiConnectionStatusReason::RouteProofStale),
        })
        .unwrap();
        let legacy_response: UiConnectionStatusResponse =
            serde_json::from_str(r#"{"stage":"NotConnected"}"#).unwrap();

        assert_eq!(
            with_reason,
            r#"{"stage":"ConnectedToNeighbor","reason":"ROUTE_PROOF_STALE"}"#
        );
        assert_eq!(legacy_response.reason, None);
    }

    #[test]
    fn receipt_session_ui_json_keeps_token_and_settlement_identities_explicit() {
        let proposal = UiReceiptSessionProposalResponse {
            proposal_id: "proposal".to_string(),
            protocol_version: 1,
            chain_name: "base-sepolia".to_string(),
            chain_id: 84532,
            masq_token_contract: "0x1111".to_string(),
            settlement_contract: "0x2222".to_string(),
            payer_wallet_address: "0x3333".to_string(),
            payer_session_public_key: "0x4444".to_string(),
            max_total_charge_wei: "500".to_string(),
            valid_from_unix_s: 1000,
            expires_at_unix_s: 1600,
            authorization_id: "0x5555".to_string(),
            eip712_typed_data: serde_json::json!({"domain": {"verifyingContract": "0x2222"}}),
        };
        let proposal_json = serde_json::to_value(&proposal).unwrap();
        assert_eq!(proposal_json["protocolVersion"], 1);
        assert_eq!(proposal_json["chainName"], "base-sepolia");
        assert_eq!(proposal_json["chainId"], 84532);
        assert_eq!(proposal_json["masqTokenContract"], "0x1111");
        assert_eq!(proposal_json["settlementContract"], "0x2222");
        assert_eq!(proposal_json["authorizationId"], "0x5555");
        assert_eq!(
            proposal_json["eip712TypedData"]["domain"]["verifyingContract"],
            proposal_json["settlementContract"]
        );

        let status = UiReceiptSessionStatusResponse {
            active: true,
            protocol_version_opt: Some(1),
            chain_name_opt: Some("base-sepolia".to_string()),
            chain_id_opt: Some(84532),
            masq_token_contract_opt: Some("0x1111".to_string()),
            settlement_contract_opt: Some("0x2222".to_string()),
            payer_wallet_address_opt: Some("0x3333".to_string()),
            payer_session_public_key_opt: Some("0x4444".to_string()),
            authorization_id_opt: Some("0x5555".to_string()),
            max_total_charge_wei_opt: Some("500".to_string()),
            spent_charge_wei_opt: Some("125".to_string()),
            remaining_charge_wei_opt: Some("375".to_string()),
            valid_from_unix_s_opt: Some(1000),
            expires_at_unix_s_opt: Some(1600),
        };
        let status_json = serde_json::to_value(&status).unwrap();
        assert_eq!(status_json["masqTokenContractOpt"], "0x1111");
        assert_eq!(status_json["settlementContractOpt"], "0x2222");
        assert_eq!(status_json["authorizationIdOpt"], "0x5555");
        assert_eq!(status_json["remainingChargeWeiOpt"], "375");

        for debug_output in [format!("{:?}", proposal), format!("{:?}", status)] {
            assert!(debug_output.contains("[REDACTED]"), "{}", debug_output);
            for sensitive in [
                "0x1111", "0x2222", "0x3333", "0x4444", "0x5555", "500", "125", "375",
            ] {
                assert!(!debug_output.contains(sensitive), "{}", debug_output);
            }
        }
    }

    #[test]
    fn provider_settlement_response_debug_redacts_identity_and_authorization() {
        let sensitive = "SENSITIVE_PROVIDER_IDENTITY";
        let proposal = UiProviderSettlementProposalResponse {
            proposal_id: sensitive.to_string(),
            protocol_version: 1,
            chain_name: "base-sepolia".to_string(),
            chain_id: 84532,
            masq_token_contract: sensitive.to_string(),
            settlement_contract: sensitive.to_string(),
            payout_wallet_address: sensitive.to_string(),
            provider_public_key: sensitive.to_string(),
            authorization_id: sensitive.to_string(),
            valid_from_unix_s: 1000,
            expires_at_unix_s: 1600,
            eip712_typed_data: serde_json::json!({ "sensitive": sensitive }),
        };
        let status = UiProviderSettlementStatusResponse {
            active: true,
            protocol_version_opt: Some(1),
            chain_name_opt: Some("base-sepolia".to_string()),
            chain_id_opt: Some(84532),
            masq_token_contract_opt: Some(sensitive.to_string()),
            settlement_contract_opt: Some(sensitive.to_string()),
            payout_wallet_address_opt: Some(sensitive.to_string()),
            provider_public_key_opt: Some(sensitive.to_string()),
            authorization_id_opt: Some(sensitive.to_string()),
            valid_from_unix_s_opt: Some(1000),
            expires_at_unix_s_opt: Some(1600),
            pending_claim_count: 7,
        };

        for debug_output in [format!("{:?}", proposal), format!("{:?}", status)] {
            assert!(debug_output.contains("[REDACTED]"), "{}", debug_output);
            assert!(!debug_output.contains(sensitive), "{}", debug_output);
        }
    }

    #[test]
    fn ui_message_errors_are_displayable() {
        let sensitive_payload = "{\"secret\":\"SENSITIVE_VALUE_SHOULD_NOT_LEAK\"}".to_string();
        let redacted_summary = format!(
            "payload contents redacted ({} bytes)",
            sensitive_payload.len()
        );
        assert_eq!(
            UnexpectedMessage(MessageBody {
                opcode: "opcode".to_string(),
                path: FireAndForget,
                payload: Ok(sensitive_payload.clone()),
            })
            .to_string(),
            format!(
                "Unexpected one-way message with opcode 'opcode'; {}",
                redacted_summary
            )
        );
        assert_eq!(
            UnexpectedMessage(MessageBody {
                opcode: "opcode".to_string(),
                path: Conversation(1234),
                payload: Ok(sensitive_payload.clone()),
            })
            .to_string(),
            format!(
                "Unexpected two-way message from context 1234 with opcode 'opcode'; {}",
                redacted_summary
            )
        );
        assert_eq!(
            PayloadError(MessageBody {
                opcode: "opcode".to_string(),
                path: Conversation (1234),
                payload: Err((1234, "Booga booga".to_string())),
            }).to_string(),
            "Daemon or Node complained about your command with opcode 'opcode'. Error code 1234: Booga booga"
                .to_string()
        );
        assert_eq!(
            PayloadError(MessageBody {
                opcode: "opcode".to_string(),
                path: Conversation (1234),
                payload: Ok(sensitive_payload.clone()),
            }).to_string(),
            format!("Daemon or Node is acting erratically: PayloadError received for 'opcode' message with path 'Conversation(1234)', but payload contained no error; {}", redacted_summary)
        );
        assert_eq!(
            DeserializationError(
                "Booga booga".to_string(),
                MessageBody {
                    opcode: "opcode".to_string(),
                    path: Conversation(1234),
                    payload: Ok(sensitive_payload),
                }
            )
            .to_string(),
            format!(
                "Could not deserialize message from Daemon or Node: Booga booga; {}",
                redacted_summary
            )
        );
        assert!(!redacted_summary.contains("SENSITIVE_VALUE_SHOULD_NOT_LEAK"));
    }

    #[test]
    fn ui_descriptor_methods_were_correctly_generated() {
        let subject = UiDescriptorResponse {
            node_descriptor_opt: Some("descriptor".to_string()),
        };

        assert_eq!(subject.opcode(), "descriptor");
        assert_eq!(subject.is_conversational(), true);
    }

    #[test]
    fn can_serialize_ui_descriptor_response() {
        let subject = UiDescriptorResponse {
            node_descriptor_opt: None,
        };
        let subject_json = serde_json::to_string(&subject).unwrap();

        let result: MessageBody = UiDescriptorResponse::tmb(subject, 1357);

        assert_eq!(
            result,
            MessageBody {
                opcode: "descriptor".to_string(),
                path: Conversation(1357),
                payload: Ok(subject_json)
            }
        );
    }

    #[test]
    fn can_deserialize_ui_descriptor_response_with_bad_opcode() {
        let json = r#"
            {
                "nodeDescriptorOpt": "descriptor"
            }
        "#
        .to_string();
        let message_body = MessageBody {
            opcode: "booga".to_string(),
            path: Conversation(1234),
            payload: Ok(json),
        };

        let result: Result<(UiDescriptorResponse, u64), UiMessageError> =
            UiDescriptorResponse::fmb(message_body.clone());

        assert_eq!(result, Err(UnexpectedMessage(message_body)))
    }

    #[test]
    fn can_deserialize_ui_descriptor_response_with_bad_path() {
        let json = r#"
            {
                "nodeDescriptorOpt": "descriptor"
            }
        "#
        .to_string();
        let message_body = MessageBody {
            opcode: "descriptor".to_string(),
            path: FireAndForget,
            payload: Ok(json),
        };

        let result: Result<(UiDescriptorResponse, u64), UiMessageError> =
            UiDescriptorResponse::fmb(message_body.clone());

        assert_eq!(result, Err(UnexpectedMessage(message_body)))
    }

    #[test]
    fn can_deserialize_ui_descriptor_response_with_bad_payload() {
        let message_body = MessageBody {
            opcode: "descriptor".to_string(),
            path: Conversation(1234),
            payload: Err((100, "error".to_string())),
        };

        let result: Result<(UiDescriptorResponse, u64), UiMessageError> =
            UiDescriptorResponse::fmb(message_body.clone());

        assert_eq!(result, Err(PayloadError(message_body)))
    }

    #[test]
    fn can_deserialize_unparseable_ui_descriptor_response() {
        let json = "} - unparseable - {".to_string();
        let message_body = MessageBody {
            opcode: "descriptor".to_string(),
            path: Conversation(1234),
            payload: Ok(json),
        };

        let result: Result<(UiDescriptorResponse, u64), UiMessageError> =
            UiDescriptorResponse::fmb(message_body.clone());

        assert_eq!(
            result,
            Err(DeserializationError(
                "Error(\"expected value\", line: 1, column: 1)".to_string(),
                message_body
            ))
        )
    }

    #[test]
    fn can_deserialize_ui_descriptor_response() {
        let json = r#"
            {
                "nodeDescriptorOpt": "descriptor"
            }
        "#
        .to_string();
        let message_body = MessageBody {
            opcode: "descriptor".to_string(),
            path: Conversation(4321),
            payload: Ok(json),
        };

        let result: Result<(UiDescriptorResponse, u64), UiMessageError> =
            UiDescriptorResponse::fmb(message_body);

        assert_eq!(
            result,
            Ok((
                UiDescriptorResponse {
                    node_descriptor_opt: Some("descriptor".to_string())
                },
                4321
            ))
        );
    }

    #[test]
    fn ui_unmarshal_error_methods_were_correctly_generated() {
        let subject = UiUnmarshalError {
            message: "".to_string(),
            bad_data: "".to_string(),
        };

        assert_eq!(subject.opcode(), "unmarshalError");
        assert_eq!(subject.is_conversational(), false);
    }

    #[test]
    fn can_serialize_ui_unmarshal_error() {
        let subject = UiUnmarshalError {
            message: "message".to_string(),
            bad_data: "bad_data".to_string(),
        };
        let subject_json = serde_json::to_string(&subject).unwrap();

        let result: MessageBody = subject.tmb(1357);

        assert_eq!(
            result,
            MessageBody {
                opcode: "unmarshalError".to_string(),
                path: FireAndForget,
                payload: Ok(subject_json)
            }
        );
    }

    #[test]
    fn can_deserialize_ui_unmarshal_error_with_bad_opcode() {
        let json = "{}".to_string();
        let message_body = MessageBody {
            opcode: "booga".to_string(),
            path: FireAndForget,
            payload: Ok(json),
        };

        let result: Result<(UiUnmarshalError, u64), UiMessageError> =
            UiUnmarshalError::fmb(message_body.clone());

        assert_eq!(result, Err(UnexpectedMessage(message_body)))
    }

    #[test]
    fn can_deserialize_ui_unmarshal_error_with_bad_path() {
        let json = r#"{"message": "message", "badData": "{\"name\": 4}"}"#.to_string();
        let message_body = MessageBody {
            opcode: "unmarshalError".to_string(),
            path: Conversation(0),
            payload: Ok(json),
        };

        let result: Result<(UiUnmarshalError, u64), UiMessageError> =
            UiUnmarshalError::fmb(message_body.clone());

        assert_eq!(result, Err(UnexpectedMessage(message_body)))
    }

    #[test]
    fn can_deserialize_ui_unmarshal_error_with_bad_payload() {
        let message_body = MessageBody {
            opcode: "unmarshalError".to_string(),
            path: FireAndForget,
            payload: Err((100, "error".to_string())),
        };

        let result: Result<(UiUnmarshalError, u64), UiMessageError> =
            UiUnmarshalError::fmb(message_body.clone());

        assert_eq!(result, Err(PayloadError(message_body)))
    }

    #[test]
    fn can_deserialize_unparseable_ui_unmarshal_error() {
        let json = "} - unparseable - {".to_string();
        let message_body = MessageBody {
            opcode: "unmarshalError".to_string(),
            path: FireAndForget,
            payload: Ok(json),
        };

        let result: Result<(UiUnmarshalError, u64), UiMessageError> =
            UiUnmarshalError::fmb(message_body.clone());

        assert_eq!(
            result,
            Err(DeserializationError(
                "Error(\"expected value\", line: 1, column: 1)".to_string(),
                message_body
            ))
        )
    }

    #[test]
    fn can_deserialize_ui_unmarshal_error() {
        let json = r#"{"message": "message", "badData": "{}"}"#.to_string();
        let message_body = MessageBody {
            opcode: "unmarshalError".to_string(),
            path: FireAndForget,
            payload: Ok(json),
        };

        let result: Result<(UiUnmarshalError, u64), UiMessageError> =
            UiUnmarshalError::fmb(message_body);

        assert_eq!(
            result,
            Ok((
                UiUnmarshalError {
                    message: "message".to_string(),
                    bad_data: "{}".to_string()
                },
                0
            ))
        );
    }

    #[test]
    fn scan_type_from_string_happy_path() {
        let result: Vec<ScanType> = vec![
            "Payables",
            "pAYABLES",
            "PendingPayables",
            "pENDINGpAYABLES",
            "Receivables",
            "rECEIVABLES",
        ]
        .into_iter()
        .map(|s| ScanType::from_str(s).unwrap())
        .collect();

        assert_eq!(
            result,
            vec![
                ScanType::Payables,
                ScanType::Payables,
                ScanType::PendingPayables,
                ScanType::PendingPayables,
                ScanType::Receivables,
                ScanType::Receivables,
            ]
        )
    }

    #[test]
    fn scan_type_from_string_error() {
        let result = ScanType::from_str("unrecognized");

        assert_eq!(
            result,
            Err("Unrecognized ScanType: 'unrecognized'".to_string())
        );
    }

    #[test]
    fn top_records_ordering_from_str() {
        assert_eq!(
            TopRecordsOrdering::try_from("balance").unwrap(),
            TopRecordsOrdering::Balance
        );
        assert_eq!(
            TopRecordsOrdering::try_from("age").unwrap(),
            TopRecordsOrdering::Age
        )
    }

    #[test]
    fn top_records_ordering_from_str_error() {
        assert_eq!(
            TopRecordsOrdering::try_from("upside-down"),
            Err("Unrecognized ordering: 'upside-down'".to_string())
        );
    }
}
