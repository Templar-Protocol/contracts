use near_sdk::{env, AccountId};
use std::vec::Vec;
use templar_common::vault::MarketId;
use templar_vault_kernel::{Address, TargetId};

/// Convert executor-facing identifiers into kernel `TargetId`.
pub trait IntoTargetId {
    fn into_target_id(self) -> TargetId;
}

impl IntoTargetId for MarketId {
    fn into_target_id(self) -> TargetId {
        u32::from(self)
    }
}

impl IntoTargetId for &MarketId {
    fn into_target_id(self) -> TargetId {
        u32::from(*self)
    }
}

impl IntoTargetId for TargetId {
    fn into_target_id(self) -> TargetId {
        self
    }
}

/// Convert kernel `TargetId` into executor `MarketId`.
pub trait IntoMarketId {
    fn into_market_id(self) -> MarketId;
}

impl IntoMarketId for TargetId {
    fn into_market_id(self) -> MarketId {
        MarketId::from(self)
    }
}

impl IntoMarketId for &TargetId {
    fn into_market_id(self) -> MarketId {
        MarketId::from(*self)
    }
}

const ADDRESS_DOMAIN: &[u8] = b"templar:near:account-id";

/// Deterministic one-way mapping for `AccountId` -> `Address`.
///
/// This keeps NEAR storage/API types unchanged (AccountId/U128/U64) while allowing
/// kernel logic (`Address`-based) to be applied. The mapping is *not reversible*,
/// so kernel effects that need `AccountId` must use executor context, not `Address`.
pub(crate) fn account_id_to_address(account: &AccountId) -> Address {
    let mut bytes = Vec::with_capacity(ADDRESS_DOMAIN.len() + account.as_bytes().len());
    bytes.extend_from_slice(ADDRESS_DOMAIN);
    bytes.extend_from_slice(account.as_bytes());
    Address(env::sha256_array(&bytes))
}
