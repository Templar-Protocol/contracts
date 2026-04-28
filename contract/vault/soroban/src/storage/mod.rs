//! Storage versioning and persistence for Soroban ledger.
//!
//! This module provides versioned storage wrappers for persisting vault state
//! to the Soroban ledger. It handles schema migrations and forward compatibility.

use alloc::string::String;
use alloc::vec::Vec;
use derive_more::From;
use soroban_sdk::{symbol_short, Address as SdkAddress, Bytes, BytesN, Env, Symbol};
use templar_curator_primitives::policy::cap_group::{CapGroup, CapGroupId, CapGroupRecord};
use templar_curator_primitives::policy::market_lock::{
    FencingToken, LeaseOwner, MarketLease, MarketLeaseRegistry,
};
use templar_curator_primitives::policy::state::{MarketConfig, OrderedMap};
use templar_curator_primitives::policy::supply_queue::{SupplyQueue, SupplyQueueEntry};
use templar_curator_primitives::PolicyState;
use templar_vault_kernel::{
    Address, AllocatingState, AllocationPlanEntry, FeeAccrualAnchor, OpState, PayoutState,
    PendingWithdrawal, RefreshingState, Restrictions, TargetId, VaultState, Wad, WithdrawQueue,
    WithdrawingState,
};

use crate::error::RuntimeError;

/// Re-extend TTL when remaining TTL drops below ~30 days (at ~5s/ledger).
pub(crate) const DEFAULT_TTL_THRESHOLD: u32 = 518_400;
/// Extend TTL to the Soroban maximum (~6 months at ~5s/ledger).
/// For a vault contract holding real assets, maximum TTL prevents state
/// loss during extended pauses or periods of inactivity.
pub(crate) const DEFAULT_TTL_EXTEND_TO: u32 = 3_110_400;

/// Internal persistent storage keys. Using Symbol constants instead of a
/// `#[contracttype]` enum to avoid contractspec bloat and enum conversion codegen.
#[allow(non_upper_case_globals)]
pub struct SorobanStorageKey;

#[allow(non_upper_case_globals)]
impl SorobanStorageKey {
    pub const StateBlob: Symbol = symbol_short!("stblob");
    pub const PolicyLocks: Symbol = symbol_short!("plocks");
    pub const PolicySupplyQueue: Symbol = symbol_short!("psupplyq");
    pub const PolicyMarkets: Symbol = symbol_short!("pmkts");
    pub const PolicyPrincipals: Symbol = symbol_short!("pprncpls");
    pub const PolicyCapGroups: Symbol = symbol_short!("pcapgrps");
    pub const Restrictions: Symbol = symbol_short!("restrict");
    pub const Paused: Symbol = symbol_short!("paused_l"); // legacy pause key (migration)
    pub const PausedState: Symbol = symbol_short!("paused_s");
}

