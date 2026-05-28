use near_sdk::{ext_contract, json_types::Base64VecU8, Promise};
use templar_common::{oracle::pyth::PriceIdentifier, Nanoseconds};
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{AcceptedHistorySource, CircuitBreaker, CircuitBreakerSetConfig},
    Proxy,
};

pub const MAX_CIRCUIT_BREAKER_HISTORY_LEN: u32 = 32;
pub const MAX_CIRCUIT_BREAKERS_PER_PROXY: usize = 16;

#[ext_contract(ext_proxy_oracle_admin)]
pub trait ProxyOracleAdminInterface {
    fn admin_set_proxy(&mut self, id: PriceIdentifier, proxy: Option<Proxy<crate::input::Source>>);
    fn admin_configure_circuit_breakers(
        &mut self,
        id: PriceIdentifier,
        config: CircuitBreakerSetConfig,
    );
    fn admin_add_circuit_breaker(
        &mut self,
        id: PriceIdentifier,
        breaker_id: u32,
        breaker: CircuitBreaker,
    );
    fn admin_remove_circuit_breaker(&mut self, id: PriceIdentifier, breaker_id: u32);
    fn admin_set_manual_trip(
        &mut self,
        id: PriceIdentifier,
        is_manually_tripped: bool,
        metadata: Option<Base64VecU8>,
    );
    fn admin_rearm(
        &mut self,
        id: PriceIdentifier,
        breaker_id: u32,
        armed_after_ns: Nanoseconds,
        accepted_history_source: AcceptedHistorySource,
    );
    fn admin_set_enforced(&mut self, id: PriceIdentifier, breaker_id: u32, is_enforced: bool);
    fn admin_upgrade(&mut self, code: Base64VecU8, migrate_args: Base64VecU8) -> Promise;
}
