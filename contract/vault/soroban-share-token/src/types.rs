use soroban_sdk::{contracterror, contractevent, contracttype, Address};

#[contracttype]
#[derive(Clone)]
pub(super) enum DataKey {
    Admin,
    Vault,
    Name,
    Symbol,
    Decimals,
    TotalSupply,
    Balance(Address),
}

#[contractevent]
#[derive(Clone)]
pub struct Transfer {
    #[topic]
    pub from: Address,
    #[topic]
    pub to: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone)]
pub struct Mint {
    #[topic]
    pub to: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone)]
pub struct Burn {
    #[topic]
    pub from: Address,
    pub amount: i128,
}

#[contracterror]
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub enum ShareTokenError {
    Unauthorized = 1,
    InvalidInput = 2,
    MissingConfig = 3,
    InsufficientBalance = 4,
    ArithmeticOverflow = 5,
}