fn push_u8(out: &mut Vec<u8>, value: u8) {
    out.push(value);
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u128(out: &mut Vec<u8>, value: u128) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_address(out: &mut Vec<u8>, value: &Address) {
    out.extend_from_slice(value.as_bytes());
}

fn push_bytes(out: &mut Vec<u8>, value: &[u8]) {
    push_u32(out, value.len() as u32);
    out.extend_from_slice(value);
}

fn read_exact<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    len: usize,
) -> Result<&'a [u8], RuntimeError> {
    let end = cursor
        .checked_add(len)
        .ok_or_else(|| RuntimeError::storage_error(""))?;
    let slice = bytes
        .get(*cursor..end)
        .ok_or_else(|| RuntimeError::storage_error(""))?;
    *cursor = end;
    Ok(slice)
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, RuntimeError> {
    Ok(read_exact(bytes, cursor, 1)?[0])
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, RuntimeError> {
    let mut raw = [0u8; 4];
    raw.copy_from_slice(read_exact(bytes, cursor, 4)?);
    Ok(u32::from_le_bytes(raw))
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, RuntimeError> {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(read_exact(bytes, cursor, 8)?);
    Ok(u64::from_le_bytes(raw))
}

fn read_u128(bytes: &[u8], cursor: &mut usize) -> Result<u128, RuntimeError> {
    let mut raw = [0u8; 16];
    raw.copy_from_slice(read_exact(bytes, cursor, 16)?);
    Ok(u128::from_le_bytes(raw))
}

fn read_address(bytes: &[u8], cursor: &mut usize) -> Result<Address, RuntimeError> {
    let mut raw = [0u8; 32];
    raw.copy_from_slice(read_exact(bytes, cursor, 32)?);
    Ok(Address(raw))
}

fn read_bytes<'a>(bytes: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], RuntimeError> {
    let len = read_u32(bytes, cursor)? as usize;
    read_exact(bytes, cursor, len)
}

fn finish_decode(bytes: &[u8], cursor: usize) -> Result<(), RuntimeError> {
    if cursor == bytes.len() {
        Ok(())
    } else {
        Err(RuntimeError::storage_error(""))
    }
}

fn encode_cap_group_id(id: &CapGroupId, out: &mut Vec<u8>) {
    push_bytes(out, id.as_str().as_bytes());
}

fn decode_cap_group_id(bytes: &[u8], cursor: &mut usize) -> Result<CapGroupId, RuntimeError> {
    let raw = read_bytes(bytes, cursor)?;
    let id = String::from_utf8(raw.to_vec()).map_err(|_| RuntimeError::storage_error(""))?;
    CapGroupId::try_from(id).map_err(|_| RuntimeError::storage_error(""))
}

pub(crate) fn encode_restrictions(mode: &Restrictions) -> Vec<u8> {
    let mut out = Vec::new();
    match mode {
        Restrictions::Blacklist(addresses) => {
            push_u8(&mut out, 0);
            push_u32(&mut out, addresses.len() as u32);
            for address in addresses {
                push_address(&mut out, address);
            }
        }
        Restrictions::Whitelist(addresses) => {
            push_u8(&mut out, 1);
            push_u32(&mut out, addresses.len() as u32);
            for address in addresses {
                push_address(&mut out, address);
            }
        }
    }
    out
}

pub(crate) fn decode_restrictions(bytes: &[u8]) -> Result<Restrictions, RuntimeError> {
    let mut cursor = 0usize;
    let tag = read_u8(bytes, &mut cursor)?;
    let len = read_u32(bytes, &mut cursor)? as usize;
    let mut addresses = Vec::with_capacity(len);
    for _ in 0..len {
        addresses.push(read_address(bytes, &mut cursor)?);
    }
    let restrictions = match tag {
        0 => Restrictions::blacklist(addresses),
        1 => Restrictions::whitelist(addresses),
        _ => return Err(RuntimeError::storage_error("")),
    };
    finish_decode(bytes, cursor)?;
    Ok(restrictions)
}

pub(crate) fn encode_supply_queue(queue: &SupplyQueue) -> Vec<u8> {
    let mut out = Vec::new();
    let max_length = queue.max_length().map(|value| value.get()).unwrap_or(0);
    push_u32(&mut out, max_length);
    let entries = queue.entries();
    push_u32(&mut out, entries.len() as u32);
    for entry in entries {
        push_u32(&mut out, entry.target_id);
        push_u128(&mut out, entry.amount);
        push_u8(&mut out, entry.priority);
    }
    out
}

pub(crate) fn decode_supply_queue(bytes: &[u8]) -> Result<SupplyQueue, RuntimeError> {
    let mut cursor = 0usize;
    let max_length = read_u32(bytes, &mut cursor)?;
    let count = read_u32(bytes, &mut cursor)? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let target_id = read_u32(bytes, &mut cursor)?;
        let amount = read_u128(bytes, &mut cursor)?;
        let priority = read_u8(bytes, &mut cursor)?;
        let entry = SupplyQueueEntry::new_with_priority(target_id, amount, priority)
            .map_err(|_| RuntimeError::storage_error(""))?;
        entries.push(entry);
    }
    let max_length = core::num::NonZeroU32::new(max_length);
    let queue = SupplyQueue::try_from_entries(entries, max_length)
        .map_err(|_| RuntimeError::storage_error(""))?;
    finish_decode(bytes, cursor)?;
    Ok(queue)
}

