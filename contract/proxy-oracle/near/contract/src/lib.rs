#![allow(clippy::needless_pass_by_value)]

use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};

use near_sdk::{
    env, json_types::Base64VecU8, near, require, AccountId, Gas, NearToken, PanicOnDefault,
    Promise, PromiseOrValue,
};
use near_sdk_contract_tools::{owner::Owner, Owner};
use templar_common::{
    oracle::{
        pyth::{ext_pyth, OracleResponse, PriceIdentifier},
        redstone::{self, ext_redstone},
    },
    self_ext,
    versioned_state::{impl_versioned_state, StateVersion, VersionedState},
    Decimal, Nanoseconds, UnwrapReject,
};
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{
        AcceptedHistorySource, CircuitBreaker, CircuitBreakerOutcome, CircuitBreakerSet,
        CircuitBreakerSetConfig,
    },
    Proxy,
};
use templar_proxy_oracle_near_common::{
    cache::{bounded_resolve_error_message, CachedProxyPrice, CachedProxyPriceStatus},
    convert::{account_id_to_kernel, pyth_price_try_from_kernel, pyth_price_try_to_kernel},
    event::{Event, MAX_MANUAL_TRIP_METADATA_LEN},
    governance::ProxyOracleAdminInterface,
    input::Source,
    request::OracleRequest,
    state,
};

mod callback_handler;
use callback_handler::{callback_result, CallbackHandler, OracleType};

type State = state::v1::State;

pub(crate) fn emit_outcome<T>(price_id: PriceIdentifier, outcome: CircuitBreakerOutcome<T>) -> T {
    for event in outcome.events {
        Event::from_kernel(price_id, event).emit();
    }
    outcome.value
}

#[derive(Debug, Owner, PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    pub state: VersionedState<State>,
}
impl_versioned_state!(Contract, State, state::migration::Migration);

