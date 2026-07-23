// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::bootstrapper::CryptDEPair;
use crate::sub_lib::hopper::{ExpiredCoresPackage, MessageType};
use crate::sub_lib::peer_actors::BindMessage;
use crate::sub_lib::proxy_server::ClientRequestPayload_0v1;
use crate::sub_lib::sequence_buffer::SequencedPacket;
use crate::sub_lib::service_receipt::ServiceReceiptOfferPayload_0v1;
use crate::sub_lib::stream_key::StreamKey;
use crate::sub_lib::versioned_data::VersionedData;
use actix::Message;
use actix::Recipient;
use ethereum_types::Address;
use masq_lib::ui_gateway::NodeFromUiMessage;
use serde_derive::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;

pub fn error_socket_addr() -> SocketAddr {
    SocketAddr::from(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 0))
}

#[derive(Clone)]
pub struct ProxyClientConfig {
    pub cryptde_pair: CryptDEPair,
    pub dns_servers: Vec<SocketAddr>,
    pub exit_service_rate: u64,
    pub exit_byte_rate: u64,
    pub is_decentralized: bool,
    pub crashable: bool,
    pub receipt_session_validation_opt: Option<ReceiptSessionValidation>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReceiptSessionValidation {
    pub chain_id: u64,
    pub settlement_contract: Address,
}

impl Debug for ReceiptSessionValidation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceiptSessionValidation { validation_data: [REDACTED] }")
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[allow(non_camel_case_types)]
pub struct ClientResponsePayload_0v1 {
    pub stream_key: StreamKey,
    pub sequenced_packet: SequencedPacket,
}

impl Debug for ClientResponsePayload_0v1 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ClientResponsePayload_0v1 {{ sequence_number: {}, last_data: {}, data_len: {}, response_data: [REDACTED] }}",
            self.sequenced_packet.sequence_number,
            self.sequenced_packet.last_data,
            self.sequenced_packet.data.len()
        )
    }
}

/// A normal response and its proof of measured exit service share one CORES package. This avoids
/// creating separately routed, unaccounted control traffic for the provider's initial offer.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[allow(non_camel_case_types)]
pub struct MeteredClientResponsePayload_0v1 {
    pub response: ClientResponsePayload_0v1,
    pub receipt_offer: ServiceReceiptOfferPayload_0v1,
}

impl Debug for MeteredClientResponsePayload_0v1 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MeteredClientResponsePayload_0v1 {{ sequence_number: {}, last_data: {}, data_len: {}, response_and_receipt: [REDACTED] }}",
            self.response.sequenced_packet.sequence_number,
            self.response.sequenced_packet.last_data,
            self.response.sequenced_packet.data.len()
        )
    }
}

#[derive(Message, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[allow(non_camel_case_types)]
pub struct DnsResolveFailure_0v1 {
    pub stream_key: StreamKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_attempt_id_opt: Option<u64>,
}

impl Debug for DnsResolveFailure_0v1 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DnsResolveFailure_0v1 {{ correlated_attempt: {}, failure_data: [REDACTED] }}",
            self.dns_attempt_id_opt.is_some()
        )
    }
}

impl DnsResolveFailure_0v1 {
    pub fn new(stream_key: StreamKey) -> Self {
        Self {
            stream_key,
            dns_attempt_id_opt: None,
        }
    }

    pub fn for_attempt(stream_key: StreamKey, dns_attempt_id_opt: Option<u64>) -> Self {
        Self {
            stream_key,
            dns_attempt_id_opt,
        }
    }
}

impl From<ClientResponsePayload_0v1> for MessageType {
    fn from(data: ClientResponsePayload_0v1) -> Self {
        MessageType::ClientResponse(VersionedData::new(
            &crate::sub_lib::migrations::client_response_payload::MIGRATIONS,
            &data,
        ))
    }
}