pub(crate) fn encode_cap_groups(cap_groups: &OrderedMap<CapGroupId, CapGroupRecord>) -> Vec<u8> {
    let mut out = Vec::new();
    push_u32(&mut out, cap_groups.len() as u32);
    for (id, record) in cap_groups.iter() {
        encode_cap_group_id(id, &mut out);
        push_u128(&mut out, record.principal);
        match record.cap.absolute_cap() {
            Some(value) => {
                push_u8(&mut out, 1);
                push_u128(&mut out, value);
            }
            None => push_u8(&mut out, 0),
        }
        match record.cap.relative_cap() {
            Some(value) => {
                push_u8(&mut out, 1);
                push_u128(&mut out, value.as_u128_trunc());
            }
            None => push_u8(&mut out, 0),
        }
    }
    out
}

pub(crate) fn decode_cap_groups(
    bytes: &[u8],
) -> Result<OrderedMap<CapGroupId, CapGroupRecord>, RuntimeError> {
    let mut cursor = 0usize;
    let count = read_u32(bytes, &mut cursor)? as usize;
    let mut cap_groups = OrderedMap::new();
    for _ in 0..count {
        let id = decode_cap_group_id(bytes, &mut cursor)?;
        let principal = read_u128(bytes, &mut cursor)?;
        let absolute_cap = match read_u8(bytes, &mut cursor)? {
            0 => None,
            1 => Some(read_u128(bytes, &mut cursor)?),
            _ => {
                return Err(RuntimeError::storage_error(
                    "cap group absolute cap tag invalid",
                ))
            }
        };
        let relative_cap = match read_u8(bytes, &mut cursor)? {
            0 => None,
            1 => Some(Wad::from(read_u128(bytes, &mut cursor)?)),
            _ => {
                return Err(RuntimeError::storage_error(
                    "cap group relative cap tag invalid",
                ))
            }
        };
        let mut cap = CapGroup::default();
        cap.set_absolute_cap(absolute_cap);
        cap.set_relative_cap(relative_cap);
        let _ = cap_groups.insert(id, CapGroupRecord { cap, principal });
    }
    finish_decode(bytes, cursor)?;
    Ok(cap_groups)
}

pub(crate) fn encode_markets(markets: &OrderedMap<TargetId, MarketConfig>) -> Vec<u8> {
    let mut out = Vec::new();
    push_u32(&mut out, markets.len() as u32);
    for (target_id, config) in markets.iter() {
        push_u32(&mut out, *target_id);
        push_u8(&mut out, u8::from(config.enabled));
        push_u128(&mut out, config.cap);
        match &config.cap_group_id {
            Some(id) => {
                push_u8(&mut out, 1);
                encode_cap_group_id(id, &mut out);
            }
            None => push_u8(&mut out, 0),
        }
    }
    out
}

pub(crate) fn decode_markets(
    bytes: &[u8],
) -> Result<OrderedMap<TargetId, MarketConfig>, RuntimeError> {
    let mut cursor = 0usize;
    let count = read_u32(bytes, &mut cursor)? as usize;
    let mut markets = OrderedMap::new();
    for _ in 0..count {
        let target_id = read_u32(bytes, &mut cursor)?;
        let enabled = match read_u8(bytes, &mut cursor)? {
            0 => false,
            1 => true,
            _ => return Err(RuntimeError::storage_error("")),
        };
        let cap = read_u128(bytes, &mut cursor)?;
        let cap_group_id = match read_u8(bytes, &mut cursor)? {
            0 => None,
            1 => Some(decode_cap_group_id(bytes, &mut cursor)?),
            _ => return Err(RuntimeError::storage_error("")),
        };
        let _ = markets.insert(target_id, MarketConfig::new(enabled, cap, cap_group_id));
    }
    finish_decode(bytes, cursor)?;
    Ok(markets)
}

pub(crate) fn encode_principals(principals: &OrderedMap<TargetId, u128>) -> Vec<u8> {
    let mut out = Vec::new();
    push_u32(&mut out, principals.len() as u32);
    for (target_id, principal) in principals.iter() {
        push_u32(&mut out, *target_id);
        push_u128(&mut out, *principal);
    }
    out
}

pub(crate) fn decode_principals(bytes: &[u8]) -> Result<OrderedMap<TargetId, u128>, RuntimeError> {
    let mut cursor = 0usize;
    let count = read_u32(bytes, &mut cursor)? as usize;
    let mut principals = OrderedMap::new();
    for _ in 0..count {
        let target_id = read_u32(bytes, &mut cursor)?;
        let principal = read_u128(bytes, &mut cursor)?;
        let _ = principals.insert(target_id, principal);
    }
    finish_decode(bytes, cursor)?;
    Ok(principals)
}

