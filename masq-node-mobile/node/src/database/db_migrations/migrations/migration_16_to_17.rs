// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::database::db_migrations::db_migrator::DatabaseMigration;
use crate::database::db_migrations::migrator_utils::DBMigDeclarator;

#[allow(non_camel_case_types)]
pub struct Migrate_16_to_17;

impl DatabaseMigration for Migrate_16_to_17 {
    fn migrate<'a>(
        &self,
        declaration_utils: Box<dyn DBMigDeclarator + 'a>,
    ) -> rusqlite::Result<()> {
        declaration_utils.execute_upon_transaction(&[
            &"insert into config (name, value, encrypted)
              values ('receipt_session_recovery_key', null, 1)",
            &"create table receipt_session_recovery (
                singleton_id integer primary key check(singleton_id = 1),
                authorization_nonce blob not null,
                payer_session_public_key blob not null,
                expires_at_unix_s text not null,
                encrypted_header blob not null
            ) strict",
            &"create table receipt_session_route_recovery (
                stream_key_cbor blob primary key,
                route_epoch blob not null,
                encrypted_route blob not null
            ) strict",
        ])
    }

    fn old_version(&self) -> usize {
        16
    }
}

#[cfg(test)]
mod tests {
    use crate::database::db_initializer::{
        DbInitializationConfig, DbInitializer, DbInitializerReal, DATABASE_FILE,
    };
    use crate::db_config::config_dao::{ConfigDao, ConfigDaoReal};
    use crate::test_utils::database_utils::{
        assert_table_exists, bring_db_0_back_to_life_and_return_connection, make_external_data,
    };
    use masq_lib::constants::RECEIPT_SESSION_RECOVERY_KEY;
    use masq_lib::test_utils::logging::{init_test_logging, TestLogHandler};
    use masq_lib::test_utils::utils::ensure_node_home_directory_exists;
    use std::fs::create_dir_all;

    #[test]
    fn migration_from_16_to_17_creates_encrypted_consumer_recovery_state() {
        init_test_logging();
        let dir_path = ensure_node_home_directory_exists(
            "db_migrations",
            "migration_from_16_to_17_creates_encrypted_consumer_recovery_state",
        );
        create_dir_all(&dir_path).unwrap();
        let db_path = dir_path.join(DATABASE_FILE);
        let _ = bring_db_0_back_to_life_and_return_connection(&db_path);
        let subject = DbInitializerReal::default();
        let _previous_connection = subject
            .initialize_to_version(
                &dir_path,
                16,
                DbInitializationConfig::create_or_migrate(make_external_data()),
            )
            .unwrap();

        let connection = subject
            .initialize_to_version(
                &dir_path,
                17,
                DbInitializationConfig::create_or_migrate(make_external_data()),
            )
            .unwrap();

        assert_table_exists(connection.as_ref(), "receipt_session_recovery");
        assert_table_exists(connection.as_ref(), "receipt_session_route_recovery");
        let config = ConfigDaoReal::new(connection)
            .get(RECEIPT_SESSION_RECOVERY_KEY)
            .unwrap();
        assert!(config.encrypted);
        assert_eq!(config.value_opt, None);
        TestLogHandler::new().assert_logs_contain_in_order(vec![
            "DbMigrator: Database successfully migrated from version 16 to 17",
        ]);
    }
}
