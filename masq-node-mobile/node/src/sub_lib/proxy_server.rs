// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::sub_lib::cryptde::PublicKey;
use crate::sub_lib::data_version::DataVersion;
use crate::sub_lib::dispatcher::InboundClientData;
use crate::sub_lib::dispatcher::StreamShutdownMsg;
use crate::sub_lib::hopper::{ExpiredCoresPackage, MessageType};
use crate::sub_lib::neighborhood::RouteQueryResponse;
use crate::sub_lib::peer_actors::BindMessage;
use crate::sub_lib::proxy_client::{
    ClientResponsePayload_0v1, DnsResolveFailure_0v1, MeteredClientResponsePayload_0v1,
};
use crate::sub_lib::sequence_buffer::SequencedPacket;
use crate::sub_lib::service_receipt::ReceiptSessionRequest;
use crate::sub_lib::service_receipt::ServiceReceiptOfferPayload_0v1;
use crate::sub_lib::stream_key::StreamKey;
use crate::sub_lib::utils::MessageScheduler;
use crate::sub_lib::versioned_data::VersionedData;
use actix::Message;
use actix::Recipient;
use masq_lib::ui_gateway::NodeFromUiMessage;
use serde_derive::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};

pub const DEFAULT_MINIMUM_HOP_COUNT: usize = 3;

#[allow(clippy::upper_case_acronyms)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum ProxyProtocol {
    HTTP,
    TLS,
}

// TODO: Based on the way it's used, this struct should comprise two elements: one, a nested
// struct that contains all the small, quickly-cloned things, and the other the big,
// expensively-cloned SequencedPacket.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[allow(non_camel_case_types)]
pub struct ClientRequestPayload_0v1 {
    pub stream_key: StreamKey,
    pub sequenced_packet: SequencedPacket,
    pub target_hostname: String,
    pub target_port: u16,
    pub protocol: ProxyProtocol,
    pub originator_public_key: PublicKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_attempt_id_opt: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_session_request_opt: Option<ReceiptSessionRequest>,
}

impl Debug for ClientRequestPayload_0v1 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ClientRequestPayload_0v1 {{ protocol: {:?}, target_port: {}, sequence_number: {}, last_data: {}, data_len: {}, correlated_dns_attempt: {}, receipt_session_present: {}, request_data: [REDACTED] }}",
            self.protocol,
            self.target_port,
            self.sequenced_packet.sequence_number,
            self.sequenced_packet.last_data,
            self.sequenced_packet.data.len(),
            self.dns_attempt_id_opt.is_some(),
            self.receipt_session_request_opt.is_some()
        )
    }
}

impl From<ClientRequestPayload_0v1> for MessageType {
    fn from(payload: ClientRequestPayload_0v1) -> Self {
        MessageType::ClientRequest(VersionedData::new(
            &crate::sub_lib::migrations::client_request_payload::MIGRATIONS,
            &payload,
        ))
    }
}

impl ClientRequestPayload_0v1 {
    pub fn version() -> DataVersion {
        DataVersion::new(0, 0).expect("Internal Error")
    }
}

#[derive(Message, PartialEq, Eq)]
pub struct AddRouteResultMessage {
    pub stream_key: StreamKey,
    pub route_request_id: u64,
    pub result: Result<RouteQueryResponse, String>,
}

impl Debug for AddRouteResultMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "AddRouteResultMessage {{ route_request_id: {}, result_category: {}, route_data: [REDACTED] }}",
            self.route_request_id,
            if self.result.is_ok() { "success" } else { "error" }
        )
    }
}

#[derive(Message, PartialEq, Eq)]
pub struct StreamKeyPurge {
    pub stream_key: StreamKey,
}

impl Debug for StreamKeyPurge {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("StreamKeyPurge { stream_key: [REDACTED] }")
    }
}

#[derive(Clone, Debug, Message, PartialEq, Eq)]
pub struct RetryReceiptAcknowledgements {
    pub schedule_after_delay: bool,
}

#[derive(Clone, Message, PartialEq, Eq)]
pub struct RecordExitRequestForReceipt {
    pub stream_key: StreamKey,
    pub payload_size: u64,
    pub routing_payload_size: u64,
}

