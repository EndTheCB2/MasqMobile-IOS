// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::sub_lib::hopper::MessageType;
use crate::sub_lib::proxy_client::MeteredClientResponsePayload_0v1;
use crate::sub_lib::versioned_data::{MigrationError, Migrations, VersionedData};
use lazy_static::lazy_static;
use std::convert::TryFrom;

lazy_static! {
    /// Metering changes financial meaning, so an unknown wrapper version must fail closed.
    pub static ref MIGRATIONS: Migrations = Migrations::new(
        masq_lib::constants::METERED_CLIENT_RESPONSE_PAYLOAD_CURRENT_VERSION
    );
}

impl From<MeteredClientResponsePayload_0v1> for VersionedData<MeteredClientResponsePayload_0v1> {
    fn from(data: MeteredClientResponsePayload_0v1) -> Self {
        VersionedData::new(&MIGRATIONS, &data)
    }
}

impl From<MeteredClientResponsePayload_0v1> for MessageType {
    fn from(data: MeteredClientResponsePayload_0v1) -> Self {
        MessageType::MeteredClientResponse(data.into())
    }
}

impl TryFrom<VersionedData<MeteredClientResponsePayload_0v1>> for MeteredClientResponsePayload_0v1 {
    type Error = MigrationError;

    fn try_from(vd: VersionedData<MeteredClientResponsePayload_0v1>) -> Result<Self, Self::Error> {
        vd.extract(&MIGRATIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub_lib::cryptde::{CryptDE, PublicKey};
    use crate::sub_lib::cryptde_null::CryptDENull;
    use crate::sub_lib::proxy_client::ClientResponsePayload_0v1;
    use crate::sub_lib::sequence_buffer::SequencedPacket;
    use crate::sub_lib::service_receipt::{
        ServiceKind, ServiceReceipt, ServiceReceiptOfferPayload_0v1,
    };
    use crate::sub_lib::stream_key::StreamKey;
    use masq_lib::data_version::DataVersion;
    use masq_lib::test_utils::utils::TEST_DEFAULT_CHAIN;

    fn payload() -> MeteredClientResponsePayload_0v1 {
        let provider = CryptDENull::from(
            &PublicKey::new(b"metered response provider"),
            TEST_DEFAULT_CHAIN,
        );
        MeteredClientResponsePayload_0v1 {
            response: ClientResponsePayload_0v1 {
                stream_key: StreamKey::make_meaningful_stream_key("metered response"),
                sequenced_packet: SequencedPacket::new(vec![1, 2, 3], 4, false),
            },
            receipt_offer: ServiceReceiptOfferPayload_0v1 {
                signed_receipt: ServiceReceipt::new(
                    [0x21; 32],
                    4,
                    ServiceKind::Exit,
                    provider.public_key().clone(),
                    [0x22; 32],
                    3,
                    7,
                    2,
                )
                .sign(&provider)
                .unwrap(),
            },
        }
    }

    #[test]
    fn metered_response_round_trips_and_unknown_versions_fail_closed() {
        let original = payload();
        let versioned = VersionedData::from(original.clone());
        assert_eq!(versioned.version(), DataVersion::new(0, 1));
        assert_eq!(
            MeteredClientResponsePayload_0v1::try_from(versioned),
            Ok(original.clone())
        );
        let serialized = serde_cbor::to_vec(&original).unwrap();
        assert!(
            MeteredClientResponsePayload_0v1::try_from(VersionedData::test_new(
                DataVersion::new(0, 0),
                serialized.clone(),
            ))
            .is_err()
        );
        assert!(
            MeteredClientResponsePayload_0v1::try_from(VersionedData::test_new(
                DataVersion::new(0, 2),
                serialized,
            ))
            .is_err()
        );
    }
}
