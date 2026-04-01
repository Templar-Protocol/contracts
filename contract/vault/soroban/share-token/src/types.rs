use soroban_sdk::{contracterror, contracttype};

#[contracttype]
#[derive(Clone)]
pub(super) enum DataKey {
    Admin,
    Vault,
}

#[contracterror]
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub enum ShareTokenError {
    Unauthorized = 1,
    InvalidInput = 2,
    MissingConfig = 3,
}
