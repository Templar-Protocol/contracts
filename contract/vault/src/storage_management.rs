use crate::PendingWithdrawal;
use crate::{env, AccountId, Contract, Nep145Controller, Nep145ForceUnregister};
use std::collections::HashSet;
use templar_common::vault::{storage_bytes_for_account_id, MarketConfiguration};

/// Set of hacks because near-sdk does not support borshschema and its overkill to implement
/// We do not implement refunds for storage management ops, to avoid any potential issues with
/// accounting.

// Conservative per-entry overheads to cover collection metadata, prefixes, etc.
pub const MAP_ENTRY_OVERHEAD: u64 = 64;

pub const VEC_ITEM_OVERHEAD: u64 = 16;
#[must_use]
pub fn storage_bytes_for_queue_account_id() -> u64 {
    VEC_ITEM_OVERHEAD + storage_bytes_for_account_id()
}

#[must_use]
pub fn storage_bytes_for_config_entry() -> u64 {
    let key = storage_bytes_for_account_id();
    MAP_ENTRY_OVERHEAD + key + MarketConfiguration::encoded_size() as u64
}

#[must_use]
pub fn storage_bytes_for_market_supply_entry() -> u64 {
    let key = storage_bytes_for_account_id();
    // u128 principal
    let val = 16u64;
    MAP_ENTRY_OVERHEAD + key + val
}

#[must_use]
pub fn storage_bytes_for_pending_cap_entry() -> u64 {
    let key = storage_bytes_for_account_id();
    // PendingValue { value: u128, valid_at: u64 }
    let val = 16u64 + 8u64;
    MAP_ENTRY_OVERHEAD + key + val
}

#[must_use]
pub fn storage_bytes_for_pending_withdrawal() -> u64 {
    // Key is u64 id -> 8 bytes
    let key = 8u64;
    let val = PendingWithdrawal::encoded_size();
    MAP_ENTRY_OVERHEAD + key + val
}

#[must_use]
pub fn yocto_for_bytes(bytes: u64) -> u128 {
    let price = env::storage_byte_cost().as_yoctonear();
    u128::from(bytes).saturating_mul(price)
}

#[must_use]
pub fn yocto_for_new_market() -> u128 {
    yocto_for_bytes(
        storage_bytes_for_config_entry().saturating_add(storage_bytes_for_market_supply_entry()),
    )
}

#[must_use]
pub fn yocto_for_pending_cap() -> u128 {
    yocto_for_bytes(storage_bytes_for_pending_cap_entry())
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
    assert!(
        attached >= required_yocto,
        "Insufficient storage deposit for {ctx}: required {required_yocto}, attached {attached}"
    );
    required_yocto
}

#[must_use]
pub fn require_attached_for_bytes(bytes: u64, ctx: &str) -> u128 {
    let req = yocto_for_bytes(bytes);
    require_attached_at_least(req, ctx)
}

#[must_use]
pub fn require_attached_for_pending_withdrawal() -> u128 {
    let bytes = storage_bytes_for_pending_withdrawal();
    require_attached_for_bytes(bytes, "withdrawal request")
}

impl Contract {
    /* ----- Storage ----- */
    pub(crate) fn charge_for_storage(&mut self, account_id: &AccountId, storage_consumption: u64) {
        // Invariant: Storage charging saturates and panics on failure to avoid negative balances.
        self.lock_storage(
            account_id,
            env::storage_byte_cost().saturating_mul(u128::from(storage_consumption)),
        )
        .unwrap_or_else(|e| env::panic_str(&format!("Storage error: {e}")));
    }

    pub(crate) fn refund_for_storage(&mut self, account_id: &AccountId, storage_consumption: u64) {
        // Invariant: Storage refunds saturate and panic on failure to preserve accounting integrity.
        self.unlock_storage(
            account_id,
            env::storage_byte_cost().saturating_mul(u128::from(storage_consumption)),
        )
        .unwrap_or_else(|e| env::panic_str(&format!("Storage error: {e}")));
    }
}
impl near_sdk_contract_tools::hook::Hook<Self, Nep145ForceUnregister<'_>> for Contract {
    fn hook<R>(_: &mut Self, _: &Nep145ForceUnregister, _: impl FnOnce(&mut Self) -> R) -> R {
        // Invariant: Force unregister must fail to preserve FT ledger integrity.
        env::panic_str("force unregistration is not supported")
    }
}