pub(crate) fn encode_policy_locks(leases: &MarketLeaseRegistry) -> Vec<u8> {
    let mut out = Vec::new();
    push_u32(&mut out, leases.stored_len() as u32);
    push_u64(&mut out, leases.next_fencing_token());
    for (_, lease) in leases.iter() {
        push_u32(&mut out, lease.target_id);
        push_u64(&mut out, lease.owner.0);
        match lease.op_id {
            Some(op_id) => {
                push_u8(&mut out, 1);
                push_u64(&mut out, op_id);
            }
            None => push_u8(&mut out, 0),
        }
        push_u64(&mut out, lease.acquired_at.as_u64());
        push_u64(&mut out, lease.expires_at.as_u64());
        push_u64(&mut out, lease.fencing_token.0);
    }
    out
}

pub(crate) fn decode_policy_locks(bytes: &[u8]) -> Result<MarketLeaseRegistry, RuntimeError> {
    let mut cursor = 0usize;
    let count = read_u32(bytes, &mut cursor)? as usize;
    let next_fencing_token = read_u64(bytes, &mut cursor)?;
    let mut leases_by_target = OrderedMap::new();
    for _ in 0..count {
        let target_id = read_u32(bytes, &mut cursor)?;
        let owner = LeaseOwner(read_u64(bytes, &mut cursor)?);
        let op_id = match read_u8(bytes, &mut cursor)? {
            0 => None,
            1 => Some(read_u64(bytes, &mut cursor)?),
            _ => {
                return Err(RuntimeError::storage_error(
                    "policy locks op_id tag invalid",
                ))
            }
        };
        let acquired_at = templar_vault_kernel::TimestampNs(read_u64(bytes, &mut cursor)?);
        let expires_at = templar_vault_kernel::TimestampNs(read_u64(bytes, &mut cursor)?);
        let fencing_token = FencingToken(read_u64(bytes, &mut cursor)?);

        let lease = MarketLease::from_parts(
            target_id,
            owner,
            op_id,
            acquired_at,
            expires_at,
            fencing_token,
        );
        let _ = leases_by_target.insert(target_id, lease);
    }
    finish_decode(bytes, cursor)?;
    Ok(MarketLeaseRegistry::from_parts(
        leases_by_target,
        next_fencing_token,
    ))
}

fn encode_withdraw_queue(queue: &WithdrawQueue, out: &mut Vec<u8>) {
    push_u64(out, queue.next_withdraw_to_execute);
    push_u64(out, queue.next_pending_withdrawal_id);
    let entries: Vec<_> = queue.iter().collect();
    push_u32(out, entries.len() as u32);
    for (id, withdrawal) in entries {
        push_u64(out, id);
        push_address(out, &withdrawal.owner);
        push_address(out, &withdrawal.receiver);
        push_u128(out, withdrawal.escrow_shares);
        push_u128(out, withdrawal.expected_assets);
        push_u64(out, withdrawal.requested_at_ns.as_u64());
    }
}

fn decode_withdraw_queue(bytes: &[u8], cursor: &mut usize) -> Result<WithdrawQueue, RuntimeError> {
    let next_withdraw_to_execute = read_u64(bytes, cursor)?;
    let next_pending_withdrawal_id = read_u64(bytes, cursor)?;
    let count = read_u32(bytes, cursor)? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let id = read_u64(bytes, cursor)?;
        let withdrawal = PendingWithdrawal::new(
            read_address(bytes, cursor)?,
            read_address(bytes, cursor)?,
            read_u128(bytes, cursor)?,
            read_u128(bytes, cursor)?,
            templar_vault_kernel::TimestampNs(read_u64(bytes, cursor)?),
        );
        entries.push((id, withdrawal));
    }
    Ok(WithdrawQueue::with_state(
        entries,
        next_withdraw_to_execute,
        next_pending_withdrawal_id,
    ))
}

