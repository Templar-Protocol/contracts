use near_sdk::{env, near};
use templar_universal_account::contract_state::{Migration, STATE_VERSION};

use crate::{Contract, ContractExt};

const VERSION_KEY: &[u8] = b"__v";

impl Contract {
    fn write_state_version(state_version: u32) {
        env::storage_write(VERSION_KEY, &state_version.to_le_bytes());
    }
}

#[near]
impl Contract {
    pub fn get_stored_state_version() -> u32 {
        env::storage_read(VERSION_KEY)
            .filter(|v| v.len() == 4)
            .map_or(0, |v| {
                let mut buf = [0u8; 4];
                buf.copy_from_slice(&v);
                u32::from_le_bytes(buf)
            })
    }

    pub fn get_target_state_version() -> u32 {
        STATE_VERSION
    }

    pub fn needs_migration() -> bool {
        Self::get_stored_state_version() < Self::get_target_state_version()
    }

    #[init(ignore_state)]
    #[private]
    pub fn migrate(args: Migration) -> Self {
        let from_version = Self::get_stored_state_version();
        let expected_from_version = args.input_version();

        if from_version != expected_from_version {
            templar_common::panic_with_message(&format!("Stored state version {from_version} != args `from_version` {expected_from_version}"));
        }

        Self::write_state_version(args.output_version());

        Self(
            args.migrate()
                .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string())),
        )
    }
}
