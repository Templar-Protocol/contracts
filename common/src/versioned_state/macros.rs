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

            // Stored-vs-target version — never `migrate_args` — decides whether to migrate, so a
            // caller can't skip a required migration or slip a stale transform past a current contract.
            let needs =
                <$current_state as $crate::versioned_state::StateVersion>::needs_migration()
                    .unwrap_or_else(|e| env::panic_str(&e.to_string()));

            let input = env::input().unwrap_or_default();

            if !needs {
                ::near_sdk::require!(
                    input.is_empty(),
                    "no state migration is needed; migrate_args must be empty",
                );
                return;
            }

            let args: $migrations = ::near_sdk::serde_json::from_slice(&input)
                .unwrap_or_else(|e| env::panic_str(&e.to_string()));

            // A contract may launch at its first state version with no migrations defined, making
            // `$migrations` an uninhabited (empty) enum. The deserialize above then has type `!`
            // (it can only panic), so this block is statically unreachable — expected, not a bug.
            #[allow(unreachable_code)]
            {
                $crate::versioned_state::Migrator::run(args);

                // One `Migrator::run` advances a single version step; re-assert we reached the target
                // so a deploy several versions ahead of stored reverts atomically instead of leaving
                // state stranded between versions.
                ::near_sdk::require!(
                    !<$current_state as $crate::versioned_state::StateVersion>::needs_migration()
                        .unwrap_or_else(|e| env::panic_str(&e.to_string())),
                    "state migration incomplete: stored version is still behind target",
                );
            }
        }
    };
}

pub use impl_versioned_state;
