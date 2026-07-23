// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::sub_lib::hopper::MessageType;
use crate::sub_lib::service_receipt::ServiceReceiptPayload_0v1;
use crate::sub_lib::versioned_data::{MigrationError, Migrations, VersionedData};
use lazy_static::lazy_static;
use std::convert::TryFrom;

lazy_static! {
    /// Receipt settlement has no predecessor. Unknown older or future payloads are rejected
    /// rather than guessed into a signed financial claim.
    pub static ref MIGRATIONS: Migrations =
        Migrations::new(masq_lib::constants::SERVICE_RECEIPT_PAYLOAD_CURRENT_VERSION);
}

impl From<ServiceReceiptPayload_0v1> for VersionedData<ServiceReceiptPayload_0v1> {
    fn from(data: ServiceReceiptPayload_0v1) -> Self {
        VersionedData::new(&MIGRATIONS, &data)
    }
}

impl From<ServiceReceiptPayload_0v1> for MessageType {
    fn from(data: ServiceReceiptPayload_0v1) -> Self {
        MessageType::ServiceReceipt(data.into())
    }
}

impl TryFrom<VersionedData<ServiceReceiptPayload_0v1>> for ServiceReceiptPayload_0v1 {
    type Error = MigrationError;

    fn try_from(vd: VersionedData<ServiceReceiptPayload_0v1>) -> Result<Self, Self::Error> {
        vd.extract(&MIGRATIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub_lib::service_receipt::{
        make_accounting_commitment, ReceiptSessionPolicy, ServiceKind, ServiceReceipt,
    };
    use crate::sub_lib::{cryptde::PublicKey, cryptde_null::CryptDENull};
    use crate::test_utils::make_paying_wallet;
    use masq_lib::data_version::DataVersion;
    use masq_lib::test_utils::utils::TEST_DEFAULT_CHAIN;

    fn payload() -> ServiceReceiptPayload_0v1 {
        let provider_key = PublicKey::new(b"receipt provider");
        let provider = CryptDENull::from(&provider_key, TEST_DEFAULT_CHAIN);
        let payer_key = PublicKey::new(b"receipt payer session");
        let payer = CryptDENull::from(&payer_key, TEST_DEFAULT_CHAIN);
        let route_epoch = [0x71; 32];
        let receipt = ServiceReceipt::new(
            route_epoch,
            1,
            ServiceKind::Routing,
            provider_key,
            make_accounting_commitment(&route_epoch, &payer_key),
            100,
            7,
            2,
        )
        .sign(&provider)
        .unwrap()
        .acknowledge(&payer)
        .unwrap();
        let wallet = make_paying_wallet(b"receipt payload wallet");
        let authorization = ReceiptSessionPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
            wallet.address(),
            payer_key,
            10_000,
            1,
            86_401,
            [0x72; 32],
        )
        .authorize(&wallet)
        .unwrap();
        ServiceReceiptPayload_0v1 {
            authorization,
            acknowledged_receipt: receipt,
        }
    }

    #[test]
    fn current_payload_round_trips() {
        let original = payload();
        let versioned = VersionedData::from(original.clone());

        assert_eq!(versioned.version(), DataVersion::new(0, 1));
        assert_eq!(ServiceReceiptPayload_0v1::try_from(versioned), Ok(original));
    }

    #[test]
    fn unknown_wire_versions_fail_closed() {
        let serialized = serde_cbor::to_vec(&payload()).unwrap();

        let old = VersionedData::test_new(DataVersion::new(0, 0), serialized.clone());
        let future = VersionedData::test_new(DataVersion::new(0, 2), serialized);

        assert!(ServiceReceiptPayload_0v1::try_from(old).is_err());
        assert!(ServiceReceiptPayload_0v1::try_from(future).is_err());
    }
}
