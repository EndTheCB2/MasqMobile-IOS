// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::accountant::db_access_objects::utils::DaoFactoryReal;
use crate::database::rusqlite_wrappers::ConnectionWrapper;
use crate::sub_lib::service_receipt::ServiceReceiptPayload_0v1;
use rusqlite::{params, OptionalExtension};
use std::fmt::{Debug, Formatter};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Eq, PartialEq)]
pub enum ReceiptAcknowledgementOutboxDaoError {
    Database(String),
    Deserialization(String),
    IdentityMismatch,
    Serialization(String),
    Time(String),
}

impl Debug for ReceiptAcknowledgementOutboxDaoError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(_) => f.write_str("Database([REDACTED])"),
            Self::Deserialization(_) => f.write_str("Deserialization([REDACTED])"),
            Self::IdentityMismatch => f.write_str("IdentityMismatch"),
            Self::Serialization(_) => f.write_str("Serialization([REDACTED])"),
            Self::Time(_) => f.write_str("Time([REDACTED])"),
        }
    }
}

pub trait ReceiptAcknowledgementOutboxDao: Send {
    fn enqueue(
        &mut self,
        payload: &ServiceReceiptPayload_0v1,
        created_at: SystemTime,
    ) -> Result<(), ReceiptAcknowledgementOutboxDaoError>;

    fn pending(
        &self,
    ) -> Result<Vec<ServiceReceiptPayload_0v1>, ReceiptAcknowledgementOutboxDaoError>;

    fn delete(
        &mut self,
        payload: &ServiceReceiptPayload_0v1,
    ) -> Result<(), ReceiptAcknowledgementOutboxDaoError>;
}

pub trait ReceiptAcknowledgementOutboxDaoFactory {
    fn make(&self) -> Box<dyn ReceiptAcknowledgementOutboxDao>;
}

impl ReceiptAcknowledgementOutboxDaoFactory for DaoFactoryReal {
    fn make(&self) -> Box<dyn ReceiptAcknowledgementOutboxDao> {
        Box::new(ReceiptAcknowledgementOutboxDaoReal::new(
            self.make_connection(),
        ))
    }
}

pub struct ReceiptAcknowledgementOutboxDaoReal {
    conn: Box<dyn ConnectionWrapper>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CumulativeEnqueueDisposition {
    Idempotent,
    Replace,
    Stale,
}

impl ReceiptAcknowledgementOutboxDaoReal {
    pub fn new(conn: Box<dyn ConnectionWrapper>) -> Self {
        Self { conn }
    }

    fn database_error(error: rusqlite::Error) -> ReceiptAcknowledgementOutboxDaoError {
        ReceiptAcknowledgementOutboxDaoError::Database(error.to_string())
    }

    fn identity(payload: &ServiceReceiptPayload_0v1) -> (&[u8; 32], &[u8]) {
        let receipt = &payload.acknowledged_receipt.signed_receipt.receipt;
        (&receipt.route_epoch, receipt.provider_public_key.as_slice())
    }

    fn serialize(
        payload: &ServiceReceiptPayload_0v1,
    ) -> Result<Vec<u8>, ReceiptAcknowledgementOutboxDaoError> {
        serde_cbor::to_vec(payload)
            .map_err(|error| ReceiptAcknowledgementOutboxDaoError::Serialization(error.to_string()))
    }

    fn deserialize(
        route_epoch: &[u8],
        provider_public_key: &[u8],
        serialized: &[u8],
    ) -> Result<ServiceReceiptPayload_0v1, ReceiptAcknowledgementOutboxDaoError> {
        let payload: ServiceReceiptPayload_0v1 =
            serde_cbor::from_slice(serialized).map_err(|error| {
                ReceiptAcknowledgementOutboxDaoError::Deserialization(error.to_string())
            })?;
        let (payload_epoch, payload_provider) = Self::identity(&payload);
        if payload_epoch.as_slice() != route_epoch || payload_provider != provider_public_key {
            return Err(ReceiptAcknowledgementOutboxDaoError::IdentityMismatch);
        }
        Ok(payload)
    }

