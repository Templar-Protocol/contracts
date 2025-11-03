use near_sdk::{env, require, AccountId};
use std::collections::HashSet;
use templar_common::vault::{storage_bytes_for_account_id, PendingWithdrawal};

/// Set of hacks because near-sdk does not support borshschema and its overkill to implement
/// We do not implement refunds for storage management ops, to avoid any potential issues with
/// accounting.

/// Conservative per-entry overheads to cover collection metadata, prefixes, etc.
pub const MAP_ENTRY_OVERHEAD: u64 = 64;

pub const VEC_ITEM_OVERHEAD: u64 = 16;
pub const U128_BYTES: u64 = 16;
pub const U64_BYTES: u64 = 8;
pub const OPTION_TAG_BYTES: u64 = 1;
#[must_use]
pub fn storage_bytes_for_queue_account_id() -> u64 {
    VEC_ITEM_OVERHEAD + storage_bytes_for_account_id()
}

#[must_use]
pub fn storage_bytes_for_ft_account_entry() -> u64 {
    let key = storage_bytes_for_account_id();
    let val = U128_BYTES; // balance: u128
    MAP_ENTRY_OVERHEAD + key + val
}

#[must_use]
pub fn yocto_for_ft_account() -> u128 {
    yocto_for_bytes(storage_bytes_for_ft_account_entry())
}





#[must_use]
pub fn storage_bytes_for_pending_withdrawal() -> u64 {
    // Key is u64 id -> 8 bytes
    let key = 8u64;
    let val = PendingWithdrawal::encoded_size() as u64;
    MAP_ENTRY_OVERHEAD + key + val
}

#[must_use]
pub fn yocto_for_bytes(bytes: u64) -> u128 {
    let price = env::storage_byte_cost().as_yoctonear();
    u128::from(bytes).saturating_mul(price)
}



#[must_use]
pub fn yocto_for_queue_additions(current: &HashSet<AccountId>, new: &[AccountId]) -> u128 {
    new.iter().fold(0u128, |acc, id| {
        if current.contains(id) {
            acc
        } else {
            acc.saturating_add(yocto_for_bytes(storage_bytes_for_queue_account_id()))
        }
    })
}

#[must_use]
pub fn require_attached_at_least(required_yocto: u128, ctx: &str) -> u128 {
    let attached = env::attached_deposit().as_yoctonear();
    require!(
        attached >= required_yocto,
        format!("Insufficient storage deposit for {ctx}: required {required_yocto}, attached {attached}")
    );
    required_yocto
}

#[must_use]
pub fn require_attached_for_bytes(bytes: u64, ctx: &str) -> u128 {
    let req = yocto_for_bytes(bytes);
    require_attached_at_least(req, ctx)
}

#[must_use]
pub fn require_attached_for_state_delta<R>(ctx: &str, mutate: impl FnOnce() -> R) -> R {
    let before = env::storage_usage();
    let out = mutate();
    let after = env::storage_usage();
    let delta = after.saturating_sub(before);
    if delta > 0 {
        let yocto = yocto_for_bytes(delta);
        require_attached_at_least(yocto, ctx);
    }
    out
}

#[must_use]
pub fn require_attached_for_pending_withdrawal() -> u128 {
    let bytes = storage_bytes_for_pending_withdrawal();
    require_attached_for_bytes(bytes, "withdrawal request")
}
