// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use super::receipt_session::{
    PersistedReceiptSessionHeader, ReceiptRouteState, ReceiptSessionRecoveryStore,
};
use crate::accountant::db_access_objects::utils::DaoFactoryReal;
use crate::database::rusqlite_wrappers::{ConnectionWrapper, TransactionSafeWrapper};
use crate::db_config::config_dao::{ConfigDao, ConfigDaoReal};
use crate::db_config::secure_config_layer::SecureConfigLayer;
use crate::sub_lib::cryptde::{CryptDE, CryptData, PlainData, SymmetricKey};
use crate::sub_lib::cryptde_real::CryptDEReal;
use masq_lib::blockchains::chains::Chain;
use masq_lib::constants::RECEIPT_SESSION_RECOVERY_KEY;
use rusqlite::OptionalExtension;
use rustc_hex::{FromHex, ToHex};

const RECOVERY_KEY_BYTES: usize = 32;
const SINGLETON_ID: i64 = 1;

pub struct ReceiptSessionRecoveryStoreReal {
    conn: Box<dyn ConnectionWrapper>,
    cryptde: CryptDEReal,
    recovery_key: SymmetricKey,
}

impl ReceiptSessionRecoveryStoreReal {
    pub fn new(factory: &DaoFactoryReal, db_password: &str, chain: Chain) -> Result<Self, String> {
        let config_dao: Box<dyn ConfigDao> =
            Box::new(ConfigDaoReal::new(factory.make_connection()));
        let secure = SecureConfigLayer::new();
        let record = config_dao
            .get(RECEIPT_SESSION_RECOVERY_KEY)
            .map_err(|error| format!("recovery-key configuration is unavailable: {:?}", error))?;
        let recovery_key_hex = match record.value_opt.as_ref() {
            Some(_) => secure
                .decrypt(record, Some(db_password.to_string()), &config_dao)
                .map_err(|error| format!("cannot decrypt recovery key: {:?}", error))?
                .ok_or_else(|| "encrypted recovery key is empty".to_string())?,
            None => {
                let cryptde = CryptDEReal::new(chain);
                let generated = cryptde.gen_key_sym().as_slice().to_hex::<String>();
                let encrypted = secure
                    .encrypt(
                        RECEIPT_SESSION_RECOVERY_KEY,
                        Some(generated.clone()),
                        Some(db_password.to_string()),
                        &config_dao,
                    )
                    .map_err(|error| format!("cannot encrypt recovery key: {:?}", error))?
                    .ok_or_else(|| "recovery-key encryption returned no value".to_string())?;
                config_dao
                    .set(RECEIPT_SESSION_RECOVERY_KEY, Some(encrypted))
                    .map_err(|error| format!("cannot store recovery key: {:?}", error))?;
                generated
            }
        };
        let recovery_key_bytes: Vec<u8> = recovery_key_hex
            .from_hex()
            .map_err(|error| format!("recovery key is not hexadecimal: {:?}", error))?;
        if recovery_key_bytes.len() != RECOVERY_KEY_BYTES {
            return Err(format!(
                "recovery key has {} bytes instead of {}",
                recovery_key_bytes.len(),
                RECOVERY_KEY_BYTES
            ));
        }
        Ok(Self {
            conn: factory.make_connection(),
            cryptde: CryptDEReal::new(chain),
            recovery_key: SymmetricKey::new(&recovery_key_bytes),
        })
    }

    fn encrypt<T: serde::Serialize>(&self, value: &T) -> Result<Vec<u8>, String> {
        let serialized = serde_cbor::to_vec(value)
            .map_err(|error| format!("cannot serialize recovery state: {}", error))?;
        self.cryptde
            .encode_sym(&self.recovery_key, &PlainData::new(&serialized))
            .map(|encrypted| encrypted.as_slice().to_vec())
            .map_err(|error| format!("cannot encrypt recovery state: {:?}", error))
    }

    fn decrypt<T: serde::de::DeserializeOwned>(&self, encrypted: &[u8]) -> Result<T, String> {
        let plain = self
            .cryptde
            .decode_sym(&self.recovery_key, &CryptData::new(encrypted))
            .map_err(|error| format!("cannot authenticate recovery state: {:?}", error))?;
        serde_cbor::from_slice(plain.as_slice())
            .map_err(|error| format!("cannot deserialize recovery state: {}", error))
    }

    fn stream_key_cbor(route: &ReceiptRouteState) -> Result<Vec<u8>, String> {
        serde_cbor::to_vec(&route.stream_key)
            .map_err(|error| format!("cannot serialize recovery stream key: {}", error))
    }

    fn save_header_with<E>(
        execute: E,
        header: &PersistedReceiptSessionHeader,
        encrypted: &[u8],
    ) -> Result<(), String>
    where
        E: FnOnce(&str, &[&dyn rusqlite::ToSql]) -> rusqlite::Result<usize>,
    {
        let policy = &header.authorization.policy;
        let params: &[&dyn rusqlite::ToSql] = &[
            &SINGLETON_ID,
            &policy.authorization_nonce.as_slice(),
            &policy.payer_session_public_key.as_slice(),
            &policy.expires_at_unix_s.to_string(),
            &encrypted,
        ];
        execute(
            "insert into receipt_session_recovery
             (singleton_id, authorization_nonce, payer_session_public_key,
              expires_at_unix_s, encrypted_header)
             values (?1, ?2, ?3, ?4, ?5)
             on conflict(singleton_id) do update set
              authorization_nonce = excluded.authorization_nonce,
              payer_session_public_key = excluded.payer_session_public_key,
              expires_at_unix_s = excluded.expires_at_unix_s,
              encrypted_header = excluded.encrypted_header",
            params,
        )
        .map(|_| ())
        .map_err(|error| format!("cannot save recovery header: {}", error))
    }

