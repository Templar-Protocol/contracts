use near_sdk::ext_contract;

#[ext_contract]
pub trait MigrateExternalInterface {
    fn get_stored_state_version() -> u32;
    fn get_target_state_version() -> u32;
    fn needs_migration() -> bool;
}
