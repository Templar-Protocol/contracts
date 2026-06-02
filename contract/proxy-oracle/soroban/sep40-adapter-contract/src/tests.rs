#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::should_panic_without_expect)]

extern crate std;

use super::*;

use soroban_sdk::{
    testutils::{Address as _, Ledger, LedgerInfo},
    Address, Env, Symbol,
};
use templar_proxy_oracle_soroban_common::{Asset as CommonAsset, NormalizedPrice};

// Mock parent oracle that returns a fixed normalized price for a fixed asset.
mod mock_parent {
    use soroban_sdk::{contract, contractimpl, contracttype, Env, Vec};
    use templar_proxy_oracle_soroban_common::{Asset, NormalizedPrice};

    #[derive(Clone)]
    #[contracttype]
    enum Key {
        Price(Asset),
        History(Asset),
    }

    #[contract]
    pub struct MockParent;

    #[contractimpl]
    impl MockParent {
        pub fn set_aggregated(env: Env, asset: Asset, price: NormalizedPrice) {
            env.storage().persistent().set(&Key::Price(asset), &price);
        }

        pub fn set_history(env: Env, asset: Asset, history: Vec<NormalizedPrice>) {
            env.storage()
                .persistent()
                .set(&Key::History(asset), &history);
        }
    }

    #[contractimpl]
    impl templar_proxy_oracle_soroban_common::ProxyOracleTrait for MockParent {
        fn aggregated_latest(env: Env, asset: Asset) -> Option<NormalizedPrice> {
            env.storage().persistent().get(&Key::Price(asset))
        }

        fn aggregated_history(
            env: Env,
            asset: Asset,
            records: u32,
        ) -> Option<Vec<NormalizedPrice>> {
            if records == 0 {
                return None;
            }
            let history: Vec<NormalizedPrice> =
                env.storage().persistent().get(&Key::History(asset))?;
            if history.is_empty() {
                return None;
            }
            let start = history.len().saturating_sub(records);
            Some(history.slice(start..))
        }
    }
}
use mock_parent::{MockParent, MockParentClient};

fn ledger(env: &Env, timestamp: u64) {
    env.ledger().set(LedgerInfo {
        timestamp,
        protocol_version: 25,
        sequence_number: 100,
        max_entry_ttl: 10_000,
        ..Default::default()
    });
}

struct Fixture {
    env: Env,
    owner: Address,
    parent_id: Address,
    parent: MockParentClient<'static>,
    adapter: Sep40AdapterClient<'static>,
    asset: CommonAsset,
    base: CommonAsset,
}

fn fixture(decimals: u32, resolution: u32) -> Fixture {
    let env = Env::default();
    env.mock_all_auths();
    ledger(&env, 100);

    let owner = Address::generate(&env);
    let asset = CommonAsset::Other(Symbol::new(&env, "BTC"));
    let base = CommonAsset::Other(Symbol::new(&env, "USD"));

    let parent_id = env.register(MockParent, ());
    let parent = MockParentClient::new(&env, &parent_id);

    let adapter_id = env.register(
        Sep40Adapter,
        (&owner, &parent_id, &asset, &decimals, &resolution, &base),
    );
    let adapter = Sep40AdapterClient::new(&env, &adapter_id);

    Fixture {
        env,
        owner,
        parent_id,
        parent,
        adapter,
        asset,
        base,
    }
}

#[test]
fn constructor_persists_fields_and_owner() {
    let f = fixture(8, 1);
    assert_eq!(f.adapter.decimals(), 8);
    assert_eq!(f.adapter.resolution(), 1);
    assert_eq!(f.adapter.base(), f.base);
    assert_eq!(f.adapter.assets().len(), 1);
    assert_eq!(f.adapter.assets().get(0).unwrap(), f.asset);
    assert_eq!(f.adapter.parent_oracle(), Some(f.parent_id));
    assert_eq!(f.adapter.price_asset(), Some(f.asset));
    assert_eq!(f.adapter.get_owner(), Some(f.owner));
}

#[test]
#[should_panic]
fn constructor_rejects_decimals_above_18() {
    let _ = fixture(19, 1);
}

#[test]
#[should_panic]
fn constructor_rejects_zero_resolution() {
    let _ = fixture(8, 0);
}

#[test]
fn lastprice_scales_normalized_to_adapter_decimals() {
    let f = fixture(8, 1);
    // Parent reports the price in its normalized form (mantissa+expo).
    // 50.00 USD with expo=-4 ↔ mantissa=500_000.
    f.parent.set_aggregated(
        &f.asset,
        &NormalizedPrice {
            mantissa: 500_000,
            expo: -4,
            timestamp: 100,
        },
    );
    let p = f.adapter.lastprice(&f.asset).unwrap();
    // Adapter decimals=8 → scaled = 500_000 * 10^(8-4) = 500_000 * 10_000 = 5_000_000_000.
    assert_eq!(p.price, 5_000_000_000);
    assert_eq!(p.timestamp, 100);
}

