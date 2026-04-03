use crate::auth::{ActionKind, AuthAdapter};
use crate::contract::helpers::transition_to_runtime;
use crate::contract::CuratorVault;
use crate::effects::{AddressRegistrar, EffectInterpreter, EffectSummary};
use crate::error::RuntimeError;
use crate::storage::{
    compose_policy_state, decode_markets, decode_policy_locks, decode_principals,
    decode_restrictions, decode_state_blob, decode_supply_queue, encode_markets,
    encode_policy_locks, encode_principals, encode_restrictions, encode_state_blob,
    encode_supply_queue, Storage, StorageVersion, VersionedState,
};
use alloc::vec::Vec;
use core::mem;
use soroban_sdk::{Address as SdkAddress, Bytes, Env};
use templar_curator_primitives::policy::cap_group::{CapGroupId, CapGroupRecord};
use templar_curator_primitives::policy::market_lock::MarketLeaseRegistry;
use templar_curator_primitives::policy::state::{MarketConfig, OrderedMap};
use templar_curator_primitives::policy::supply_queue::SupplyQueue;
use templar_curator_primitives::PolicyState;
use templar_vault_kernel::state::op_state::AllocationPlanEntry;
use templar_vault_kernel::{
    complete_allocation, start_allocation, Address, AddressBook, AssetId, Restrictions, TargetId,
    TimestampNs,
};

pub type AttemptId = u64;

pub mod fuzz_api {
    use super::*;

    pub fn encode_restrictions_bytes(value: &Restrictions) -> Vec<u8> {
        encode_restrictions(value)
    }

    pub fn decode_restrictions_bytes(bytes: &[u8]) -> Result<Restrictions, RuntimeError> {
        decode_restrictions(bytes)
    }

    pub fn encode_supply_queue_bytes(value: &SupplyQueue) -> Vec<u8> {
        encode_supply_queue(value)
    }

    pub fn decode_supply_queue_bytes(bytes: &[u8]) -> Result<SupplyQueue, RuntimeError> {
        decode_supply_queue(bytes)
    }

    pub fn encode_markets_bytes(value: &OrderedMap<TargetId, MarketConfig>) -> Vec<u8> {
        encode_markets(value)
    }

    pub fn decode_markets_bytes(
        bytes: &[u8],
    ) -> Result<OrderedMap<TargetId, MarketConfig>, RuntimeError> {
        decode_markets(bytes)
    }

    pub fn encode_principals_bytes(value: &OrderedMap<TargetId, u128>) -> Vec<u8> {
        encode_principals(value)
    }

    pub fn decode_principals_bytes(
        bytes: &[u8],
    ) -> Result<OrderedMap<TargetId, u128>, RuntimeError> {
        decode_principals(bytes)
    }

    pub fn encode_policy_locks_bytes(value: &MarketLeaseRegistry) -> Vec<u8> {
        encode_policy_locks(value)
    }

    pub fn decode_policy_locks_bytes(bytes: &[u8]) -> Result<MarketLeaseRegistry, RuntimeError> {
        decode_policy_locks(bytes)
    }

    pub fn encode_state_blob_bytes(value: &VersionedState) -> Vec<u8> {
        encode_state_blob(value)
    }

