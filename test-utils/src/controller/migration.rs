use near_sdk::{Gas, NearToken};
use near_workspaces::{result::ExecutionSuccess, Account};

use crate::define;

use super::ContractController;

pub trait MigrationController: ContractController {
    type Migration: near_sdk::serde::Serialize;

    define! {
        #[view]
        fn get_stored_state_version() -> u32;
        #[view]
        fn get_target_state_version() -> u32;
        #[view]
        fn needs_migration() -> bool;
    }

    async fn migrate(
        &self,
        executor: &Account,
        args: impl Into<Self::Migration>,
    ) -> ExecutionSuccess {
        self.call_exec(
            executor,
            "migrate",
            args.into(),
            NearToken::from_yoctonear(0),
            Gas::from_tgas(300),
        )
        .await
    }
}
