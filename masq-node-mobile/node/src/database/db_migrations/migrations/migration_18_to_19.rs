// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::database::db_migrations::db_migrator::DatabaseMigration;
use crate::database::db_migrations::migrator_utils::DBMigDeclarator;

#[allow(non_camel_case_types)]
pub struct Migrate_18_to_19;

impl DatabaseMigration for Migrate_18_to_19 {
    fn migrate<'a>(
        &self,
        declaration_utils: Box<dyn DBMigDeclarator + 'a>,
    ) -> rusqlite::Result<()> {
        declaration_utils.execute_upon_transaction(&[
            &"create table receipt_settlement_claim_archive (
                claim_id blob primary key,
                authorization_nonce blob not null,
                route_epoch blob not null,
                provider_public_key blob not null,
                payer_session_public_key blob not null,
                cumulative_charge_wei text not null,
                receipt_payload_cbor blob not null,
                accepted_at_unix_s text not null,
                confirmed_cumulative_charge_wei text not null,
                observed_block_number text not null,
                observed_block_hash blob not null
            ) strict",
            &"create table receipt_settlement_reconciliation_checkpoint (
                singleton_id integer primary key check(singleton_id = 1),
                chain_id text not null,
                settlement_contract blob not null,
                confirmation_depth text not null,
                observed_block_number text not null,
                observed_block_hash blob not null
            ) strict",
        ])
    }

    fn old_version(&self) -> usize {
        18
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
    fn migration_from_18_to_19_creates_reorg_recovery_tables() {
        init_test_logging();
        let dir_path = ensure_node_home_directory_exists(
            "db_migrations",
            "migration_from_18_to_19_creates_reorg_recovery_tables",
        );
        create_dir_all(&dir_path).unwrap();
        let db_path = dir_path.join(DATABASE_FILE);
        let _ = bring_db_0_back_to_life_and_return_connection(&db_path);
        let subject = DbInitializerReal::default();
        let _previous_connection = subject
            .initialize_to_version(
                &dir_path,
                18,
                DbInitializationConfig::create_or_migrate(make_external_data()),
            )
            .unwrap();

        let connection = subject
            .initialize_to_version(
                &dir_path,
                19,
                DbInitializationConfig::create_or_migrate(make_external_data()),
            )
            .unwrap();

        assert_table_exists(connection.as_ref(), "receipt_settlement_claim_archive");
        assert_table_exists(
            connection.as_ref(),
            "receipt_settlement_reconciliation_checkpoint",
        );
        TestLogHandler::new().assert_logs_contain_in_order(vec![
            "DbMigrator: Database successfully migrated from version 18 to 19",
        ]);
    }
}
