#![allow(clippy::needless_pass_by_value)]

use std::ops::{Deref, DerefMut};

use near_sdk::{env, near, AccountId, BorshStorageKey, PanicOnDefault};
use near_sdk_contract_tools::standard::nep145::{
    Nep145Controller, Nep145ForceUnregister, StorageBalanceBounds,
};
use templar_common::market::{Market, MarketConfiguration};

macro_rules! self_ext {
    ($gas:expr) => {
        Self::ext(::near_sdk::env::current_account_id()).with_static_gas($gas)
    };
}

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
enum StorageKey {
    Market,
}

#[derive(PanicOnDefault, near_sdk_contract_tools::Nep145)]
#[nep145(force_unregister_hook = "Self")]
#[near(contract_state)]
pub struct Contract {
    pub market: Market,
    storage_usage_snapshot: u64,
    storage_usage_supply_position: u64,
    storage_usage_borrow_position: u64,
}

#[near]
impl Contract {
    #[private]
    pub fn patch_configuration(&mut self, configuration: MarketConfiguration) {
        self.configuration = configuration;
    }

    #[allow(clippy::unwrap_used, reason = "Infallible")]
    #[init]
    pub fn new(configuration: MarketConfiguration) -> Self {
        let mut market = Market::new(StorageKey::Market, configuration);
        let storage_usage_1 = env::storage_usage();
        market.finalized_snapshots.flush();
        let storage_usage_2 = env::storage_usage();
        let storage_usage_snapshot = storage_usage_2.saturating_sub(storage_usage_1);

        // These values shoud be approximately:
        // 161 (fixed cost) +
        // borsh serialization length of position record +
        // 128 (max account length in bytes)

        drop(market.get_or_create_supply_position_guard("0".repeat(64).parse().unwrap()));
        let storage_usage_3 = env::storage_usage();
        let storage_usage_supply_position = storage_usage_3.saturating_sub(storage_usage_2);

        drop(market.get_or_create_borrow_position_guard("0".repeat(64).parse().unwrap()));
        let storage_usage_4 = env::storage_usage();
        let storage_usage_borrow_position = storage_usage_4.saturating_sub(storage_usage_3);

        env::log_str(&format!("Storage usage: {{ \"snapshot\": {storage_usage_snapshot}, \"supply_position\":{storage_usage_supply_position}, \"borrow_position\": {storage_usage_borrow_position} }}"));

        let mut self_ = Self {
            market,
            storage_usage_snapshot,
            storage_usage_supply_position,
            storage_usage_borrow_position,
        };

        self_.set_storage_balance_bounds(&StorageBalanceBounds {
            min: env::storage_byte_cost().saturating_mul(u128::from(
                storage_usage_supply_position.max(storage_usage_borrow_position)
                    + 2 * storage_usage_snapshot,
            )),
            max: None,
        });

        self_
    }

    fn charge_for_storage(&mut self, account_id: &AccountId, storage_consumption: u64) {
        self.lock_storage(
            account_id,
            env::storage_byte_cost().saturating_mul(u128::from(storage_consumption)),
        )
        .unwrap_or_else(|e| env::panic_str(&format!("Storage error: {e}")));
    }
}

impl near_sdk_contract_tools::hook::Hook<Self, Nep145ForceUnregister<'_>> for Contract {
    fn hook<R>(_: &mut Self, _: &Nep145ForceUnregister, _: impl FnOnce(&mut Self) -> R) -> R {
        env::panic_str("force unregistration is not supported")
    }
}

impl Deref for Contract {
    type Target = Market;

    fn deref(&self) -> &Self::Target {
        &self.market
    }
}

impl DerefMut for Contract {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.market
    }
}

mod impl_helper;
mod impl_market_external;
mod impl_token_receiver;

#[cfg(target_arch = "wasm32")]
mod custom_getrandom {
    #![allow(clippy::no_mangle_with_rust_abi)]

    use getrandom::{register_custom_getrandom, Error};
    use near_sdk::env;

    register_custom_getrandom!(custom_getrandom);

    #[allow(clippy::unnecessary_wraps)]
    pub fn custom_getrandom(buf: &mut [u8]) -> Result<(), Error> {
        buf.copy_from_slice(&env::random_seed_array());
        Ok(())
    }
}