fn encode_op_state(op_state: &OpState, out: &mut Vec<u8>) {
    match op_state {
        OpState::Idle => push_u8(out, 0),
        OpState::Allocating(state) => {
            push_u8(out, 1);
            push_u64(out, state.op_id);
            push_u32(out, state.index);
            push_u128(out, state.remaining);
            push_u32(out, state.plan.len() as u32);
            for entry in &state.plan {
                push_u32(out, entry.target_id);
                push_u128(out, entry.amount);
            }
        }
        OpState::Withdrawing(state) => {
            push_u8(out, 2);
            push_u64(out, state.op_id);
            push_u64(out, state.request_id);
            push_u32(out, state.index);
            push_u128(out, state.remaining);
            push_u128(out, state.collected);
            push_address(out, &state.receiver);
            push_address(out, &state.owner);
            push_u128(out, state.escrow_shares);
        }
        OpState::Refreshing(state) => {
            push_u8(out, 3);
            push_u64(out, state.op_id);
            push_u32(out, state.index);
            push_u32(out, state.plan.len() as u32);
            for target_id in &state.plan {
                push_u32(out, *target_id);
            }
        }
        OpState::Payout(state) => {
            push_u8(out, 4);
            push_u64(out, state.op_id);
            push_u64(out, state.request_id);
            push_address(out, &state.receiver);
            push_u128(out, state.amount);
            push_address(out, &state.owner);
            push_u128(out, state.escrow_shares);
            push_u128(out, state.burn_shares);
        }
    }
}

fn decode_op_state(bytes: &[u8], cursor: &mut usize) -> Result<OpState, RuntimeError> {
    match read_u8(bytes, cursor)? {
        0 => Ok(OpState::Idle),
        1 => {
            let op_id = read_u64(bytes, cursor)?;
            let index = read_u32(bytes, cursor)?;
            let remaining = read_u128(bytes, cursor)?;
            let count = read_u32(bytes, cursor)? as usize;
            let mut plan = Vec::with_capacity(count);
            for _ in 0..count {
                plan.push(AllocationPlanEntry::new(
                    read_u32(bytes, cursor)?,
                    read_u128(bytes, cursor)?,
                ));
            }
            Ok(OpState::Allocating(AllocatingState {
                op_id,
                index,
                remaining,
                plan,
            }))
        }
        2 => Ok(OpState::Withdrawing(WithdrawingState {
            op_id: read_u64(bytes, cursor)?,
            request_id: read_u64(bytes, cursor)?,
            index: read_u32(bytes, cursor)?,
            remaining: read_u128(bytes, cursor)?,
            collected: read_u128(bytes, cursor)?,
            receiver: read_address(bytes, cursor)?,
            owner: read_address(bytes, cursor)?,
            escrow_shares: read_u128(bytes, cursor)?,
        })),
        3 => {
            let op_id = read_u64(bytes, cursor)?;
            let index = read_u32(bytes, cursor)?;
            let count = read_u32(bytes, cursor)? as usize;
            let mut plan = Vec::with_capacity(count);
            for _ in 0..count {
                plan.push(read_u32(bytes, cursor)?);
            }
            Ok(OpState::Refreshing(RefreshingState { op_id, index, plan }))
        }
        4 => Ok(OpState::Payout(PayoutState {
            op_id: read_u64(bytes, cursor)?,
            request_id: read_u64(bytes, cursor)?,
            receiver: read_address(bytes, cursor)?,
            amount: read_u128(bytes, cursor)?,
            owner: read_address(bytes, cursor)?,
            escrow_shares: read_u128(bytes, cursor)?,
            burn_shares: read_u128(bytes, cursor)?,
        })),
        _ => Err(RuntimeError::storage_error("")),
    }
}

pub(crate) fn encode_state_blob(state: &VaultState) -> Vec<u8> {
    let mut out = Vec::new();
    push_u128(&mut out, state.total_assets);
    push_u128(&mut out, state.total_shares);
    push_u128(&mut out, state.idle_assets);
    push_u128(&mut out, state.external_assets);
    push_u128(&mut out, state.fee_anchor.total_assets);
    push_u64(&mut out, state.fee_anchor.timestamp_ns.as_u64());
    encode_op_state(&state.op_state, &mut out);
    encode_withdraw_queue(&state.withdraw_queue, &mut out);
    push_u64(&mut out, state.next_op_id);
    out
}