impl From<DnsResolveFailure_0v1> for MessageType {
    fn from(data: DnsResolveFailure_0v1) -> Self {
        MessageType::DnsResolveFailed(VersionedData::new(
            &crate::sub_lib::migrations::dns_resolve_failure::MIGRATIONS,
            &data,
        ))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProxyClientSubs {
    pub bind: Recipient<BindMessage>,
    pub from_hopper: Recipient<ExpiredCoresPackage<ClientRequestPayload_0v1>>,
    pub inbound_server_data: Recipient<InboundServerData>,
    pub dns_resolve_failed: Recipient<DnsResolveFailure_0v1>,
    pub node_from_ui: Recipient<NodeFromUiMessage>,
}

impl Debug for ProxyClientSubs {
    fn fmt(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        write!(f, "ProxyClientSubs")
    }
}

impl ClientResponsePayload_0v1 {
    pub fn make_terminating_payload(stream_key: StreamKey) -> ClientResponsePayload_0v1 {
        ClientResponsePayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket {
                data: vec![],
                sequence_number: 0,
                last_data: true,
            },
        }
    }
}

#[derive(PartialEq, Eq, Clone, Message)]
pub struct InboundServerData {
    pub stream_key: StreamKey,
    pub last_data: bool,
    pub sequence_number: u64,
    pub source: SocketAddr,
    pub data: Vec<u8>,
}

impl Debug for InboundServerData {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "InboundServerData {{ sequence_number: {}, last_data: {}, data_len: {}, server_data: [REDACTED] }}",
            self.sequence_number,
            self.last_data,
            self.data.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub_lib::cryptde::{CryptDE, PublicKey};
    use crate::sub_lib::cryptde_null::CryptDENull;
    use crate::sub_lib::peer_actors::BindMessage;
    use crate::sub_lib::service_receipt::{ServiceKind, ServiceReceipt};
    use crate::test_utils::recorder::Recorder;
    use actix::Actor;
    use masq_lib::test_utils::utils::TEST_DEFAULT_CHAIN;

    #[test]
    fn make_terminating_payload_makes_terminating_payload() {
        let stream_key: StreamKey = StreamKey::make_meaningless_stream_key();

        let payload = ClientResponsePayload_0v1::make_terminating_payload(stream_key);

        assert_eq!(
            payload,
            ClientResponsePayload_0v1 {
                stream_key,
                sequenced_packet: SequencedPacket {
                    data: vec!(),
                    sequence_number: 0,
                    last_data: true
                },
            }
        )
    }

    #[test]
    fn transport_debug_redacts_response_stream_server_and_settlement_identity() {
        let stream_key = StreamKey::make_meaningful_stream_key("private stream marker");
        let sensitive_data = b"private response marker".to_vec();
        let response = ClientResponsePayload_0v1 {
            stream_key,
            sequenced_packet: SequencedPacket::new(sensitive_data.clone(), 7, false),
        };
        let provider = CryptDENull::from(
            &PublicKey::new(b"private provider marker"),
            TEST_DEFAULT_CHAIN,
        );
        let metered = MeteredClientResponsePayload_0v1 {
            response: response.clone(),
            receipt_offer: ServiceReceiptOfferPayload_0v1 {
                signed_receipt: ServiceReceipt::new(
                    [0x21; 32],
                    7,
                    ServiceKind::Exit,
                    provider.public_key().clone(),
                    [0x22; 32],
                    sensitive_data.len() as u64,
                    9,
                    11,
                )
                .sign(&provider)
                .unwrap(),
            },
        };
        let validation = ReceiptSessionValidation {
            chain_id: 8_452,
            settlement_contract: Address::from([0x71; 20]),
        };
        let failure = DnsResolveFailure_0v1::for_attempt(stream_key, Some(91));
        let inbound = InboundServerData {
            stream_key,
            last_data: false,
            sequence_number: 9,
            source: "203.0.113.10:54321".parse().unwrap(),
            data: sensitive_data,
        };

        assert_eq!(
            format!("{:?}", validation),
            "ReceiptSessionValidation { validation_data: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", response),
            "ClientResponsePayload_0v1 { sequence_number: 7, last_data: false, data_len: 23, response_data: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", metered),
            "MeteredClientResponsePayload_0v1 { sequence_number: 7, last_data: false, data_len: 23, response_and_receipt: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", failure),
            "DnsResolveFailure_0v1 { correlated_attempt: true, failure_data: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", inbound),
            "InboundServerData { sequence_number: 9, last_data: false, data_len: 23, server_data: [REDACTED] }"
        );
    }

    #[test]
    fn proxy_client_subs_debug() {
        let recorder = Recorder::new().start();

        let subject = ProxyClientSubs {
            bind: recipient!(recorder, BindMessage),
            from_hopper: recipient!(recorder, ExpiredCoresPackage<ClientRequestPayload_0v1>),
            inbound_server_data: recipient!(recorder, InboundServerData),
            dns_resolve_failed: recipient!(recorder, DnsResolveFailure_0v1),
            node_from_ui: recipient!(recorder, NodeFromUiMessage),
        };

        assert_eq!(format!("{:?}", subject), "ProxyClientSubs");
    }
}
