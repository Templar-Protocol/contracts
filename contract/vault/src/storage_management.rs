use near_sdk::borsh::{self, BorshSerialize};
use near_sdk::{env, AccountId};
use std::collections::HashSet;
use templar_common::vault::MarketConfiguration;

// Conservative per-entry overheads to cover collection metadata, prefixes, etc.
pub const MAP_ENTRY_OVERHEAD: u64 = 64;
pub const VEC_ITEM_OVERHEAD: u64 = 16;

// Borsh length of an AccountId (4-byte length + bytes)
pub fn storage_bytes_for_account_id(id: &AccountId) -> u64 {
    4 + (id.as_str().as_bytes().len() as u64)
}

pub fn storage_bytes_for_queue_item(id: &AccountId) -> u64 {
    VEC_ITEM_OVERHEAD + storage_bytes_for_account_id(id)
}

pub fn storage_bytes_for_config_entry(market: &AccountId) -> u64 {
    let key = storage_bytes_for_account_id(market);
    // Value size from default config serialization (upper-bound enough for our use)
    let cfg = MarketConfiguration::default();
    let val = borsh::to_vec(&cfg).map(|v| v.len() as u64).unwrap_or(32);
    MAP_ENTRY_OVERHEAD + key + val
}

pub fn storage_bytes_for_market_supply_entry(market: &AccountId) -> u64 {
    let key = storage_bytes_for_account_id(market);
    // u128 principal
    let val = 16u64;
    MAP_ENTRY_OVERHEAD + key + val
}

pub fn storage_bytes_for_pending_cap_entry(market: &AccountId) -> u64 {
    let key = storage_bytes_for_account_id(market);
    // PendingValue { value: u128, valid_at: u64 }
    let val = 16u64 + 8u64;
    MAP_ENTRY_OVERHEAD + key + val
}

pub fn storage_bytes_for_pending_withdrawal(owner: &AccountId, receiver: &AccountId) -> u64 {
    // Key is u64 id -> 8 bytes; value is Borsh of the struct members
    let key = 8u64;
    let val = storage_bytes_for_account_id(owner)
        + storage_bytes_for_account_id(receiver)
        + 16  // escrow_shares: u128
        + 16  // expected_assets: u128
        + 8   // requested_at: u64
        + 16; // deposit_yocto: u128
    MAP_ENTRY_OVERHEAD + key + val
}

pub fn yocto_for_bytes(bytes: u64) -> u128 {
    let price = env::storage_byte_cost().as_yoctonear();
    u128::from(bytes).saturating_mul(price)
}

pub fn yocto_for_new_market(market: &AccountId) -> u128 {
    yocto_for_bytes(
        storage_bytes_for_config_entry(market)
            .saturating_add(storage_bytes_for_market_supply_entry(market)),
    )
}

pub fn yocto_for_pending_cap(market: &AccountId) -> u128 {
    yocto_for_bytes(storage_bytes_for_pending_cap_entry(market))
}

pub fn yocto_for_queue_additions(current: &HashSet<AccountId>, new: &[AccountId]) -> u128 {
    new.iter().fold(0u128, |acc, id| {
        if current.contains(id) {
            acc
        } else {
            acc.saturating_add(yocto_for_bytes(storage_bytes_for_queue_item(id)))
        }
    })
}

pub fn require_attached_at_least(required_yocto: u128, ctx: &str) -> u128 {
    let attached = env::attached_deposit().as_yoctonear();
    assert!(
        attached >= required_yocto,
        "Insufficient storage deposit for {ctx}: required {required_yocto}, attached {attached}"
    );
    required_yocto
}

pub fn require_attached_for_bytes(bytes: u64, ctx: &str) -> u128 {
    let req = yocto_for_bytes(bytes);
    require_attached_at_least(req, ctx)
}

pub fn require_attached_for_pending_withdrawal(owner: &AccountId, receiver: &AccountId) -> u128 {
    let bytes = storage_bytes_for_pending_withdrawal(owner, receiver);
    require_attached_for_bytes(bytes, "withdrawal request")
}
