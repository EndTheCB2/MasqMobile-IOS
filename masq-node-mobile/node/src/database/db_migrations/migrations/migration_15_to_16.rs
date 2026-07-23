// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::database::db_migrations::db_migrator::DatabaseMigration;
use crate::database::db_migrations::migrator_utils::DBMigDeclarator;

#[allow(non_camel_case_types)]
pub struct Migrate_15_to_16;

impl DatabaseMigration for Migrate_15_to_16 {
    fn migrate<'a>(
        &self,
        declaration_utils: Box<dyn DBMigDeclarator + 'a>,
    ) -> rusqlite::Result<()> {
        declaration_utils.execute_upon_transaction(&[&"create table routing_receipt_offer_state (
                authorization_nonce blob not null,
                route_epoch blob not null,
                provider_public_key blob not null,
                payer_session_public_key blob not null,
                expires_at_unix_s text not null,
                offer_state_cbor blob not null,
                primary key (authorization_nonce, route_epoch, provider_public_key,
                             payer_session_public_key)
            ) strict"])
    }

    fn old_version(&self) -> usize {
        15
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
    fn migration_from_15_to_16_creates_durable_routing_offer_state() {
        init_test_logging();
        let dir_path = ensure_node_home_directory_exists(
            "db_migrations",
            "migration_from_15_to_16_creates_durable_routing_offer_state",
        );
        create_dir_all(&dir_path).unwrap();
        let db_path = dir_path.join(DATABASE_FILE);
        let _ = bring_db_0_back_to_life_and_return_connection(&db_path);
        let subject = DbInitializerReal::default();
        let _previous_connection = subject
            .initialize_to_version(
                &dir_path,
                15,
                DbInitializationConfig::create_or_migrate(make_external_data()),
            )
            .unwrap();

        let connection = subject
            .initialize_to_version(
                &dir_path,
                16,
                DbInitializationConfig::create_or_migrate(make_external_data()),
            )
            .unwrap();

        assert_table_exists(connection.as_ref(), "routing_receipt_offer_state");
        TestLogHandler::new().assert_logs_contain_in_order(vec![
            "DbMigrator: Database successfully migrated from version 15 to 16",
        ]);
    }
}
