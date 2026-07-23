// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::database::db_migrations::db_migrator::DatabaseMigration;
use crate::database::db_migrations::migrator_utils::DBMigDeclarator;

#[allow(non_camel_case_types)]
pub struct Migrate_13_to_14;

impl DatabaseMigration for Migrate_13_to_14 {
    fn migrate<'a>(
        &self,
        declaration_utils: Box<dyn DBMigDeclarator + 'a>,
    ) -> rusqlite::Result<()> {
        let receipt_sequence_checkpoint = "create table receipt_sequence_checkpoint (
                route_epoch blob not null,
                provider_public_key blob not null,
                payer_session_public_key blob not null,
                last_sequence text not null,
                cumulative_charge_wei text not null,
                checkpoint_cbor blob not null,
                primary key (route_epoch, provider_public_key, payer_session_public_key)
            ) strict";
        let receipt_session_authorization = "create table receipt_session_authorization (
                authorization_nonce blob primary key,
                expires_at_unix_s text not null,
                spent_charge_wei text not null default '0',
                authorization_cbor blob not null
            ) strict";

        declaration_utils.execute_upon_transaction(&[
            &receipt_sequence_checkpoint,
            &receipt_session_authorization,
        ])
    }

    fn old_version(&self) -> usize {
        13
    }
}

#[cfg(test)]
mod tests {
    use crate::database::db_initializer::{
        DbInitializationConfig, DbInitializer, DbInitializerReal, DATABASE_FILE,
    };
    use crate::test_utils::database_utils::{
        assert_table_exists, bring_db_0_back_to_life_and_return_connection, make_external_data,
    };
    use masq_lib::test_utils::logging::{init_test_logging, TestLogHandler};
    use masq_lib::test_utils::utils::ensure_node_home_directory_exists;
    use std::fs::create_dir_all;

    #[test]
    fn migration_from_13_to_14_creates_receipt_replay_tables() {
        init_test_logging();
        let dir_path = ensure_node_home_directory_exists(
            "db_migrations",
            "migration_from_13_to_14_creates_receipt_replay_tables",
        );
        create_dir_all(&dir_path).unwrap();
        let db_path = dir_path.join(DATABASE_FILE);
        let _ = bring_db_0_back_to_life_and_return_connection(&db_path);
        let subject = DbInitializerReal::default();
        let _previous_connection = subject
            .initialize_to_version(
                &dir_path,
                13,
                DbInitializationConfig::create_or_migrate(make_external_data()),
            )
            .unwrap();

        let connection = subject
            .initialize_to_version(
                &dir_path,
                14,
                DbInitializationConfig::create_or_migrate(make_external_data()),
            )
            .unwrap();

        assert_table_exists(connection.as_ref(), "receipt_sequence_checkpoint");
        assert_table_exists(connection.as_ref(), "receipt_session_authorization");
        TestLogHandler::new().assert_logs_contain_in_order(vec![
            "DbMigrator: Database successfully migrated from version 13 to 14",
        ]);
    }
}
