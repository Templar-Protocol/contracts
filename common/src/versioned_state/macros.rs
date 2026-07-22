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

            // Accept either a single migration (legacy wire shape) or an ordered list, and run the
            // whole chain in one receipt: each step's output version must feed the next step's
            // input, the first must start at the stored version, and the last must land on target.
            // The chain is fully validated before any transform runs, so an invalid chain reverts
            // without writing state.
            let migrations: Vec<$migrations> = $crate::versioned_state::parse_one_or_many(&input)
                .unwrap_or_else(|e| env::panic_str(&e.to_string()));

            $crate::versioned_state::run_migration_chain(
                migrations,
                <$current_state as $crate::versioned_state::StateVersion>::VERSION,
            )
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));

            // Defense in depth: the chain validated its endpoints against the args, but re-assert
            // against live state so a run that somehow left state stranded reverts atomically.
            ::near_sdk::require!(
                !<$current_state as $crate::versioned_state::StateVersion>::needs_migration()
                    .unwrap_or_else(|e| env::panic_str(&e.to_string())),
                "state migration incomplete: stored version is still behind target",
            );
        }
    };
}

pub use impl_versioned_state;
