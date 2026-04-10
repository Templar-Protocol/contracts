use near_sdk::{borsh::BorshSerialize, BorshStorageKey};

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
pub enum StorageKey {
    Governance,
    Proxies,
}