impl Debug for RecordExitRequestForReceipt {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RecordExitRequestForReceipt {{ payload_size: {}, routing_payload_size: {}, stream_key: [REDACTED] }}",
            self.payload_size, self.routing_payload_size
        )
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProxyServerSubs {
    // ProxyServer will handle these messages:
    pub bind: Recipient<BindMessage>,
    pub from_dispatcher: Recipient<InboundClientData>,
    pub from_hopper: Recipient<ExpiredCoresPackage<ClientResponsePayload_0v1>>,
    pub dns_failure_from_hopper: Recipient<ExpiredCoresPackage<DnsResolveFailure_0v1>>,
    pub receipt_offer_from_hopper: Recipient<ExpiredCoresPackage<ServiceReceiptOfferPayload_0v1>>,
    pub metered_response_from_hopper:
        Recipient<ExpiredCoresPackage<MeteredClientResponsePayload_0v1>>,
    pub stream_shutdown_sub: Recipient<StreamShutdownMsg>,
    pub node_from_ui: Recipient<NodeFromUiMessage>,
    pub route_result_sub: Recipient<AddRouteResultMessage>,
    pub schedule_stream_key_purge: Recipient<MessageScheduler<StreamKeyPurge>>,
    pub retry_receipt_acknowledgements: Recipient<RetryReceiptAcknowledgements>,
    pub record_exit_request_for_receipt: Recipient<RecordExitRequestForReceipt>,
}

impl Debug for ProxyServerSubs {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(f, "ProxyServerSubs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub_lib::proxy_server::ProxyServerSubs;
    use crate::test_utils::recorder::Recorder;
    use actix::Actor;

    #[test]
    fn request_and_route_control_debug_redacts_destination_stream_and_payload() {
        let stream_key = StreamKey::make_meaningful_stream_key("private request stream");
        let request = ClientRequestPayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket::new(b"private request marker".to_vec(), 4, false),
            target_hostname: "private.example".to_string(),
            target_port: 443,
            protocol: ProxyProtocol::TLS,
            originator_public_key: PublicKey::new(b"private originator marker"),
            dns_attempt_id_opt: Some(31),
            receipt_session_request_opt: None,
        };
        let route_result = AddRouteResultMessage {
            stream_key,
            route_request_id: 17,
            result: Err("private route error marker".to_string()),
        };
        let purge = StreamKeyPurge { stream_key };
        let receipt_record = RecordExitRequestForReceipt {
            stream_key,
            payload_size: 22,
            routing_payload_size: 81,
        };

        assert_eq!(
            format!("{:?}", request),
            "ClientRequestPayload_0v1 { protocol: TLS, target_port: 443, sequence_number: 4, last_data: false, data_len: 22, correlated_dns_attempt: true, receipt_session_present: false, request_data: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", route_result),
            "AddRouteResultMessage { route_request_id: 17, result_category: error, route_data: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", purge),
            "StreamKeyPurge { stream_key: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", receipt_record),
            "RecordExitRequestForReceipt { payload_size: 22, routing_payload_size: 81, stream_key: [REDACTED] }"
        );
    }

    #[test]
    fn proxy_server_subs_debug() {
        let recorder = Recorder::new().start();

        let subject = ProxyServerSubs {
            bind: recipient!(recorder, BindMessage),
            from_dispatcher: recipient!(recorder, InboundClientData),
            from_hopper: recipient!(recorder, ExpiredCoresPackage<ClientResponsePayload_0v1>),
            dns_failure_from_hopper: recipient!(
                recorder,
                ExpiredCoresPackage<DnsResolveFailure_0v1>
            ),
            receipt_offer_from_hopper: recipient!(
                recorder,
                ExpiredCoresPackage<ServiceReceiptOfferPayload_0v1>
            ),
            metered_response_from_hopper: recipient!(
                recorder,
                ExpiredCoresPackage<MeteredClientResponsePayload_0v1>
            ),
            stream_shutdown_sub: recipient!(recorder, StreamShutdownMsg),
            node_from_ui: recipient!(recorder, NodeFromUiMessage),
            route_result_sub: recipient!(recorder, AddRouteResultMessage),
            schedule_stream_key_purge: recipient!(recorder, MessageScheduler<StreamKeyPurge>),
            retry_receipt_acknowledgements: recipient!(recorder, RetryReceiptAcknowledgements),
            record_exit_request_for_receipt: recipient!(recorder, RecordExitRequestForReceipt),
        };

        assert_eq!(format!("{:?}", subject), "ProxyServerSubs");
    }
}
