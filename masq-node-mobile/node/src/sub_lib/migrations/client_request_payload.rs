// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::sub_lib::cryptde::PublicKey;
use crate::sub_lib::migrations::utils::value_to_type;
use crate::sub_lib::proxy_server::{ClientRequestPayload_0v1, ProxyProtocol};
use crate::sub_lib::sequence_buffer::SequencedPacket;
use crate::sub_lib::service_receipt::ReceiptSessionRequest;
use crate::sub_lib::stream_key::StreamKey;
use crate::sub_lib::versioned_data::Migrations;
use crate::sub_lib::versioned_data::{MigrationError, StepError, VersionedData};
use lazy_static::lazy_static;
use serde_cbor::Value;
use std::convert::TryFrom;

lazy_static! {
    pub static ref MIGRATIONS: Migrations = {
        let current_version = masq_lib::constants::CLIENT_REQUEST_PAYLOAD_CURRENT_VERSION;
        let mut migrations = Migrations::new(current_version);

        migrate_value!(dv!(0, 1), ClientRequestPayload_0v1, ClientRequestPayloadMF_0v1, {|value: serde_cbor::Value| {
            ClientRequestPayload_0v1::try_from (&value)
        }});
        migrations.add_step (masq_lib::data_version::FUTURE_VERSION, dv!(0, 1), Box::new (ClientRequestPayloadMF_0v1{}));

        // add more steps here

        migrations
    };
}

impl From<ClientRequestPayload_0v1> for VersionedData<ClientRequestPayload_0v1> {
    fn from(data: ClientRequestPayload_0v1) -> Self {
        VersionedData::new(&MIGRATIONS, &data)
    }
}

impl TryFrom<VersionedData<ClientRequestPayload_0v1>> for ClientRequestPayload_0v1 {
    type Error = MigrationError;

    fn try_from(vd: VersionedData<ClientRequestPayload_0v1>) -> Result<Self, Self::Error> {
        vd.extract(&MIGRATIONS)
    }
}