    fn cumulative_enqueue_disposition(
        existing: &ServiceReceiptPayload_0v1,
        new: &ServiceReceiptPayload_0v1,
    ) -> Result<CumulativeEnqueueDisposition, ReceiptAcknowledgementOutboxDaoError> {
        if existing == new {
            return Ok(CumulativeEnqueueDisposition::Idempotent);
        }
        let existing_acknowledgement = &existing.acknowledged_receipt;
        let new_acknowledgement = &new.acknowledged_receipt;
        let existing_receipt = &existing_acknowledgement.signed_receipt.receipt;
        let new_receipt = &new_acknowledgement.signed_receipt.receipt;
        if existing.authorization != new.authorization
            || existing_acknowledgement.payer_session_public_key
                != new_acknowledgement.payer_session_public_key
            || existing_receipt.route_epoch != new_receipt.route_epoch
            || existing_receipt.provider_public_key != new_receipt.provider_public_key
            || existing_receipt.accounting_commitment != new_receipt.accounting_commitment
        {
            return Err(ReceiptAcknowledgementOutboxDaoError::IdentityMismatch);
        }
        if new_receipt.sequence < existing_receipt.sequence
            && new_receipt.cumulative_charge_wei < existing_receipt.cumulative_charge_wei
        {
            return Ok(CumulativeEnqueueDisposition::Stale);
        }
        if new_receipt.sequence > existing_receipt.sequence
            && new_receipt.cumulative_charge_wei > existing_receipt.cumulative_charge_wei
        {
            return Ok(CumulativeEnqueueDisposition::Replace);
        }
        Err(ReceiptAcknowledgementOutboxDaoError::IdentityMismatch)
    }
}

impl ReceiptAcknowledgementOutboxDao for ReceiptAcknowledgementOutboxDaoReal {
    fn enqueue(
        &mut self,
        payload: &ServiceReceiptPayload_0v1,
        created_at: SystemTime,
    ) -> Result<(), ReceiptAcknowledgementOutboxDaoError> {
        let serialized = Self::serialize(payload)?;
        let created_at_unix_s = created_at
            .duration_since(UNIX_EPOCH)
            .map_err(|error| ReceiptAcknowledgementOutboxDaoError::Time(error.to_string()))?
            .as_secs()
            .to_string();
        let (route_epoch, provider_public_key) = Self::identity(payload);
        let inserted = self
            .conn
            .prepare(
                "insert into receipt_acknowledgement_outbox
                 (route_epoch, provider_public_key, acknowledgement_cbor, created_at_unix_s)
                 values (?1, ?2, ?3, ?4) on conflict(route_epoch) do nothing",
            )
            .map_err(Self::database_error)?
            .execute(params![
                route_epoch.as_slice(),
                provider_public_key,
                serialized,
                created_at_unix_s
            ])
            .map_err(Self::database_error)?;
        if inserted == 1 {
            return Ok(());
        }
        let existing_opt: Option<(Vec<u8>, Vec<u8>)> = self
            .conn
            .prepare(
                "select provider_public_key, acknowledgement_cbor
                 from receipt_acknowledgement_outbox where route_epoch = ?1",
            )
            .map_err(Self::database_error)?
            .query_row(params![route_epoch.as_slice()], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(Self::database_error)?;
        let (existing_provider, existing_serialized) =
            existing_opt.ok_or(ReceiptAcknowledgementOutboxDaoError::IdentityMismatch)?;
        if existing_provider != provider_public_key {
            return Err(ReceiptAcknowledgementOutboxDaoError::IdentityMismatch);
        }
        let existing = Self::deserialize(
            route_epoch.as_slice(),
            &existing_provider,
            &existing_serialized,
        )?;
        match Self::cumulative_enqueue_disposition(&existing, payload)? {
            CumulativeEnqueueDisposition::Idempotent | CumulativeEnqueueDisposition::Stale => {
                return Ok(())
            }
            CumulativeEnqueueDisposition::Replace => (),
        }
        let updated = self
            .conn
            .prepare(
                "update receipt_acknowledgement_outbox
                 set acknowledgement_cbor = ?1, created_at_unix_s = ?2
                 where route_epoch = ?3 and provider_public_key = ?4
                   and acknowledgement_cbor = ?5",
            )
            .map_err(Self::database_error)?
            .execute(params![
                &serialized,
                created_at_unix_s,
                route_epoch.as_slice(),
                provider_public_key,
                &existing_serialized,
            ])
            .map_err(Self::database_error)?;
        if updated == 1 {
            Ok(())
        } else {
            Err(ReceiptAcknowledgementOutboxDaoError::Database(
                "acknowledgement changed during cumulative replacement".to_string(),
            ))
        }
    }

    fn pending(
        &self,
    ) -> Result<Vec<ServiceReceiptPayload_0v1>, ReceiptAcknowledgementOutboxDaoError> {
        let mut statement = self
            .conn
            .prepare(
                "select route_epoch, provider_public_key, acknowledgement_cbor
                 from receipt_acknowledgement_outbox
                 order by cast(created_at_unix_s as integer), route_epoch",
            )
            .map_err(Self::database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(Self::database_error)?;
        rows.map(|row| {
            let (route_epoch, provider_public_key, serialized) =
                row.map_err(Self::database_error)?;
            Self::deserialize(&route_epoch, &provider_public_key, &serialized)
        })
        .collect()
    }

    fn delete(
        &mut self,
        payload: &ServiceReceiptPayload_0v1,
    ) -> Result<(), ReceiptAcknowledgementOutboxDaoError> {
        let serialized = Self::serialize(payload)?;
        let (route_epoch, provider_public_key) = Self::identity(payload);
        self.conn
            .prepare(
                "delete from receipt_acknowledgement_outbox
                 where route_epoch = ?1 and provider_public_key = ?2
                   and acknowledgement_cbor = ?3",
            )
            .map_err(Self::database_error)?
            .execute(params![
                route_epoch.as_slice(),
                provider_public_key,
                serialized
            ])
            .map_err(Self::database_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::db_initializer::{
        DbInitializationConfig, DbInitializer, DbInitializerReal,
    };
    use crate::sub_lib::cryptde::{CryptDE, PublicKey};
    use crate::sub_lib::cryptde_null::CryptDENull;
    use crate::sub_lib::service_receipt::{
        make_accounting_commitment, ReceiptSessionPolicy, ServiceKind, ServiceReceipt,
    };
    use crate::test_utils::make_paying_wallet;
    use masq_lib::test_utils::utils::{
        ensure_node_home_directory_does_not_exist, TEST_DEFAULT_CHAIN,
    };

    fn make_payload(route_epoch: [u8; 32]) -> ServiceReceiptPayload_0v1 {
        let provider_public_key = PublicKey::new(b"outbox provider");
        let provider = CryptDENull::from(&provider_public_key, TEST_DEFAULT_CHAIN);
        let payer_public_key = PublicKey::new(b"outbox payer session");
        let payer = CryptDENull::from(&payer_public_key, TEST_DEFAULT_CHAIN);
        let acknowledged_receipt = ServiceReceipt::new(
            route_epoch,
            1,
            ServiceKind::Exit,
            provider.public_key().clone(),
            make_accounting_commitment(&route_epoch, &payer_public_key),
            100,
            5,
            2,
        )
        .sign(&provider)
        .unwrap()
        .acknowledge(&payer)
        .unwrap();
        let wallet = make_paying_wallet(b"outbox wallet");
        let authorization = ReceiptSessionPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
            wallet.address(),
            payer_public_key,
            10_000,
            0,
            86_400,
            [0x42; 32],
        )
        .authorize(&wallet)
        .unwrap();
        ServiceReceiptPayload_0v1 {
            authorization,
            acknowledged_receipt,
        }
    }

    fn advance_payload(
        payload: &ServiceReceiptPayload_0v1,
        sequence: u64,
    ) -> ServiceReceiptPayload_0v1 {
        let current = &payload.acknowledged_receipt.signed_receipt.receipt;
        let provider = CryptDENull::from(&current.provider_public_key, TEST_DEFAULT_CHAIN);
        let payer = CryptDENull::from(
            &payload.acknowledged_receipt.payer_session_public_key,
            TEST_DEFAULT_CHAIN,
        );
        let acknowledged_receipt = current
            .next_for_same_route(sequence, ServiceKind::Exit, 200, 5, 2)
            .unwrap()
            .sign(&provider)
            .unwrap()
            .acknowledge(&payer)
            .unwrap();
        ServiceReceiptPayload_0v1 {
            authorization: payload.authorization.clone(),
            acknowledged_receipt,
        }
    }

    #[test]
    fn acknowledgement_is_durable_idempotent_and_deleted_only_by_exact_identity() {
        let home_dir = ensure_node_home_directory_does_not_exist(
            "receipt_acknowledgement_outbox_dao",
            "acknowledgement_is_durable_idempotent_and_deleted_only_by_exact_identity",
        );
        let initializer = DbInitializerReal::default();
        let conn = initializer
            .initialize(&home_dir, DbInitializationConfig::test_default())
            .unwrap();
        let mut subject = ReceiptAcknowledgementOutboxDaoReal::new(conn);
        let payload = make_payload([0x41; 32]);

        subject.enqueue(&payload, UNIX_EPOCH).unwrap();
        subject.enqueue(&payload, UNIX_EPOCH).unwrap();
        assert_eq!(subject.pending().unwrap(), vec![payload.clone()]);
        drop(subject);

        let conn = initializer
            .initialize(&home_dir, DbInitializationConfig::test_default())
            .unwrap();
        let mut subject = ReceiptAcknowledgementOutboxDaoReal::new(conn);
        assert_eq!(subject.pending().unwrap(), vec![payload.clone()]);
        subject.delete(&make_payload([0x43; 32])).unwrap();
        assert_eq!(subject.pending().unwrap(), vec![payload.clone()]);
        subject.delete(&payload).unwrap();
        assert!(subject.pending().unwrap().is_empty());
    }

    #[test]
    fn route_epoch_cannot_be_reused_for_a_different_acknowledgement() {
        let home_dir = ensure_node_home_directory_does_not_exist(
            "receipt_acknowledgement_outbox_dao",
            "route_epoch_cannot_be_reused_for_a_different_acknowledgement",
        );
        let initializer = DbInitializerReal::default();
        let conn = initializer
            .initialize(&home_dir, DbInitializationConfig::test_default())
            .unwrap();
        let mut subject = ReceiptAcknowledgementOutboxDaoReal::new(conn);
        let payload = make_payload([0x51; 32]);
        subject.enqueue(&payload, UNIX_EPOCH).unwrap();
        let mut conflicting = payload;
        conflicting
            .acknowledged_receipt
            .signed_receipt
            .receipt
            .payload_size += 1;

        assert_eq!(
            subject.enqueue(&conflicting, UNIX_EPOCH),
            Err(ReceiptAcknowledgementOutboxDaoError::IdentityMismatch)
        );
    }

    #[test]
    fn newer_cumulative_acknowledgement_supersedes_older_without_delete_race() {
        let home_dir = ensure_node_home_directory_does_not_exist(
            "receipt_acknowledgement_outbox_dao",
            "newer_cumulative_acknowledgement_supersedes_older_without_delete_race",
        );
        let initializer = DbInitializerReal::default();
        let conn = initializer
            .initialize(&home_dir, DbInitializationConfig::test_default())
            .unwrap();
        let mut subject = ReceiptAcknowledgementOutboxDaoReal::new(conn);
        let older = make_payload([0x61; 32]);
        let newer = advance_payload(&older, 2);

        subject.enqueue(&older, UNIX_EPOCH).unwrap();
        subject.enqueue(&newer, UNIX_EPOCH).unwrap();
        assert_eq!(subject.pending().unwrap(), vec![newer.clone()]);

        subject.enqueue(&older, UNIX_EPOCH).unwrap();
        subject.delete(&older).unwrap();
        assert_eq!(subject.pending().unwrap(), vec![newer.clone()]);

        subject.delete(&newer).unwrap();
        assert!(subject.pending().unwrap().is_empty());
    }

    #[test]
    fn cumulative_replacement_decision_is_identity_bound_and_monotonic_without_a_database() {
        let older = make_payload([0x71; 32]);
        let newer = advance_payload(&older, 2);

        assert_eq!(
            ReceiptAcknowledgementOutboxDaoReal::cumulative_enqueue_disposition(&older, &older),
            Ok(CumulativeEnqueueDisposition::Idempotent)
        );
        assert_eq!(
            ReceiptAcknowledgementOutboxDaoReal::cumulative_enqueue_disposition(&older, &newer),
            Ok(CumulativeEnqueueDisposition::Replace)
        );
        assert_eq!(
            ReceiptAcknowledgementOutboxDaoReal::cumulative_enqueue_disposition(&newer, &older),
            Ok(CumulativeEnqueueDisposition::Stale)
        );

        let mut inconsistent = newer.clone();
        inconsistent
            .acknowledged_receipt
            .signed_receipt
            .receipt
            .cumulative_charge_wei = older
            .acknowledged_receipt
            .signed_receipt
            .receipt
            .cumulative_charge_wei;
        assert_eq!(
            ReceiptAcknowledgementOutboxDaoReal::cumulative_enqueue_disposition(
                &older,
                &inconsistent,
            ),
            Err(ReceiptAcknowledgementOutboxDaoError::IdentityMismatch)
        );
        assert_eq!(
            format!(
                "{:?}",
                ReceiptAcknowledgementOutboxDaoError::Deserialization(
                    "private acknowledgement marker".to_string()
                )
            ),
            "Deserialization([REDACTED])"
        );
    }
}