#[test]
fn lastprice_scales_to_smaller_decimals() {
    let f = fixture(2, 1);
    // Parent reports 50.00 at expo=-8 (mantissa=5_000_000_000).
    f.parent.set_aggregated(
        &f.asset,
        &NormalizedPrice {
            mantissa: 5_000_000_000,
            expo: -8,
            timestamp: 100,
        },
    );
    let p = f.adapter.lastprice(&f.asset).unwrap();
    // Adapter decimals=2 → scale = 2 + (-8) = -6 → 5_000_000_000 / 10^6 = 5_000.
    assert_eq!(p.price, 5_000);
}

#[test]
fn lastprice_unknown_asset_returns_none() {
    let f = fixture(8, 1);
    let other = CommonAsset::Other(Symbol::new(&f.env, "ETH"));
    assert_eq!(f.adapter.lastprice(&other), None);
}

#[test]
fn lastprice_missing_parent_data_returns_none() {
    let f = fixture(8, 1);
    assert_eq!(f.adapter.lastprice(&f.asset), None);
}

#[test]
fn prices_returns_scaled_history() {
    let f = fixture(8, 1);
    let mut h = soroban_sdk::Vec::new(&f.env);
    h.push_back(NormalizedPrice {
        mantissa: 100,
        expo: -2,
        timestamp: 50,
    });
    h.push_back(NormalizedPrice {
        mantissa: 200,
        expo: -2,
        timestamp: 60,
    });
    f.parent.set_history(&f.asset, &h);

    let prices = f.adapter.prices(&f.asset, &2).unwrap();
    assert_eq!(prices.len(), 2);
    // 100 with expo=-2 at decimals=8 → 100 * 10^6 = 100_000_000.
    assert_eq!(prices.get(0).unwrap().price, 100_000_000);
    assert_eq!(prices.get(1).unwrap().price, 200_000_000);
}

#[test]
fn price_finds_matching_timestamp() {
    let f = fixture(8, 1);
    let mut h = soroban_sdk::Vec::new(&f.env);
    h.push_back(NormalizedPrice {
        mantissa: 100,
        expo: -2,
        timestamp: 50,
    });
    h.push_back(NormalizedPrice {
        mantissa: 200,
        expo: -2,
        timestamp: 60,
    });
    f.parent.set_history(&f.asset, &h);

    assert_eq!(f.adapter.price(&f.asset, &60).unwrap().price, 200_000_000);
    assert_eq!(f.adapter.price(&f.asset, &50).unwrap().price, 100_000_000);
    assert_eq!(f.adapter.price(&f.asset, &99), None);
}

#[test]
fn set_decimals_owner_gated_changes_scaling() {
    let f = fixture(8, 1);
    f.parent.set_aggregated(
        &f.asset,
        &NormalizedPrice {
            mantissa: 500_000,
            expo: -4,
            timestamp: 100,
        },
    );
    assert_eq!(f.adapter.lastprice(&f.asset).unwrap().price, 5_000_000_000);

    // Owner can change decimals; auth is mocked.
    f.adapter.set_decimals(&4);
    assert_eq!(f.adapter.decimals(), 4);
    // Now scaling: 500_000 with expo=-4 at decimals=4 → 500_000 * 10^0 = 500_000.
    assert_eq!(f.adapter.lastprice(&f.asset).unwrap().price, 500_000);
}

#[test]
#[should_panic]
fn set_decimals_rejects_above_18() {
    let f = fixture(8, 1);
    f.adapter.set_decimals(&19);
}

#[test]
#[should_panic]
fn set_resolution_rejects_zero() {
    let f = fixture(8, 1);
    f.adapter.set_resolution(&0);
}

#[test]
fn set_base_owner_gated_persists() {
    let f = fixture(8, 1);
    let new_base = CommonAsset::Other(Symbol::new(&f.env, "EUR"));
    f.adapter.set_base(&new_base);
    assert_eq!(f.adapter.base(), new_base);
}

#[test]
fn upgrade_rejects_zero_wasm_hash() {
    let f = fixture(8, 1);
    let zero = soroban_sdk::BytesN::from_array(&f.env, &[0; 32]);
    let res = f.adapter.try_upgrade(&zero);
    assert!(res.is_err());
}

#[test]
fn two_step_ownership_transfer_round_trip() {
    let f = fixture(8, 1);
    let new_owner = Address::generate(&f.env);
    // Initiate transfer; live_until_ledger=1000 (well beyond current ledger).
    f.adapter.transfer_ownership(&new_owner, &1000_u32);
    // Owner hasn't changed until acceptance.
    assert_eq!(f.adapter.get_owner(), Some(f.owner));
    f.adapter.accept_ownership();
    assert_eq!(f.adapter.get_owner(), Some(new_owner));
}
