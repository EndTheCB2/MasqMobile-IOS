// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::sub_lib::hopper::MessageType;
use crate::sub_lib::service_receipt::ServiceReceiptOfferPayload_0v1;
use crate::sub_lib::versioned_data::{MigrationError, Migrations, VersionedData};
use lazy_static::lazy_static;
use std::convert::TryFrom;

lazy_static! {
    /// A signed financial offer has no safe guessed migration. Unknown versions fail closed.
    pub static ref MIGRATIONS: Migrations =
        Migrations::new(masq_lib::constants::SERVICE_RECEIPT_OFFER_PAYLOAD_CURRENT_VERSION);
}

impl From<ServiceReceiptOfferPayload_0v1> for VersionedData<ServiceReceiptOfferPayload_0v1> {
    fn from(data: ServiceReceiptOfferPayload_0v1) -> Self {
        VersionedData::new(&MIGRATIONS, &data)
    }
}

impl From<ServiceReceiptOfferPayload_0v1> for MessageType {
    fn from(data: ServiceReceiptOfferPayload_0v1) -> Self {
        MessageType::ServiceReceiptOffer(data.into())
    }
}

impl TryFrom<VersionedData<ServiceReceiptOfferPayload_0v1>> for ServiceReceiptOfferPayload_0v1 {
    type Error = MigrationError;

    fn try_from(vd: VersionedData<ServiceReceiptOfferPayload_0v1>) -> Result<Self, Self::Error> {
        vd.extract(&MIGRATIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub_lib::cryptde::{CryptDE, PublicKey};
    use crate::sub_lib::cryptde_null::CryptDENull;
    use crate::sub_lib::service_receipt::{ServiceKind, ServiceReceipt};
    use masq_lib::data_version::DataVersion;
    use masq_lib::test_utils::utils::TEST_DEFAULT_CHAIN;

    fn payload() -> ServiceReceiptOfferPayload_0v1 {
        let provider = CryptDENull::from(
            &PublicKey::new(b"service receipt offer provider"),
            TEST_DEFAULT_CHAIN,
        );
        ServiceReceiptOfferPayload_0v1 {
            signed_receipt: ServiceReceipt::new(
                [0x31; 32],
                1,
                ServiceKind::Exit,
                provider.public_key().clone(),
                [0x32; 32],
                100,
                7,
                2,
            )
            .sign(&provider)
            .unwrap(),
        }
    }

    #[test]
    fn current_offer_round_trips_and_unknown_versions_fail_closed() {
        let original = payload();
        let versioned = VersionedData::from(original.clone());
        assert_eq!(versioned.version(), DataVersion::new(0, 1));
        assert_eq!(
            ServiceReceiptOfferPayload_0v1::try_from(versioned),
            Ok(original.clone())
        );

        let serialized = serde_cbor::to_vec(&original).unwrap();
        assert!(
            ServiceReceiptOfferPayload_0v1::try_from(VersionedData::test_new(
                DataVersion::new(0, 0),
                serialized.clone(),
            ))
            .is_err()
        );
        assert!(
            ServiceReceiptOfferPayload_0v1::try_from(VersionedData::test_new(
                DataVersion::new(0, 2),
                serialized,
            ))
            .is_err()
        );
    }
}