impl Deref for Contract {
    type Target = State;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for Contract {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

#[near]
impl Contract {
    pub const GAS_FOR_PYTH_REQUEST: Gas = Gas::from_tgas(16).saturating_div(10);
    pub const GAS_FOR_REDSONE_REQUEST: Gas = Gas::from_tgas(17).saturating_div(10);
    pub const GAS_FOR_MIGRATE: Gas = Gas::from_tgas(250);

    #[init]
    pub fn new() -> Self {
        let mut self_ = Self {
            state: State::new(()),
        };

        let deployer = env::predecessor_account_id();

        Owner::init(&mut self_, &deployer);

        self_
    }

    // View methods

    pub fn list_proxies(&self, offset: Option<u32>, count: Option<u32>) -> Vec<PriceIdentifier> {
        self.state.list_proxies(offset, count)
    }

    pub fn get_proxy(&self, id: PriceIdentifier) -> Option<Proxy<Source>> {
        self.state.get_proxy(id)
    }

    pub fn get_proxy_circuit_breaker_set(&self, id: PriceIdentifier) -> Option<CircuitBreakerSet> {
        self.state.get_proxy_circuit_breaker_set(id)
    }

    pub fn get_cached_proxy_price(&self, id: PriceIdentifier) -> Option<CachedProxyPrice> {
        self.state.get_cached_proxy_price(id)
    }

    pub fn list_cached_proxy_prices(
        &self,
        price_ids: Vec<PriceIdentifier>,
    ) -> HashMap<PriceIdentifier, Option<CachedProxyPrice>> {
        self.state.list_cached_proxy_prices(price_ids)
    }

    // Pyth interface

    pub fn price_feed_exists(&self, price_identifier: PriceIdentifier) -> bool {
        self.state.proxy_exists(&price_identifier)
    }

    pub const GAS_FOR_LIST_00_ENTRY: Gas = Gas::from_tgas(35).saturating_div(10);
    pub fn list_ema_prices_no_older_than(
        &self,
        price_ids: Vec<PriceIdentifier>,
        age: u64,
    ) -> OracleResponse {
        if price_ids.is_empty() {
            return OracleResponse::new();
        }

        let max_age = Nanoseconds::from_secs(age);
        let now = Nanoseconds::near_timestamp();
        let mut results = OracleResponse::new();

        for price_id in HashSet::<PriceIdentifier>::from_iter(price_ids) {
            if !self.state.proxy_exists(&price_id) {
                continue;
            }

            let price = self
                .state
                .get_cached_proxy_price(price_id)
                .and_then(|cached| {
                    cached
                        .accepted_price_no_older_than(now, max_age)
                        .and_then(pyth_price_try_from_kernel)
                });
            results.insert(price_id, price);
        }

        results
    }

    pub fn update_prices(
        &self,
        price_ids: Vec<PriceIdentifier>,
    ) -> PromiseOrValue<HashMap<PriceIdentifier, CachedProxyPriceStatus>> {
        if price_ids.is_empty() {
            return PromiseOrValue::Value(HashMap::new());
        }
        let price_ids = HashSet::<PriceIdentifier>::from_iter(price_ids);

        let mut invoked = Vec::with_capacity(price_ids.len());
        let mut pyth_requests =
            HashMap::<AccountId, HashSet<PriceIdentifier>>::with_capacity(price_ids.len());
        let mut redstone_requests =
            HashMap::<AccountId, HashSet<redstone::FeedId>>::with_capacity(price_ids.len());
        let mut transformer_promises = Vec::with_capacity(price_ids.len());

        for price_id in &price_ids {
            let Some(proxy) = self.state.proxy_entry(*price_id) else {
                // Skip unknown.
                continue;
            };
            let pending = proxy.prepare_price_update();

            invoked.push(pending.clone());

            for source in pending.proxy.sources() {
                let request = match source {
                    Source::Request(request) => request,
                    Source::Transformer(transformer) => {
                        transformer_promises.push(transformer.call.promise());
                        &transformer.request
                    }
                };

                match request {
                    OracleRequest::Pyth(p) => {
                        pyth_requests
                            .entry(p.oracle_id.clone())
                            .or_default()
                            .insert(p.price_id);
                    }
                    OracleRequest::RedStone(p) => {
                        redstone_requests
                            .entry(p.oracle_id.clone())
                            .or_default()
                            .insert(p.price_id.clone());
                    }
                }
            }
        }

        let mut oracle_order = Vec::with_capacity(pyth_requests.len() + redstone_requests.len());
        let mut oracle_promises = Vec::with_capacity(pyth_requests.len() + redstone_requests.len());

        for (oracle_id, price_ids) in pyth_requests {
            oracle_order.push(OracleType::Pyth(oracle_id.clone()));
            oracle_promises.push(
                ext_pyth::ext(oracle_id)
                    .with_static_gas(Self::GAS_FOR_PYTH_REQUEST)
                    .list_ema_prices_unsafe(Vec::from_iter(price_ids)),
            );
        }

        for (oracle_id, price_ids) in redstone_requests {
            oracle_order.push(OracleType::RedStone(oracle_id.clone()));
            oracle_promises.push(
                ext_redstone::ext(oracle_id)
                    .with_static_gas(Self::GAS_FOR_REDSONE_REQUEST)
                    .read_price_data(Vec::from_iter(price_ids)),
            );
        }

        let Some(promise) = oracle_promises
            .into_iter()
            .chain(transformer_promises)
            .reduce(near_sdk::Promise::and)
        else {
            return PromiseOrValue::Value(HashMap::new());
        };

        PromiseOrValue::Promise(
            promise.then(
                self_ext!(Self::GAS_FOR_UPDATE_01_CALLBACK)
                    .update_prices_01_consume_results(oracle_order, invoked),
            ),
        )
    }

    pub const GAS_FOR_UPDATE_01_CALLBACK: Gas = Gas::from_tgas(10);
    #[private]
    #[allow(
        unused_mut,
        reason = "near macro expansion checks the original binding"
    )]
    pub fn update_prices_01_consume_results(
        &mut self,
        oracle_order: Vec<OracleType>,
        invoked: Vec<state::v1::PendingProxyPriceUpdate>,
    ) -> HashMap<PriceIdentifier, CachedProxyPriceStatus> {
        let callback = CallbackHandler::new(&oracle_order);

        let now = Nanoseconds::near_timestamp();
        let mut results = HashMap::new();

        let mut i = oracle_order.len() as u64;
        for pending in invoked {
            let price_id = pending.price_id;
            let mut prices = vec![];

            for source in pending.proxy.sources() {
                let source_result = match source {
                    Source::Transformer(transformer) => {
                        let price = callback.get(&transformer.request);
                        let input = callback_result::<Decimal>(i);
                        i += 1;

                        price
                            .zip(input)
                            .and_then(|(price, input)| transformer.action.apply(price, input))
                    }
                    Source::Request(request) => callback.get(request),
                };

                prices.push(source_result.as_ref().and_then(pyth_price_try_to_kernel));
            }

            if let Some(status) =
                self.state
                    .finish_price_update_if_current(pending, now, |proxy, set| {
                        match proxy.resolve(set, prices, now) {
                            Ok(resolution) => match emit_outcome(price_id, resolution) {
                                Ok(price) => CachedProxyPriceStatus::Accepted { price },
                                Err(reason) => CachedProxyPriceStatus::Blocked { reason },
                            },
                            Err(error) => {
                                let message = bounded_resolve_error_message(error.to_string());
                                near_sdk::log!(
                                    "Proxy resolve failed price_id={:?} error={}",
                                    price_id,
                                    message
                                );
                                CachedProxyPriceStatus::ResolveFailed { message }
                            }
                        }
                    })
            {
                results.insert(price_id, status);
            }
        }

        results
    }
}

