use soroban_sdk::{contracterror, contracttype};

#[contracttype]
#[derive(Clone)]
pub(super) enum DataKey {
    Admin,
    Vault,
    Paused,
    Restrictions,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub enum RestrictionMode {
    None,
    Blacklist,
    Whitelist,
}

#[contracttype]
#[derive(Clone)]
pub struct Restrictions {
    pub mode: RestrictionMode,
    pub accounts: soroban_sdk::Vec<soroban_sdk::Address>,
}

#[contracterror]
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub enum ShareTokenError {
    Unauthorized = 1,
    InvalidInput = 2,
    MissingConfig = 3,
    VaultImmutable = 4,
    MetadataImmutable = 5,
    Paused = 6,
    Restricted = 7,
}