    fn save_header_in_transaction(
        transaction: &TransactionSafeWrapper,
        header: &PersistedReceiptSessionHeader,
        encrypted: &[u8],
    ) -> Result<(), String> {
        Self::save_header_with(
            |sql, params| transaction.prepare(sql)?.execute(params),
            header,
            encrypted,
        )
    }
}

impl ReceiptSessionRecoveryStore for ReceiptSessionRecoveryStoreReal {
    fn load(
        &mut self,
        now_unix_s: u64,
    ) -> Result<Option<(PersistedReceiptSessionHeader, Vec<ReceiptRouteState>)>, String> {
        let header_row_opt: Option<(Vec<u8>, Vec<u8>, String, Vec<u8>)> = self
            .conn
            .prepare(
                "select authorization_nonce, payer_session_public_key, expires_at_unix_s,
                        encrypted_header
                 from receipt_session_recovery where singleton_id = ?1",
            )
            .map_err(|error| format!("cannot prepare recovery header load: {}", error))?
            .query_row(rusqlite::params![SINGLETON_ID], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()
            .map_err(|error| format!("cannot load recovery header: {}", error))?;
        let (authorization_nonce, payer_session_public_key, expires_at, encrypted_header) =
            match header_row_opt {
                Some(row) => row,
                None => return Ok(None),
            };
        let expires_at_unix_s = expires_at
            .parse::<u64>()
            .map_err(|error| format!("recovery expiry is invalid: {}", error))?;
        if expires_at_unix_s < now_unix_s {
            self.clear()?;
            return Ok(None);
        }
        let header: PersistedReceiptSessionHeader = self.decrypt(&encrypted_header)?;
        if header.authorization.policy.authorization_nonce.as_slice() != authorization_nonce
            || header
                .authorization
                .policy
                .payer_session_public_key
                .as_slice()
                != payer_session_public_key
            || header.authorization.policy.expires_at_unix_s != expires_at_unix_s
        {
            return Err("recovery header identity does not match its index".to_string());
        }
        let mut statement = self
            .conn
            .prepare(
                "select stream_key_cbor, route_epoch, encrypted_route
                 from receipt_session_route_recovery order by stream_key_cbor",
            )
            .map_err(|error| format!("cannot prepare recovery route load: {}", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(|error| format!("cannot load recovery routes: {}", error))?;
        let mut routes = vec![];
        for row in rows {
            let (stream_key_cbor, route_epoch, encrypted_route) =
                row.map_err(|error| format!("cannot read recovery route: {}", error))?;
            let route: ReceiptRouteState = self.decrypt(&encrypted_route)?;
            if Self::stream_key_cbor(&route)? != stream_key_cbor
                || route.request.route_epoch.as_slice() != route_epoch
                || route.request.authorization != header.authorization
            {
                return Err("recovery route identity does not match its index".to_string());
            }
            routes.push(route);
        }
        Ok(Some((header, routes)))
    }

    fn save_header(&mut self, header: &PersistedReceiptSessionHeader) -> Result<(), String> {
        let encrypted = self.encrypt(header)?;
        Self::save_header_with(
            |sql, params| self.conn.prepare(sql)?.execute(params),
            header,
            &encrypted,
        )
    }

    fn save_header_and_route(
        &mut self,
        header: &PersistedReceiptSessionHeader,
        route: &ReceiptRouteState,
    ) -> Result<(), String> {
        if route.request.authorization != header.authorization {
            return Err("route authorization differs from recovery header".to_string());
        }
        let encrypted_header = self.encrypt(header)?;
        let encrypted_route = self.encrypt(route)?;
        let stream_key_cbor = Self::stream_key_cbor(route)?;
        let transaction = self
            .conn
            .transaction()
            .map_err(|error| format!("cannot begin recovery transaction: {}", error))?;
        Self::save_header_in_transaction(&transaction, header, &encrypted_header)?;
        transaction
            .execute(
                "insert into receipt_session_route_recovery
                 (stream_key_cbor, route_epoch, encrypted_route)
                 values (?1, ?2, ?3)
                 on conflict(stream_key_cbor) do update set
                  route_epoch = excluded.route_epoch,
                  encrypted_route = excluded.encrypted_route",
                rusqlite::params![
                    &stream_key_cbor,
                    route.request.route_epoch.as_slice(),
                    &encrypted_route,
                ],
            )
            .map_err(|error| format!("cannot save recovery route: {}", error))?;
        transaction
            .commit()
            .map_err(|error| format!("cannot commit recovery route: {}", error))
    }

    fn clear(&mut self) -> Result<(), String> {
        let transaction = self
            .conn
            .transaction()
            .map_err(|error| format!("cannot begin recovery cleanup: {}", error))?;
        transaction
            .execute("delete from receipt_session_route_recovery", &[])
            .map_err(|error| format!("cannot clear recovery routes: {}", error))?;
        transaction
            .execute(
                "delete from receipt_session_recovery where singleton_id = ?1",
                rusqlite::params![SINGLETON_ID],
            )
            .map_err(|error| format!("cannot clear recovery header: {}", error))?;
        transaction
            .commit()
            .map_err(|error| format!("cannot commit recovery cleanup: {}", error))
    }
}