pub(crate) fn decode_state_blob(bytes: &[u8]) -> Result<VaultState, RuntimeError> {
    let mut cursor = 0usize;
    let state = VaultState {
        total_assets: read_u128(bytes, &mut cursor)?,
        total_shares: read_u128(bytes, &mut cursor)?,
        idle_assets: read_u128(bytes, &mut cursor)?,
        external_assets: read_u128(bytes, &mut cursor)?,
        fee_anchor: FeeAccrualAnchor::new(
            read_u128(bytes, &mut cursor)?,
            templar_vault_kernel::TimestampNs(read_u64(bytes, &mut cursor)?),
        ),
        op_state: decode_op_state(bytes, &mut cursor)?,
        withdraw_queue: decode_withdraw_queue(bytes, &mut cursor)?,
        next_op_id: read_u64(bytes, &mut cursor)?,
    };

    if cursor != bytes.len() {
        return Err(RuntimeError::storage_error(
            "state blob trailing bytes are invalid",
        ));
    }

    Ok(state)
}

pub(crate) fn compose_policy_state(
    markets: Option<OrderedMap<TargetId, MarketConfig>>,
    principals: Option<OrderedMap<TargetId, u128>>,
    cap_groups: Option<OrderedMap<CapGroupId, CapGroupRecord>>,
    leases: Option<MarketLeaseRegistry>,
    supply_queue: Option<SupplyQueue>,
) -> Result<Option<PolicyState>, RuntimeError> {
    if markets.is_none()
        && principals.is_none()
        && cap_groups.is_none()
        && leases.is_none()
        && supply_queue.is_none()
    {
        return Ok(None);
    }

    let state = PolicyState::from_parts(
        markets.unwrap_or_default(),
        principals.unwrap_or_default(),
        cap_groups.unwrap_or_default(),
        leases.unwrap_or_default(),
        supply_queue.unwrap_or_default(),
    )
    .map_err(|_| RuntimeError::storage_error(""))?;

    Ok(Some(state))
}

/// Soroban ledger storage implementation.
///
/// Uses the Soroban SDK's persistent storage to store vault state
/// directly on the blockchain ledger.
pub struct SorobanStorage<'a> {
    env: &'a Env,
}

impl<'a> SorobanStorage<'a> {
    /// Create a new Soroban storage instance.
    #[inline]
    #[must_use]
    pub fn new(env: &'a Env) -> Self {
        Self { env }
    }

    const SK_ADDRBOOK: Symbol = symbol_short!("addrbook");

    fn address_key(&self, kernel_addr: &Address) -> (Symbol, BytesN<32>) {
        (
            Self::SK_ADDRBOOK,
            BytesN::from_array(self.env, kernel_addr.as_bytes()),
        )
    }

    fn load_blob(&self, key: &Symbol) -> Option<Vec<u8>> {
        self.env
            .storage()
            .persistent()
            .get::<_, Bytes>(key)
            .map(|bytes| bytes.to_alloc_vec())
    }

    fn save_blob(&self, key: &Symbol, bytes: &[u8]) {
        self.env
            .storage()
            .persistent()
            .set(key, &Bytes::from_slice(self.env, bytes));
    }

    /// Load a kernel-to-Soroban address mapping from persistent storage.
    pub fn load_address(&self, kernel_addr: &Address) -> Option<SdkAddress> {
        let key = self.address_key(kernel_addr);
        self.env.storage().persistent().get(&key)
    }

    /// Save a kernel-to-Soroban address mapping to persistent storage.
    pub fn save_address(&self, kernel_addr: &Address, soroban_addr: &SdkAddress) {
        let key = self.address_key(kernel_addr);
        self.env.storage().persistent().set(&key, soroban_addr);
        self.env.storage().persistent().extend_ttl(
            &key,
            DEFAULT_TTL_THRESHOLD,
            DEFAULT_TTL_EXTEND_TO,
        );
        self.extend_default_ttl();
    }

