//! Minimal vendored fungible token primitives.
//!
//! Copied from [OpenZeppelin stellar-contracts](https://github.com/OpenZeppelin/stellar-contracts)
//! crate `stellar-tokens` v0.5.0 (crates.io).
//!
//! Only the subset needed by the Templar vault share token is included:
//! `Base::update`, `Base::balance`, and event emitters (`emit_mint`,
//! `emit_transfer`, `emit_burn`).
//!
//! **Why**: The full `stellar-tokens` crate pulls in 44 `#[contracttype]` and
//! 11 `#[contracterror]` from NFT, RWA, identity, compliance, and distributor
//! modules — none of which this contract uses. Those types inflate the WASM
//! `contractspecv0` section by ~34 KB, pushing the binary over Soroban's
//! 128 KiB deployment limit.

use soroban_sdk::{contracterror, contractevent, contracttype, panic_with_error, Address, Env};

// ─── Constants ───────────────────────────────────────────────────────────────

const DAY_IN_LEDGERS: u32 = 17280;
pub const BALANCE_EXTEND_AMOUNT: u32 = 30 * DAY_IN_LEDGERS;
pub const BALANCE_TTL_THRESHOLD: u32 = BALANCE_EXTEND_AMOUNT - DAY_IN_LEDGERS;

// ─── Storage keys ────────────────────────────────────────────────────────────

/// Storage keys for fungible token data.
///
/// NOTE: The discriminants and variant shapes **must** stay identical to
/// `stellar_tokens::fungible::StorageKey` so that on-chain storage written by
/// earlier contract versions remains readable.
#[contracttype]
pub enum StorageKey {
    TotalSupply,
    Balance(Address),
}

// ─── Errors ──────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Eq, PartialEq, PartialOrd, Ord)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[repr(u32)]
pub enum FungibleTokenError {
    InsufficientBalance = 100,
    InsufficientAllowance = 101,
    InvalidLiveUntilLedger = 102,
    LessThanZero = 103,
    MathOverflow = 104,
    UnsetMetadata = 105,
    ExceededCap = 106,
    InvalidCap = 107,
    CapNotSet = 108,
    SACNotSet = 109,
    SACAddressMismatch = 110,
    SACMissingFnParam = 111,
    SACInvalidFnParam = 112,
    UserNotAllowed = 113,
    UserBlocked = 114,
}

// ─── Base token primitive ────────────────────────────────────────────────────

/// Marker struct mirroring `stellar_tokens::fungible::Base`.
pub struct Base;

impl Base {
    /// Returns the amount of tokens held by `account`. Defaults to `0`.
    pub fn balance(e: &Env, account: &Address) -> i128 {
        let key = StorageKey::Balance(account.clone());
        if let Some(balance) = e.storage().persistent().get::<_, i128>(&key) {
            e.storage()
                .persistent()
                .extend_ttl(&key, BALANCE_TTL_THRESHOLD, BALANCE_EXTEND_AMOUNT);
            balance
        } else {
            0
        }
    }

    /// Returns the total amount of tokens in circulation.
    pub fn total_supply(e: &Env) -> i128 {
        e.storage()
            .instance()
            .get(&StorageKey::TotalSupply)
            .unwrap_or(0)
    }

    /// Core mint / burn / transfer primitive.
    ///
    /// - `from = None` → mint (increases total supply)
    /// - `to = None`   → burn (decreases total supply)
    /// - Both `Some`   → transfer
    pub fn update(e: &Env, from: Option<&Address>, to: Option<&Address>, amount: i128) {
        if amount < 0 {
            panic_with_error!(e, FungibleTokenError::LessThanZero);
        }
        if let Some(account) = from {
            let mut from_balance = Base::balance(e, account);
            if from_balance < amount {
                panic_with_error!(e, FungibleTokenError::InsufficientBalance);
            }
            from_balance -= amount;
            e.storage()
                .persistent()
                .set(&StorageKey::Balance(account.clone()), &from_balance);
        } else {
            let total_supply = Base::total_supply(e);
            let Some(new_total_supply) = total_supply.checked_add(amount) else {
                panic_with_error!(e, FungibleTokenError::MathOverflow);
            };
            e.storage()
                .instance()
                .set(&StorageKey::TotalSupply, &new_total_supply);
        }

        if let Some(account) = to {
            let to_balance = Base::balance(e, account) + amount;
            e.storage()
                .persistent()
                .set(&StorageKey::Balance(account.clone()), &to_balance);
        } else {
            let total_supply = Base::total_supply(e) - amount;
            e.storage()
                .instance()
                .set(&StorageKey::TotalSupply, &total_supply);
        }
    }
}

// ─── Events ──────────────────────────────────────────────────────────────────

#[contractevent]
#[derive(Clone, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub struct Transfer {
    #[topic]
    pub from: Address,
    #[topic]
    pub to: Address,
    pub amount: i128,
}

pub fn emit_transfer(e: &Env, from: &Address, to: &Address, amount: i128) {
    Transfer {
        from: from.clone(),
        to: to.clone(),
        amount,
    }
    .publish(e);
}

#[contractevent]
#[derive(Clone, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub struct Mint {
    #[topic]
    pub to: Address,
    pub amount: i128,
}

pub fn emit_mint(e: &Env, to: &Address, amount: i128) {
    Mint {
        to: to.clone(),
        amount,
    }
    .publish(e);
}

#[contractevent]
#[derive(Clone, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub struct Burn {
    #[topic]
    pub from: Address,
    pub amount: i128,
}

pub fn emit_burn(e: &Env, from: &Address, amount: i128) {
    Burn {
        from: from.clone(),
        amount,
    }
    .publish(e);
}