    pub fn decode_state_blob_bytes(bytes: &[u8]) -> Result<VersionedState, RuntimeError> {
        decode_state_blob(bytes)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct SettlementReceipt {
    pub op_id: u64,
    pub attempt_id: u64,
    pub new_external_assets: i128,
}

impl SettlementReceipt {
    #[inline]
    #[must_use]
    pub const fn new(op_id: u64, attempt_id: u64, new_external_assets: i128) -> Self {
        Self {
            op_id,
            attempt_id,
            new_external_assets,
        }
    }
}

pub trait SorobanCrossChainMarketAdapter {
    fn submit_intent(&self, env: &Env, plan_bytes: Bytes) -> Result<u64, RuntimeError>;

    fn settle(
        &self,
        env: &Env,
        op_id: u64,
        attempt_id: u64,
    ) -> Result<SettlementReceipt, RuntimeError>;

    fn total_assets(&self, env: &Env, asset: &SdkAddress) -> Result<i128, RuntimeError>;
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct MarketRef {
    pub market_id: TargetId,
    pub asset_id: AssetId,
}

impl MarketRef {
    #[inline]
    #[must_use]
    pub const fn new(market_id: TargetId, asset_id: AssetId) -> Self {
        Self {
            market_id,
            asset_id,
        }
    }
}

impl From<(TargetId, AssetId)> for MarketRef {
    fn from(value: (TargetId, AssetId)) -> Self {
        Self::new(value.0, value.1)
    }
}

impl From<MarketRef> for (TargetId, AssetId) {
    fn from(value: MarketRef) -> Self {
        (value.market_id, value.asset_id)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Default)]
pub struct MemoryStorage {
    state: Option<VersionedState>,
    initialized: bool,
    paused: bool,
    policy_locks: Option<MarketLeaseRegistry>,
    policy_supply_queue: Option<SupplyQueue>,
    policy_markets: Option<OrderedMap<TargetId, MarketConfig>>,
    policy_principals: Option<OrderedMap<TargetId, u128>>,
    policy_cap_groups: Option<OrderedMap<CapGroupId, CapGroupRecord>>,
    restrictions: Option<Restrictions>,
    address_book: AddressBook<SdkAddress>,
}

impl MemoryStorage {
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    #[must_use]
    pub fn with_state(state: VersionedState) -> Self {
        Self {
            state: Some(state),
            initialized: true,
            paused: false,
            policy_locks: None,
            policy_supply_queue: None,
            policy_markets: None,
            policy_principals: None,
            policy_cap_groups: None,
            restrictions: None,
            address_book: AddressBook::new(),
        }
    }

    #[inline]
    #[must_use]
    pub fn get_state(&self) -> Option<&VersionedState> {
        self.state.as_ref()
    }

    #[inline]
    pub fn clear(&mut self) {
        self.state = None;
        self.initialized = false;
        self.policy_locks = None;
        self.policy_supply_queue = None;
        self.policy_markets = None;
        self.policy_principals = None;
        self.policy_cap_groups = None;
        self.restrictions = None;
        self.address_book.clear();
    }
}

impl Storage for MemoryStorage {
    fn load_state(&self) -> Result<Option<VersionedState>, RuntimeError> {
        Ok(self.state.clone())
    }

    fn save_state(&mut self, state: &VersionedState) -> Result<(), RuntimeError> {
        self.state = Some(state.clone());
        self.initialized = true;
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }

    fn get_version(&self) -> Result<StorageVersion, RuntimeError> {
        self.state
            .as_ref()
            .map(|s| s.version)
            .ok_or_else(|| RuntimeError::storage_error("state not initialized"))
    }

    fn load_paused(&self) -> Result<bool, RuntimeError> {
        Ok(self.paused)
    }

    fn save_paused(&mut self, paused: bool) -> Result<(), RuntimeError> {
        self.paused = paused;
        Ok(())
    }

    fn load_policy_state(&self) -> Result<Option<PolicyState>, RuntimeError> {
        compose_policy_state(
            self.policy_markets.clone(),
            self.policy_principals.clone(),
            self.policy_cap_groups.clone(),
            self.policy_locks.clone(),
            self.policy_supply_queue.clone(),
        )
    }

    fn save_policy_state(&mut self, state: &PolicyState) -> Result<(), RuntimeError> {
        self.policy_locks = Some(state.leases().clone());
        self.policy_supply_queue = Some(state.supply_queue().clone());
        self.policy_markets = Some(state.markets().clone());
        self.policy_principals = Some(state.principals().clone());
        self.policy_cap_groups = Some(state.cap_groups().clone());
        Ok(())
    }

    fn load_restrictions(&self) -> Result<Option<Restrictions>, RuntimeError> {
        Ok(self.restrictions.clone())
    }

    fn save_restrictions(
        &mut self,
        restrictions: &Option<Restrictions>,
    ) -> Result<(), RuntimeError> {
        self.restrictions = restrictions.clone();
        Ok(())
    }

    fn load_address(&self, kernel_addr: &Address) -> Result<Option<SdkAddress>, RuntimeError> {
        Ok(self.address_book.resolve(kernel_addr).cloned())
    }

    fn save_address(
        &mut self,
        kernel_addr: &Address,
        soroban_addr: &SdkAddress,
    ) -> Result<(), RuntimeError> {
        self.address_book.insert(*kernel_addr, soroban_addr.clone());
        Ok(())
    }
}

struct TestAllocationDecision {
    filtered_plan: Vec<AllocationPlanEntry>,
    total_allocated: u128,
}

fn build_partial_allocation_plan_excluding_leased<S, A, E>(
    vault: &CuratorVault<S, A, E>,
    plan: &[(TargetId, u128)],
    now_ns: TimestampNs,
) -> TestAllocationDecision
where
    S: Storage,
    A: AuthAdapter,
    E: EffectInterpreter + AddressRegistrar,
{
    let filtered_plan = plan
        .iter()
        .copied()
        .filter(|(target_id, _)| {
            vault
                .policy_state()
                .leases()
                .is_unleased(*target_id, now_ns)
        })
        .map(|(target_id, amount)| AllocationPlanEntry::new(target_id, amount))
        .collect::<Vec<_>>();
    let total_allocated = filtered_plan.iter().map(|entry| entry.amount).sum();

    TestAllocationDecision {
        filtered_plan,
        total_allocated,
    }
}

pub fn begin_allocating<S, A, E>(
    vault: &mut CuratorVault<S, A, E>,
    caller: Address,
    plan: Vec<(TargetId, u128)>,
    current_ns: u64,
) -> Result<u64, RuntimeError>
where
    S: Storage,
    A: AuthAdapter,
    E: EffectInterpreter + AddressRegistrar,
{
    let decision =
        build_partial_allocation_plan_excluding_leased(vault, &plan, TimestampNs(current_ns));
    vault.authorize(ActionKind::BeginAllocating, caller)?;
    let op_id = {
        let state = vault.state_mut()?;
        if decision.total_allocated > state.idle_assets {
            return Err(RuntimeError::insufficient_balance(
                state.idle_assets,
                decision.total_allocated,
            ));
        }

        let op_id = CuratorVault::<S, A, E>::reserve_op_id(state)?;
        state.idle_assets -= decision.total_allocated;
        state.sync_total_assets();

        let result = transition_to_runtime(start_allocation(
            mem::take(&mut state.op_state),
            decision.filtered_plan,
            op_id,
        ))?;
        state.op_state = result.new_state;
        op_id
    };
    vault.save_state()?;
    Ok(op_id)
}

pub fn finish_allocating<S, A, E>(
    vault: &mut CuratorVault<S, A, E>,
    caller: Address,
    op_id: u64,
) -> Result<crate::contract::AllocationResult, RuntimeError>
where
    S: Storage,
    A: AuthAdapter,
    E: EffectInterpreter + AddressRegistrar,
{
    vault.authorize(ActionKind::FinishAllocating, caller)?;
    let result = {
        let state = vault.state_mut()?;
        let transition_result = transition_to_runtime(complete_allocation(
            mem::take(&mut state.op_state),
            op_id,
            None,
        ))?;
        state.op_state = transition_result.new_state;
        crate::contract::AllocationResult {
            op_id,
            new_external_assets: state.external_assets,
            summary: EffectSummary::new(),
        }
    };
    vault.save_state()?;
    Ok(result)
}