#[near]
impl ProxyOracleAdminInterface for Contract {
    fn admin_set_proxy(&mut self, id: PriceIdentifier, proxy: Option<Proxy<Source>>) {
        self.assert_owner();
        self.state.set_proxy(id, proxy);
    }

    fn admin_configure_circuit_breakers(
        &mut self,
        id: PriceIdentifier,
        config: CircuitBreakerSetConfig,
    ) {
        self.assert_owner();
        let result = self
            .state
            .proxy_entry_mut(id)
            .unwrap_or_else(|| env::panic_str("Proxy not found"))
            .configure_circuit_breakers(config);
        emit_outcome(id, result);
    }

    fn admin_add_circuit_breaker(
        &mut self,
        id: PriceIdentifier,
        breaker_id: u32,
        breaker: CircuitBreaker,
    ) {
        self.assert_owner();
        let result = self
            .state
            .proxy_entry_mut(id)
            .unwrap_or_else(|| env::panic_str("Proxy not found"))
            .add_circuit_breaker(breaker_id, breaker)
            .unwrap_or_reject();
        emit_outcome(id, result);
    }

    fn admin_remove_circuit_breaker(&mut self, id: PriceIdentifier, breaker_id: u32) {
        self.assert_owner();
        let result = self
            .state
            .proxy_entry_mut(id)
            .unwrap_or_else(|| env::panic_str("Proxy not found"))
            .remove_circuit_breaker(breaker_id)
            .unwrap_or_reject();
        emit_outcome(id, result);
    }

    fn admin_set_manual_trip(
        &mut self,
        id: PriceIdentifier,
        is_manually_tripped: bool,
        metadata: Option<Base64VecU8>,
    ) {
        self.assert_owner();

        require!(
            metadata
                .as_ref()
                .is_none_or(|metadata| metadata.0.len() <= MAX_MANUAL_TRIP_METADATA_LEN),
            "Manual trip metadata is too long"
        );
        let result = self
            .state
            .proxy_entry_mut(id)
            .unwrap_or_else(|| env::panic_str("Proxy not found"))
            .set_circuit_breaker_manual_trip(
                is_manually_tripped,
                account_id_to_kernel(env::predecessor_account_id().as_ref()),
                metadata.map(|metadata| metadata.0),
            );
        if result.events.is_empty() {
            return;
        }

        emit_outcome(id, result);
    }

    fn admin_rearm(
        &mut self,
        id: PriceIdentifier,
        breaker_id: u32,
        armed_after_ns: Nanoseconds,
        accepted_history_source: AcceptedHistorySource,
    ) {
        self.assert_owner();

        let result = self
            .state
            .proxy_entry_mut(id)
            .unwrap_or_else(|| env::panic_str("Proxy not found"))
            .rearm(breaker_id, armed_after_ns, accepted_history_source)
            .unwrap_or_reject();
        emit_outcome(id, result);
    }

    fn admin_set_enforced(&mut self, id: PriceIdentifier, breaker_id: u32, is_enforced: bool) {
        self.assert_owner();

        let result = self
            .state
            .proxy_entry_mut(id)
            .unwrap_or_else(|| env::panic_str("Proxy not found"))
            .set_enforced(breaker_id, is_enforced)
            .unwrap_or_reject();
        emit_outcome(id, result);
    }

    fn admin_upgrade(&mut self, code: Base64VecU8, migrate_args: Base64VecU8) -> Promise {
        self.assert_owner();
        Promise::new(env::current_account_id())
            .deploy_contract(code.0)
            .function_call(
                "migrate".to_string(),
                migrate_args.0,
                NearToken::from_yoctonear(0),
                Self::GAS_FOR_MIGRATE,
            )
    }
}

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
