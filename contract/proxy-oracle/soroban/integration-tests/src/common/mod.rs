//! Shared harness for the proxy-oracle integration test suite.
//!
//! [`Bootstrap`] deploys the runtime, governance, SEP-40 adapter, and one
//! mock upstream oracle into a single in-process `Env` and re-wires
//! ownership so governance is the runtime's owner. Most scenarios start from
//! `Bootstrap::new()` and layer on configuration + role grants from there.

pub mod ledger;
pub mod mock_oracle;

use soroban_sdk::testutils::{Address as _, Ledger as _, LedgerInfo};
use soroban_sdk::{Address, Env, Symbol};
use templar_proxy_oracle_soroban_common::{Asset, ProxyConfig, SourceConfig};
use templar_proxy_oracle_soroban_contract::{
    RefreshStatus, SorobanProxyOracle, SorobanProxyOracleClient,
};
use templar_proxy_oracle_soroban_governance_common::{GovernanceAction, Role};
use templar_proxy_oracle_soroban_governance_contract::{
    ProxyOracleGovernance, ProxyOracleGovernanceClient,
};
use templar_proxy_oracle_soroban_sep40_adapter_contract::{Sep40Adapter, Sep40AdapterClient};

pub use mock_oracle::{MockOracle, MockOracleClient};

/// Default per-asset adapter decimals; matches the runtime's default scaling
/// in most happy-path tests.
pub const ADAPTER_DECIMALS: u32 = 8;
/// Default SEP-40 resolution.
pub const ADAPTER_RESOLUTION: u32 = 1;

/// One-stop fixture: three deployed contracts + one mock upstream oracle +
/// the symbolic Assets we use in tests.
pub struct Bootstrap {
    pub env: Env,
    /// Initial admin of the governance contract (holds `Role::Admin`).
    pub admin: Address,
    pub runtime_id: Address,
    pub runtime: SorobanProxyOracleClient<'static>,
    pub governance_id: Address,
    pub governance: ProxyOracleGovernanceClient<'static>,
    pub adapter_id: Address,
    pub adapter: Sep40AdapterClient<'static>,
    pub upstream_id: Address,
    pub upstream: MockOracleClient<'static>,
    pub asset_btc: Asset,
    pub base_usd: Asset,
}

impl Bootstrap {
    /// Default fixture: TTL = 0 so proposals can execute immediately. Suitable
    /// for the bulk of scenarios; the lifecycle tests override.
    pub fn new() -> Self {
        Self::with_initial_ttl(0)
    }

    pub fn with_initial_ttl(initial_uniform_ttl_ns: u64) -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 100,
            protocol_version: 25,
            sequence_number: 100,
            min_temp_entry_ttl: 16,
            min_persistent_entry_ttl: 4096,
            max_entry_ttl: 6_312_000,
            ..Default::default()
        });

        let admin = Address::generate(&env);
        let base_usd = Asset::Other(Symbol::new(&env, "USD"));
        let asset_btc = Asset::Other(Symbol::new(&env, "BTC"));

        // Deploy runtime with admin as the initial owner so we can hand off
        // to governance once the governance contract address exists.
        let runtime_id = env.register(SorobanProxyOracle, (&admin, &base_usd));
        let runtime = SorobanProxyOracleClient::new(&env, &runtime_id);

        let governance_id = env.register(
            ProxyOracleGovernance,
            (&admin, &runtime_id, initial_uniform_ttl_ns),
        );
        let governance = ProxyOracleGovernanceClient::new(&env, &governance_id);

        let live_until_ledger = env.ledger().max_live_until_ledger();
        runtime.transfer_ownership(&governance_id, &live_until_ledger);
        runtime.accept_ownership();

        // Mock upstream oracle (one source feed).
        let upstream_id = env.register(
            MockOracle,
            (&base_usd, &ADAPTER_DECIMALS, &ADAPTER_RESOLUTION),
        );
        let upstream = MockOracleClient::new(&env, &upstream_id);

        // SEP-40 adapter: owned by `admin` directly. Adapter mutations
        // (`set_metadata`, `upgrade`) aren't routed through the governance
        // proposal flow because they don't move oracle state; making the
        // bootstrap admin the adapter owner reflects that and keeps test
        // invocations straightforward.
        let adapter_id = env.register(
            Sep40Adapter,
            (
                &admin,
                &runtime_id,
                &asset_btc,
                &ADAPTER_DECIMALS,
                &ADAPTER_RESOLUTION,
                &base_usd,
            ),
        );
        let adapter = Sep40AdapterClient::new(&env, &adapter_id);

        Self {
            env,
            admin,
            runtime_id,
            runtime,
            governance_id,
            governance,
            adapter_id,
            adapter,
            upstream_id,
            upstream,
            asset_btc,
            base_usd,
        }
    }

    /// Submit + execute a proposal as the given caller. Returns the proposal id.
    pub fn submit_and_execute(&self, caller: &Address, action: GovernanceAction) -> u64 {
        let id = self.governance.next_proposal_id();
        self.governance.create_proposal(caller, &id, &action, &0);
        self.governance.execute_proposal(caller, &id);
        id
    }

    /// Convenience for the common pattern of registering BTC/USD with one
    /// upstream source.
    pub fn configure_default_feed(&self) {
        let mut sources = soroban_sdk::Vec::new(&self.env);
        sources.push_back(SourceConfig {
            oracle: self.upstream_id.clone(),
            asset: self.asset_btc.clone(),
        });
        let config = ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: Some(300),
            max_clock_drift_secs: Some(60),
        };
        self.submit_and_execute(
            &self.admin,
            GovernanceAction::SetProxy(self.asset_btc.clone(), config),
        );
    }

    /// Grant a role via an admin proposal.
    pub fn grant_role(&self, who: &Address, role: Role) {
        self.submit_and_execute(
            &self.admin,
            GovernanceAction::SetRole(who.clone(), role, true),
        );
    }

    /// Drive the mock upstream with an explicit (price, ts).
    pub fn push_upstream_price(&self, asset: &Asset, price: i128, timestamp: u64) {
        self.upstream.set_price(asset, &price, &timestamp);
    }

    /// One-asset refresh helper. Panics if the runtime returned no result
    /// for the asked-for asset — only possible if the contract changed shape.
    #[allow(clippy::expect_used)]
    pub fn refresh_one(&self, asset: &Asset) -> RefreshStatus {
        let assets = soroban_sdk::Vec::from_array(&self.env, [asset.clone()]);
        let results = self.runtime.refresh(&assets);
        results
            .get(0)
            .expect("runtime returned no entry for refreshed asset")
            .1
    }
}

impl Default for Bootstrap {
    fn default() -> Self {
        Self::new()
    }
}
