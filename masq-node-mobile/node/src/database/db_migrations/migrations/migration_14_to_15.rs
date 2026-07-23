// Copyright (c) 2026, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::database::db_migrations::db_migrator::DatabaseMigration;
use crate::database::db_migrations::migrator_utils::DBMigDeclarator;

#[allow(non_camel_case_types)]
pub struct Migrate_14_to_15;

impl DatabaseMigration for Migrate_14_to_15 {
    fn migrate<'a>(
        &self,
        declaration_utils: Box<dyn DBMigDeclarator + 'a>,
    ) -> rusqlite::Result<()> {
        declaration_utils.execute_upon_transaction(&[
            &"create table receipt_acknowledgement_outbox (
                route_epoch blob primary key,
                provider_public_key blob not null,
                acknowledgement_cbor blob not null,
                created_at_unix_s text not null
            ) strict",
        ])
    }

    fn old_version(&self) -> usize {
        14
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
    fn migration_from_14_to_15_creates_receipt_acknowledgement_outbox() {
        init_test_logging();
        let dir_path = ensure_node_home_directory_exists(
            "db_migrations",
            "migration_from_14_to_15_creates_receipt_acknowledgement_outbox",
        );
        create_dir_all(&dir_path).unwrap();
        let db_path = dir_path.join(DATABASE_FILE);
        let _ = bring_db_0_back_to_life_and_return_connection(&db_path);
        let subject = DbInitializerReal::default();
        let _previous_connection = subject
            .initialize_to_version(
                &dir_path,
                14,
                DbInitializationConfig::create_or_migrate(make_external_data()),
            )
            .unwrap();

        let connection = subject
            .initialize_to_version(
                &dir_path,
                15,
                DbInitializationConfig::create_or_migrate(make_external_data()),
            )
            .unwrap();

        assert_table_exists(connection.as_ref(), "receipt_acknowledgement_outbox");
        TestLogHandler::new().assert_logs_contain_in_order(vec![
            "DbMigrator: Database successfully migrated from version 14 to 15",
        ]);
    }
}