impl TryFrom<&Value> for ClientRequestPayload_0v1 {
    type Error = StepError;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::Map(map) => {
                let mut stream_key_opt: Option<StreamKey> = None;
                let mut sequenced_packet_opt: Option<SequencedPacket> = None;
                let mut target_hostname: Option<String> = None;
                let mut target_port_opt: Option<u16> = None;
                let mut protocol_opt: Option<ProxyProtocol> = None;
                let mut originator_public_key_opt: Option<PublicKey> = None;
                let mut dns_attempt_id_opt: Option<u64> = None;
                let mut receipt_session_request_opt: Option<ReceiptSessionRequest> = None;
                map.keys().for_each(|k| {
                    let v = map.get(k).expect("Disappeared");
                    if let (Value::Text(field_name), _) = (k, v) {
                        match field_name.as_str() {
                            "stream_key" => stream_key_opt = value_to_type::<StreamKey>(v),
                            "sequenced_packet" => {
                                sequenced_packet_opt = value_to_type::<SequencedPacket>(v)
                            }
                            "target_hostname" => target_hostname = value_to_type::<String>(v),
                            "target_port" => target_port_opt = value_to_type::<u16>(v),
                            "protocol" => protocol_opt = value_to_type::<ProxyProtocol>(v),
                            "originator_public_key" => {
                                originator_public_key_opt = value_to_type::<PublicKey>(v)
                            }
                            "dns_attempt_id_opt" => dns_attempt_id_opt = value_to_type::<u64>(v),
                            "receipt_session_request_opt" => {
                                receipt_session_request_opt =
                                    value_to_type::<ReceiptSessionRequest>(v)
                            }
                            _ => (),
                        }
                    }
                });
                let mut missing_fields: Vec<&str> = vec![];
                fn check_field<'a, T>(
                    missing_fields: &mut Vec<&'a str>,
                    name: &'a str,
                    field: &Option<T>,
                ) {
                    if field.is_none() {
                        missing_fields.push(name)
                    }
                }
                check_field(&mut missing_fields, "stream_key", &stream_key_opt);
                check_field(
                    &mut missing_fields,
                    "sequenced_packet",
                    &sequenced_packet_opt,
                );
                check_field(&mut missing_fields, "target_hostname", &target_hostname);
                check_field(&mut missing_fields, "target_port", &target_port_opt);
                check_field(&mut missing_fields, "protocol", &protocol_opt);
                check_field(
                    &mut missing_fields,
                    "originator_public_key",
                    &originator_public_key_opt,
                );
                if !missing_fields.is_empty() {
                    return Err(StepError::SemanticError(format!(
                        "Missing required fields: {:?}",
                        missing_fields
                    )));
                }
                Ok(ClientRequestPayload_0v1 {
                    stream_key: stream_key_opt.expect("stream_key disappeared"),
                    sequenced_packet: sequenced_packet_opt.expect("sequenced_packet disappeared"),
                    target_hostname: target_hostname.expect("target_hostname disappeared"),
                    target_port: target_port_opt.expect("target_port disappeared"),
                    protocol: protocol_opt.expect("protocol disappeared"),
                    originator_public_key: originator_public_key_opt
                        .expect("originator_public_key disappeared"),
                    dns_attempt_id_opt,
                    receipt_session_request_opt,
                })
            }
            _ => Err(StepError::SemanticError(format!(
                "Expected Value::Map; found {:?}",
                value
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub_lib::cryptde::PublicKey;
    use masq_lib::data_version::DataVersion;
    use serde_derive::{Deserialize, Serialize};

    #[test]
    fn client_request_payload_missing_fields_are_rejected_without_panicking() {
        let result = ClientRequestPayload_0v1::try_from(&Value::Map(Default::default()));

        assert!(matches!(result, Err(StepError::SemanticError(_))));
    }

    #[test]
    fn dns_attempt_id_survives_value_extraction() {
        let expected = ClientRequestPayload_0v1 {
            stream_key: StreamKey::make_meaningful_stream_key("dns attempt"),
            sequenced_packet: SequencedPacket::new(vec![1, 2, 3], 0, false),
            target_hostname: "example.com".to_string(),
            target_port: 443,
            protocol: ProxyProtocol::TLS,
            originator_public_key: PublicKey::new(&[4, 5, 6]),
            dns_attempt_id_opt: Some(37),
            receipt_session_request_opt: None,
        };
        let encoded = serde_cbor::ser::to_vec(&expected).unwrap();
        let value = serde_cbor::de::from_slice::<Value>(&encoded).unwrap();

        let actual = ClientRequestPayload_0v1::try_from(&value).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn can_migrate_from_the_future() {
        #[derive(Serialize, Deserialize)]
        struct ExampleFutureCRP {
            pub stream_key: StreamKey,
            pub sequenced_packet: SequencedPacket,
            pub target_hostname: String,
            pub target_port: u16,
            pub protocol: ProxyProtocol,
            pub originator_public_key: PublicKey,
            pub another_field: String,
            pub yet_another_field: u64,
        }
        let expected_crp = ClientRequestPayload_0v1 {
            stream_key: StreamKey::make_meaningful_stream_key("All Things Must Pass"),
            sequenced_packet: SequencedPacket::new(vec![4, 3, 2, 1], 4321, false),
            target_hostname: "target.hostname.com".to_string(),
            target_port: 1234,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: PublicKey::new(&[2, 3, 4, 5]),
            dns_attempt_id_opt: None,
            receipt_session_request_opt: None,
        };
        let future_crp = ExampleFutureCRP {
            stream_key: expected_crp.stream_key.clone(),
            sequenced_packet: expected_crp.sequenced_packet.clone(),
            target_hostname: expected_crp.target_hostname.clone(),
            target_port: expected_crp.target_port.clone(),
            protocol: expected_crp.protocol.clone(),
            originator_public_key: expected_crp.originator_public_key.clone(),
            another_field: "These are the times that try men's souls".to_string(),
            yet_another_field: 1234567890,
        };
        let future_migrations = Migrations::new(DataVersion::new(4095, 4095));
        let serialized =
            serde_cbor::ser::to_vec(&VersionedData::new(&future_migrations, &future_crp)).unwrap();
        let future_vd =
            serde_cbor::de::from_slice::<VersionedData<ClientRequestPayload_0v1>>(&serialized)
                .unwrap();

        let actual_crp = ClientRequestPayload_0v1::try_from(future_vd).unwrap();

        assert_eq!(actual_crp, expected_crp);
    }

    #[test]
    fn cannot_migrate_from_value_other_than_map() {
        let value = Value::Bool(true);

        let result = ClientRequestPayload_0v1::try_from(&value);

        assert_eq!(
            result,
            Err(StepError::SemanticError(
                "Expected Value::Map; found Bool(true)".to_string()
            ))
        )
    }
}
