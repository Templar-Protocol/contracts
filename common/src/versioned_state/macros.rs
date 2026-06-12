#[macro_export]
macro_rules! impl_versioned_state {
    ($contract: ident, $current_state: ty, $migrations: ty) => {
        #[::near_sdk::near]
        impl $crate::versioned_state::MigrateExternalInterface for $contract {
            fn get_stored_state_version() -> u32 {
                $crate::versioned_state::read_state_version()
                    .unwrap_or_else(|e| ::near_sdk::env::panic_str(&e.to_string()))
            }

            fn get_target_state_version() -> u32 {
                <$current_state as $crate::versioned_state::StateVersion>::VERSION
            }

            fn needs_migration() -> bool {
                <$current_state as $crate::versioned_state::StateVersion>::needs_migration()
                    .unwrap_or_else(|e| ::near_sdk::env::panic_str(&e.to_string()))
            }
        }

        #[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
        #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
        pub fn migrate() {
            use near_sdk::env;
            env::setup_panic_hook();

            ::near_sdk::require!(
                env::predecessor_account_id() == env::current_account_id(),
                "migrate function is private",
            );

            let input = env::input().unwrap_or_else(|| env::panic_str("no input"));

            let args: $migrations = ::near_sdk::serde_json::from_slice(&input)
                .unwrap_or_else(|e| env::panic_str(&e.to_string()));

            $crate::versioned_state::Migrator::run(args);
        }
    };
}

pub use impl_versioned_state;