    pub(crate) fn load_state_blob(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::StateBlob)
    }

    pub(crate) fn save_state_blob(&self, state: &[u8]) {
        self.save_blob(&SorobanStorageKey::StateBlob, state);
    }

    pub fn load_policy_locks(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::PolicyLocks)
    }

    pub fn save_policy_locks(&self, state: &[u8]) {
        self.save_blob(&SorobanStorageKey::PolicyLocks, state);
    }

    pub fn load_policy_supply_queue(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::PolicySupplyQueue)
    }

    pub fn save_policy_supply_queue(&self, state: &[u8]) {
        self.save_blob(&SorobanStorageKey::PolicySupplyQueue, state);
    }

    pub fn load_policy_markets(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::PolicyMarkets)
    }

    pub fn save_policy_markets(&self, state: &[u8]) {
        self.save_blob(&SorobanStorageKey::PolicyMarkets, state);
    }

    pub fn load_policy_principals(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::PolicyPrincipals)
    }

    pub fn save_policy_principals(&self, state: &[u8]) {
        self.save_blob(&SorobanStorageKey::PolicyPrincipals, state);
    }

    pub fn load_policy_cap_groups(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::PolicyCapGroups)
    }

    pub fn save_policy_cap_groups(&self, state: &[u8]) {
        self.save_blob(&SorobanStorageKey::PolicyCapGroups, state);
    }

    /// Load restrictions from persistent storage.
    pub fn load_restrictions(&self) -> Option<Vec<u8>> {
        self.load_blob(&SorobanStorageKey::Restrictions)
    }

    /// Save restrictions to persistent storage.
    pub fn save_restrictions(&self, restrictions: &[u8]) {
        self.save_blob(&SorobanStorageKey::Restrictions, restrictions);
    }

    /// Clear restrictions from persistent storage.
    pub fn clear_restrictions(&self) {
        self.env
            .storage()
            .persistent()
            .remove(&SorobanStorageKey::Restrictions);
    }

    /// Check if the contract is paused.
    pub fn is_paused(&self) -> bool {
        self.env
            .storage()
            .instance()
            .get(&SorobanStorageKey::PausedState)
            .unwrap_or(false)
    }

    /// Set the pause state in instance storage.
    pub fn set_paused(&self, paused: bool) {
        self.env
            .storage()
            .instance()
            .set(&SorobanStorageKey::PausedState, &paused);
    }

    /// Check if the contract has the legacy pause key (for migration).
    pub fn has_legacy_paused(&self) -> bool {
        self.env
            .storage()
            .instance()
            .has(&SorobanStorageKey::Paused)
    }

    /// Get the legacy pause value and remove it (for migration).
    pub fn take_legacy_paused(&self) -> Option<bool> {
        if self.has_legacy_paused() {
            let paused: bool = self
                .env
                .storage()
                .instance()
                .get(&SorobanStorageKey::Paused)
                .unwrap_or(false);
            self.env
                .storage()
                .instance()
                .remove(&SorobanStorageKey::Paused);
            Some(paused)
        } else {
            None
        }
    }

    /// Check if storage has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.env
            .storage()
            .persistent()
            .has(&SorobanStorageKey::StateBlob)
    }

    /// Extend the TTL of storage entries.
    ///
    /// Call this periodically to prevent state from expiring.
    pub fn extend_ttl(&self, threshold: u32, extend_to: u32) {
        self.env
            .storage()
            .instance()
            .extend_ttl(threshold, extend_to);
        let p = self.env.storage().persistent();
        // Extend each persistent key if it exists.
        for key in &[
            SorobanStorageKey::StateBlob,
            SorobanStorageKey::PolicyLocks,
            SorobanStorageKey::PolicySupplyQueue,
            SorobanStorageKey::PolicyMarkets,
            SorobanStorageKey::PolicyPrincipals,
            SorobanStorageKey::PolicyCapGroups,
            SorobanStorageKey::Restrictions,
        ] {
            if p.has(key) {
                p.extend_ttl(key, threshold, extend_to);
            }
        }
    }

    fn extend_default_ttl(&self) {
        self.extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
    }
}

