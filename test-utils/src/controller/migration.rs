use crate::define;

use super::ContractController;

pub trait MigrationController: ContractController {
    type Migration: near_sdk::serde::Serialize;

    define! {
        #[view]
        fn mig_stored_state_version() -> u32;
        #[view]
        fn mig_target_state_version() -> u32;
        #[view]
        fn mig_needs_migration() -> bool;

        #[call(exec, tgas(300))]
        fn migrate(args: Self::Migration);
    }
}
