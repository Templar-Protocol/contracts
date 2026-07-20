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
pub fn account_id_to_address(account: &AccountId) -> Address {
    let mut bytes = Vec::with_capacity(ADDRESS_DOMAIN.len() + account.as_bytes().len());
    bytes.extend_from_slice(ADDRESS_DOMAIN);
    bytes.extend_from_slice(account.as_bytes());
    Address(env::sha256_array(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known-answer guard against a silent change to the domain separator or the
    /// hashing itself. The expected value is `sha256("templar:near:account-id" ||
    /// "alice.near")`, computed independently of this function — so a regression
    /// here is caught even by callers that derive their expectation *through*
    /// `account_id_to_address` (e.g. the vault blacklist test helper).
    #[test]
    fn account_id_to_address_is_stable() {
        let account: AccountId = "alice.near".parse().unwrap();
        assert_eq!(
            account_id_to_address(&account).0,
            [
                0xc6, 0x76, 0x58, 0x20, 0x30, 0xde, 0x5f, 0x2c, 0x4c, 0xb3, 0x42, 0x98, 0xa2, 0xf4,
                0x0f, 0x0e, 0xd0, 0xee, 0x8e, 0xca, 0x98, 0x3e, 0xd6, 0xd1, 0xc7, 0x43, 0xac, 0x39,
                0x95, 0xad, 0xa7, 0x0e,
            ],
        );
    }
}