impl Storage for SorobanStorage<'_> {
    fn load_state(&self) -> Result<Option<VaultState>, RuntimeError> {
        if let Some(stored) = self.load_state_blob() {
            return Ok(Some(decode_state_blob(&stored)?));
        }

        Ok(None)
    }

    fn save_state(&mut self, state: &VaultState) -> Result<(), RuntimeError> {
        let state_blob = encode_state_blob(state);
        self.save_state_blob(&state_blob);
        self.extend_default_ttl();
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        SorobanStorage::is_initialized(self)
    }

    fn load_paused(&self) -> Result<bool, RuntimeError> {
        Ok(self.is_paused())
    }

    fn save_paused(&mut self, paused: bool) -> Result<(), RuntimeError> {
        self.set_paused(paused);
        self.extend_default_ttl();
        Ok(())
    }

    fn load_policy_state(&self) -> Result<Option<PolicyState>, RuntimeError> {
        let leases = match self.load_policy_locks() {
            Some(stored) => Some(decode_policy_locks(&stored)?),
            None => None,
        };
        let supply_queue = match self.load_policy_supply_queue() {
            Some(stored) => Some(decode_supply_queue(&stored)?),
            None => None,
        };
        let markets = match self.load_policy_markets() {
            Some(stored) => Some(decode_markets(&stored)?),
            None => None,
        };
        let principals = match self.load_policy_principals() {
            Some(stored) => Some(decode_principals(&stored)?),
            None => None,
        };
        let cap_groups = match self.load_policy_cap_groups() {
            Some(stored) => Some(decode_cap_groups(&stored)?),
            None => None,
        };

        compose_policy_state(markets, principals, cap_groups, leases, supply_queue)
    }

    fn save_policy_state(&mut self, state: &PolicyState) -> Result<(), RuntimeError> {
        self.save_policy_locks(&encode_policy_locks(state.leases()));
        self.save_policy_supply_queue(&encode_supply_queue(state.supply_queue()));
        self.save_policy_markets(&encode_markets(state.markets()));
        self.save_policy_principals(&encode_principals(state.principals()));
        self.save_policy_cap_groups(&encode_cap_groups(state.cap_groups()));
        self.extend_default_ttl();
        Ok(())
    }

    fn load_restrictions(&self) -> Result<Option<Restrictions>, RuntimeError> {
        let restrictions = match SorobanStorage::load_restrictions(self) {
            Some(stored) => Some(decode_restrictions(&stored)?),
            None => None,
        };

        Ok(restrictions)
    }

    fn save_restrictions(
        &mut self,
        restrictions: &Option<Restrictions>,
    ) -> Result<(), RuntimeError> {
        if let Some(restrictions) = restrictions {
            SorobanStorage::save_restrictions(self, &encode_restrictions(restrictions));
        } else {
            SorobanStorage::clear_restrictions(self);
        }
        self.extend_default_ttl();
        Ok(())
    }

    fn load_address(&self, kernel_addr: &Address) -> Result<Option<SdkAddress>, RuntimeError> {
        Ok(SorobanStorage::load_address(self, kernel_addr))
    }

    fn save_address(
        &mut self,
        kernel_addr: &Address,
        soroban_addr: &SdkAddress,
    ) -> Result<(), RuntimeError> {
        SorobanStorage::save_address(self, kernel_addr, soroban_addr);
        Ok(())
    }
}

/// Storage key types for different data categories.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StorageKey {
    /// Main vault state.
    VaultState,
    /// Pending withdrawal by ID.
    PendingWithdrawal(u64),
    /// Share balance for an account.
    ShareBalance([u8; 32]),
    /// Total share supply.
    TotalSupply,
}

/// Trait for storage operations.
///
/// Implementations of this trait handle the actual persistence to the
/// Soroban ledger.
pub trait Storage {
    fn load_state(&self) -> Result<Option<VaultState>, RuntimeError>;

    fn save_state(&mut self, state: &VaultState) -> Result<(), RuntimeError>;

    /// Check if storage has been initialized.
    fn is_initialized(&self) -> bool;

    /// Load the paused flag for the vault.
    fn load_paused(&self) -> Result<bool, RuntimeError>;

    /// Persist the paused flag for the vault.
    fn save_paused(&mut self, paused: bool) -> Result<(), RuntimeError>;

    /// Load the policy state for the vault.
    fn load_policy_state(&self) -> Result<Option<PolicyState>, RuntimeError>;

    /// Persist the policy state for the vault.
    fn save_policy_state(&mut self, state: &PolicyState) -> Result<(), RuntimeError>;

    /// Load kernel restrictions for the vault.
    fn load_restrictions(&self) -> Result<Option<Restrictions>, RuntimeError>;

    /// Persist kernel restrictions for the vault.
    fn save_restrictions(
        &mut self,
        restrictions: &Option<Restrictions>,
    ) -> Result<(), RuntimeError>;

    /// Load a kernel-to-Soroban address mapping.
    fn load_address(&self, kernel_addr: &Address) -> Result<Option<SdkAddress>, RuntimeError>;

    /// Persist a kernel-to-Soroban address mapping.
    fn save_address(
        &mut self,
        kernel_addr: &Address,
        soroban_addr: &SdkAddress,
    ) -> Result<(), RuntimeError>;
}
