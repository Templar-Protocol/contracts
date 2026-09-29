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
    Address, AllocatingState, AllocationPlanEntry, EpochId, EpochPhase, EpochSnapshot, EpochState,
    FeeAccrualAnchor, OpState, PayoutState, PendingWithdrawal, RefreshingState, Restrictions,
    TargetId, ValuationReportRef, VaultState, Wad, WithdrawQueue, WithdrawingState,
};

use crate::error::RuntimeError;

pub(crate) mod deposit;
pub(crate) mod epoch;
#[cfg(test)]
mod tests;
/// Re-extend TTL when remaining TTL drops below ~30 days (at ~5s/ledger).
pub(crate) const DEFAULT_TTL_THRESHOLD: u32 = 518_400;
/// Extend TTL to the Soroban maximum (~6 months at ~5s/ledger).
/// For a vault contract holding real assets, maximum TTL prevents state
/// loss during extended pauses or periods of inactivity.
pub(crate) const DEFAULT_TTL_EXTEND_TO: u32 = 3_110_400;
pub(crate) const SOROBAN_MAX_PENDING_WITHDRAWALS: u32 = templar_vault_kernel::MAX_PENDING as u32;
pub(crate) const SOROBAN_MAX_RESTRICTION_ADDRESSES: usize = 3_072;

const MAX_CONTRACT_DATA_ENTRY_SIZE_BYTES: usize = 64 * 1024;
const BLOB_PAGE_BYTES: usize = 32 * 1024;
const BLOB_MAGIC: [u8; 3] = *b"TVP";
const BLOB_VERSION_CURRENT: u8 = 1;
const BLOB_INLINE: u8 = 0;
const BLOB_PAGED: u8 = 1;
const STORAGE_MAGIC: [u8; 3] = *b"TVS";
const STORAGE_VERSION_CURRENT: u8 = 1;
#[cfg(feature = "immediate-entrypoints")]
const STORAGE_VERSION_STATE_PAGED: u8 = 2;
const STORAGE_VERSION_STATE_EPOCH: u8 = 3;
const WITHDRAW_QUEUE_PAGE_SIZE: u64 = 128;
const WITHDRAW_PAGE_MAGIC: [u8; 3] = *b"TWP";
const WITHDRAW_PAGE_FORMAT_EPOCH: u8 = 1;
const WITHDRAW_PAGE_ITEM_BYTES: usize = 120;
#[cfg(feature = "immediate-entrypoints")]
const LEGACY_WITHDRAW_PAGE_ITEM_BYTES: usize = 112;
const STORAGE_KIND_STATE: u8 = 1;
const STORAGE_KIND_RESTRICTIONS: u8 = 2;
const STORAGE_KIND_SUPPLY_QUEUE: u8 = 3;
const STORAGE_KIND_MARKETS: u8 = 4;
const STORAGE_KIND_PRINCIPALS: u8 = 5;
const STORAGE_KIND_CAP_GROUPS: u8 = 6;
const STORAGE_KIND_POLICY_LOCKS: u8 = 7;
const STORAGE_KIND_EPOCH_STATE: u8 = 8;
const STORAGE_KIND_ACCEPTED_REPORT: u8 = 9;
const STORAGE_KIND_MAX_REPORT_AGE: u8 = 13;
/// Upper bound on a single versioned law record (epoch lifecycle, accepted
/// report tracking, or governance config). Records above this are corrupt.
const MAX_VERSIONED_RECORD_BYTES: usize = 4096;

#[derive(Clone, Copy)]
pub(crate) enum StorageKind {
    State,
    Restrictions,
    SupplyQueue,
    Markets,
    Principals,
    CapGroups,
    PolicyLocks,
    EpochState,
    AcceptedReport,
    MaxReportAge,
}

impl StorageKind {
    const fn tag(self) -> u8 {
        match self {
            Self::State => STORAGE_KIND_STATE,
            Self::Restrictions => STORAGE_KIND_RESTRICTIONS,
            Self::SupplyQueue => STORAGE_KIND_SUPPLY_QUEUE,
            Self::Markets => STORAGE_KIND_MARKETS,
            Self::Principals => STORAGE_KIND_PRINCIPALS,
            Self::CapGroups => STORAGE_KIND_CAP_GROUPS,
            Self::PolicyLocks => STORAGE_KIND_POLICY_LOCKS,
            Self::EpochState => STORAGE_KIND_EPOCH_STATE,
            Self::AcceptedReport => STORAGE_KIND_ACCEPTED_REPORT,
            Self::MaxReportAge => STORAGE_KIND_MAX_REPORT_AGE,
        }
    }
}

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
    pub const PausedState: Symbol = symbol_short!("paused_s");
}

fn push_storage_header_version(out: &mut Vec<u8>, kind: u8, version: u8) {
    out.extend_from_slice(&STORAGE_MAGIC);
    push_u8(out, kind);
    push_u8(out, version);
}

fn push_storage_header(out: &mut Vec<u8>, kind: u8) {
    push_storage_header_version(out, kind, STORAGE_VERSION_CURRENT);
}

fn storage_payload(bytes: &[u8], kind: StorageKind) -> Result<(u8, &[u8]), RuntimeError> {
    if bytes.len() < 5 || bytes[..3] != STORAGE_MAGIC || bytes[3] != kind.tag() {
        return Err(RuntimeError::storage_error("invalid storage envelope"));
    }
    Ok((bytes[4], &bytes[5..]))
}

struct StorageVersionDecoder<T> {
    version: u8,
    decode: fn(&[u8]) -> Result<T, RuntimeError>,
}

impl<T> StorageVersionDecoder<T> {
    const fn current(decode: fn(&[u8]) -> Result<T, RuntimeError>) -> Self {
        Self {
            version: STORAGE_VERSION_CURRENT,
            decode,
        }
    }
}

fn decode_storage_payload<T>(
    bytes: &[u8],
    kind: StorageKind,
    decoders: &[StorageVersionDecoder<T>],
) -> Result<T, RuntimeError> {
    let (version, payload) = storage_payload(bytes, kind)?;
    for decoder in decoders {
        if decoder.version == version {
            return (decoder.decode)(payload);
        }
    }

    Err(RuntimeError::storage_error("unsupported storage version"))
}

fn ensure_contract_data_entry_size(bytes: &[u8]) -> Result<(), RuntimeError> {
    if bytes.len() <= MAX_CONTRACT_DATA_ENTRY_SIZE_BYTES {
        Ok(())
    } else {
        Err(RuntimeError::storage_error("persistent entry too large"))
    }
}

fn push_blob_header(out: &mut Vec<u8>, mode: u8) {
    out.extend_from_slice(&BLOB_MAGIC);
    push_u8(out, BLOB_VERSION_CURRENT);
    push_u8(out, mode);
}

fn encode_blob_inline(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + bytes.len());
    push_blob_header(&mut out, BLOB_INLINE);
    push_u32(&mut out, bytes.len() as u32);
    out.extend_from_slice(bytes);
    out
}

fn encode_blob_manifest(bytes_len: usize, page_count: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(13);
    push_blob_header(&mut out, BLOB_PAGED);
    push_u32(&mut out, bytes_len as u32);
    push_u32(&mut out, page_count);
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlobManifest {
    Inline,
    Paged { len: usize, page_count: u32 },
}

fn decode_blob_manifest(bytes: &[u8]) -> Result<Option<BlobManifest>, RuntimeError> {
    if bytes.len() < 5 || bytes[..3] != BLOB_MAGIC {
        return Ok(None);
    }

    let version = bytes[3];
    if version != BLOB_VERSION_CURRENT {
        return Err(RuntimeError::storage_error(
            "unsupported blob storage version",
        ));
    }

    let mut cursor = 4usize;
    match read_u8(bytes, &mut cursor)? {
        BLOB_INLINE => {
            let len = read_u32(bytes, &mut cursor)? as usize;
            if bytes.len().saturating_sub(cursor) != len {
                return Err(RuntimeError::storage_error("invalid inline blob"));
            }
            Ok(Some(BlobManifest::Inline))
        }
        BLOB_PAGED => {
            let len = read_u32(bytes, &mut cursor)? as usize;
            let page_count = read_u32(bytes, &mut cursor)?;
            finish_decode(bytes, cursor)?;
            if page_count == 0 || len == 0 {
                return Err(RuntimeError::storage_error("invalid paged blob"));
            }
            let capacity = (page_count as usize)
                .checked_mul(BLOB_PAGE_BYTES)
                .ok_or_else(|| RuntimeError::storage_error("paged blob too large"))?;
            if len > capacity || len <= capacity.saturating_sub(BLOB_PAGE_BYTES) {
                return Err(RuntimeError::storage_error("invalid paged blob length"));
            }
            Ok(Some(BlobManifest::Paged { len, page_count }))
        }
        _ => Err(RuntimeError::storage_error("invalid blob storage mode")),
    }
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

fn bounded_count_for_fixed_items(
    bytes: &[u8],
    cursor: usize,
    count: usize,
    item_size: usize,
    error: &'static str,
) -> Result<usize, RuntimeError> {
    let remaining = bytes
        .len()
        .checked_sub(cursor)
        .ok_or_else(|| RuntimeError::storage_error(error))?;
    if count > remaining / item_size {
        return Err(RuntimeError::storage_error(error));
    }
    Ok(count)
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
    push_storage_header(&mut out, STORAGE_KIND_RESTRICTIONS);
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
    decode_storage_payload(
        bytes,
        StorageKind::Restrictions,
        &[StorageVersionDecoder::current(decode_restrictions_v1)],
    )
}

fn decode_restrictions_v1(bytes: &[u8]) -> Result<Restrictions, RuntimeError> {
    let mut cursor = 0usize;
    let tag = read_u8(bytes, &mut cursor)?;
    let len = read_u32(bytes, &mut cursor)? as usize;
    if len > SOROBAN_MAX_RESTRICTION_ADDRESSES {
        return Err(RuntimeError::storage_error("restrictions too large"));
    }
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
    push_storage_header(&mut out, STORAGE_KIND_SUPPLY_QUEUE);
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
    decode_storage_payload(
        bytes,
        StorageKind::SupplyQueue,
        &[StorageVersionDecoder::current(decode_supply_queue_v1)],
    )
}

fn decode_supply_queue_v1(bytes: &[u8]) -> Result<SupplyQueue, RuntimeError> {
    let mut cursor = 0usize;
    let max_length = read_u32(bytes, &mut cursor)?;
    let count = read_u32(bytes, &mut cursor)? as usize;
    let count = bounded_count_for_fixed_items(bytes, cursor, count, 21, "supply queue too large")?;
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
    push_storage_header(&mut out, STORAGE_KIND_CAP_GROUPS);
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
    decode_storage_payload(
        bytes,
        StorageKind::CapGroups,
        &[StorageVersionDecoder::current(decode_cap_groups_v1)],
    )
}

fn decode_cap_groups_v1(
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
    push_storage_header(&mut out, STORAGE_KIND_MARKETS);
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
    decode_storage_payload(
        bytes,
        StorageKind::Markets,
        &[StorageVersionDecoder::current(decode_markets_v1)],
    )
}

fn decode_markets_v1(bytes: &[u8]) -> Result<OrderedMap<TargetId, MarketConfig>, RuntimeError> {
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
        if markets
            .insert(target_id, MarketConfig::new(enabled, cap, cap_group_id))
            .is_some()
        {
            return Err(RuntimeError::storage_error("duplicate market target"));
        }
    }
    finish_decode(bytes, cursor)?;
    Ok(markets)
}

pub(crate) fn encode_principals(principals: &OrderedMap<TargetId, u128>) -> Vec<u8> {
    let mut out = Vec::new();
    push_storage_header(&mut out, STORAGE_KIND_PRINCIPALS);
    push_u32(&mut out, principals.len() as u32);
    for (target_id, principal) in principals.iter() {
        push_u32(&mut out, *target_id);
        push_u128(&mut out, *principal);
    }
    out
}

pub(crate) fn decode_principals(bytes: &[u8]) -> Result<OrderedMap<TargetId, u128>, RuntimeError> {
    decode_storage_payload(
        bytes,
        StorageKind::Principals,
        &[StorageVersionDecoder::current(decode_principals_v1)],
    )
}

fn decode_principals_v1(bytes: &[u8]) -> Result<OrderedMap<TargetId, u128>, RuntimeError> {
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
    push_storage_header(&mut out, STORAGE_KIND_POLICY_LOCKS);
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
    decode_storage_payload(
        bytes,
        StorageKind::PolicyLocks,
        &[StorageVersionDecoder::current(decode_policy_locks_v1)],
    )
}

fn decode_policy_locks_v1(bytes: &[u8]) -> Result<MarketLeaseRegistry, RuntimeError> {
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

#[derive(Clone, Copy)]
struct WithdrawQueueHeader {
    next_withdraw_to_execute: u64,
    next_pending_withdrawal_id: u64,
    pending_count: u32,
}
struct StoredStateHeader {
    total_assets: u128,
    total_shares: u128,
    idle_assets: u128,
    external_assets: u128,
    fee_anchor: FeeAccrualAnchor,
    op_state: OpState,
    withdraw_queue: WithdrawQueueHeader,
    epoch: EpochState,
    next_op_id: u64,
}

fn queue_page_id(withdrawal_id: u64) -> u64 {
    withdrawal_id / WITHDRAW_QUEUE_PAGE_SIZE
}

fn queue_page_range(queue: &WithdrawQueue) -> Option<(u64, u64)> {
    if queue.is_empty() {
        return None;
    }
    let tail_id = queue.next_pending_withdrawal_id.checked_sub(1)?;
    Some((
        queue_page_id(queue.next_withdraw_to_execute),
        queue_page_id(tail_id),
    ))
}

fn queue_header_page_range(header: WithdrawQueueHeader) -> Option<(u64, u64)> {
    if header.next_withdraw_to_execute >= header.next_pending_withdrawal_id {
        return None;
    }
    let tail_id = header.next_pending_withdrawal_id.checked_sub(1)?;
    Some((
        queue_page_id(header.next_withdraw_to_execute),
        queue_page_id(tail_id),
    ))
}

fn encode_withdraw_queue_header(queue: &WithdrawQueue, out: &mut Vec<u8>) {
    push_u64(out, queue.next_withdraw_to_execute);
    push_u64(out, queue.next_pending_withdrawal_id);
    push_u32(out, queue.len() as u32);
}

fn decode_withdraw_queue_header(
    bytes: &[u8],
    cursor: &mut usize,
) -> Result<WithdrawQueueHeader, RuntimeError> {
    let header = WithdrawQueueHeader {
        next_withdraw_to_execute: read_u64(bytes, cursor)?,
        next_pending_withdrawal_id: read_u64(bytes, cursor)?,
        pending_count: read_u32(bytes, cursor)?,
    };
    if header.next_withdraw_to_execute > header.next_pending_withdrawal_id {
        return Err(RuntimeError::storage_error("withdraw queue invalid ids"));
    }
    if header.pending_count > SOROBAN_MAX_PENDING_WITHDRAWALS {
        return Err(RuntimeError::storage_error(
            "withdraw queue exceeds soroban cap",
        ));
    }
    if header.pending_count == 0
        && header.next_withdraw_to_execute != header.next_pending_withdrawal_id
    {
        return Err(RuntimeError::storage_error(
            "empty withdraw queue invalid ids",
        ));
    }
    if header.pending_count > 0
        && header.next_withdraw_to_execute >= header.next_pending_withdrawal_id
    {
        return Err(RuntimeError::storage_error(
            "non-empty withdraw queue invalid ids",
        ));
    }
    Ok(header)
}

pub(crate) fn encode_withdraw_queue_page<'a>(
    entries: impl IntoIterator<Item = (u64, &'a PendingWithdrawal)>,
) -> Vec<u8> {
    let entries: Vec<_> = entries.into_iter().collect();
    let mut out = Vec::new();
    out.extend_from_slice(&WITHDRAW_PAGE_MAGIC);
    push_u8(&mut out, WITHDRAW_PAGE_FORMAT_EPOCH);
    push_u32(&mut out, entries.len() as u32);
    for (id, withdrawal) in entries {
        push_u64(&mut out, id);
        push_address(&mut out, &withdrawal.owner);
        push_address(&mut out, &withdrawal.receiver);
        push_u128(&mut out, withdrawal.escrow_shares);
        push_u128(&mut out, withdrawal.min_assets_out);
        push_u64(&mut out, withdrawal.requested_at_ns.as_u64());
        push_u64(&mut out, withdrawal.epoch_id.as_u64());
    }
    out
}

pub(crate) fn decode_withdraw_queue_page(
    bytes: &[u8],
) -> Result<Vec<(u64, PendingWithdrawal)>, RuntimeError> {
    if bytes.len() >= 4 && bytes[..3] == WITHDRAW_PAGE_MAGIC {
        return decode_withdraw_queue_page_epoch(bytes);
    }
    // Dedicated epoch deployments are fresh: pre-epoch (immediate-style)
    // queue pages can never legally exist in their storage. Reading one
    // fails closed instead of migrating an immediate vault's queue into a
    // fresh epoch contract.
    #[cfg(feature = "immediate-entrypoints")]
    {
        return decode_withdraw_queue_page_legacy(bytes);
    }
    #[cfg(not(feature = "immediate-entrypoints"))]
    {
        Err(RuntimeError::storage_error("unsupported withdraw page format"))
    }
}

fn decode_withdraw_queue_page_epoch(
    bytes: &[u8],
) -> Result<Vec<(u64, PendingWithdrawal)>, RuntimeError> {
    let mut cursor = 3usize;
    if read_u8(bytes, &mut cursor)? != WITHDRAW_PAGE_FORMAT_EPOCH {
        return Err(RuntimeError::storage_error("unsupported withdraw page format"));
    }
    let count = read_u32(bytes, &mut cursor)? as usize;
    if count > WITHDRAW_QUEUE_PAGE_SIZE as usize {
        return Err(RuntimeError::storage_error("withdraw queue page too large"));
    }
    let count = bounded_count_for_fixed_items(
        bytes,
        cursor,
        count,
        WITHDRAW_PAGE_ITEM_BYTES,
        "withdraw queue page too large",
    )?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let id = read_u64(bytes, &mut cursor)?;
        let owner = read_address(bytes, &mut cursor)?;
        let receiver = read_address(bytes, &mut cursor)?;
        let escrow_shares = read_u128(bytes, &mut cursor)?;
        let min_assets_out = read_u128(bytes, &mut cursor)?;
        let requested_at_ns = templar_vault_kernel::TimestampNs(read_u64(bytes, &mut cursor)?);
        let epoch_id = EpochId::new(read_u64(bytes, &mut cursor)?);
        let withdrawal = if epoch_id == EpochId::MIGRATION_INTAKE {
            if min_assets_out != 0 {
                return Err(RuntimeError::storage_error(
                    "migrated intake carries claim authority",
                ));
            }
            PendingWithdrawal::migrated_legacy(owner, receiver, escrow_shares, requested_at_ns)
        } else {
            PendingWithdrawal::new(
                owner,
                receiver,
                escrow_shares,
                min_assets_out,
                requested_at_ns,
                epoch_id,
            )
        }
        .map_err(|_| RuntimeError::storage_error("withdraw queue page entry invalid"))?;
        if withdrawal.min_assets_out != min_assets_out || withdrawal.epoch_id != epoch_id {
            return Err(RuntimeError::storage_error("withdraw queue page entry mismatch"));
        }
        entries.push((id, withdrawal));
    }
    finish_decode(bytes, cursor)?;
    Ok(entries)
}

#[cfg(feature = "immediate-entrypoints")]
fn decode_withdraw_queue_page_legacy(
    bytes: &[u8],
) -> Result<Vec<(u64, PendingWithdrawal)>, RuntimeError> {
    let mut cursor = 0usize;
    let count = read_u32(bytes, &mut cursor)? as usize;
    if count > WITHDRAW_QUEUE_PAGE_SIZE as usize {
        return Err(RuntimeError::storage_error("withdraw queue page too large"));
    }
    let count = bounded_count_for_fixed_items(
        bytes,
        cursor,
        count,
        LEGACY_WITHDRAW_PAGE_ITEM_BYTES,
        "withdraw queue page too large",
    )?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let id = read_u64(bytes, &mut cursor)?;
        let owner = read_address(bytes, &mut cursor)?;
        let receiver = read_address(bytes, &mut cursor)?;
        let escrow_shares = read_u128(bytes, &mut cursor)?;
        // The legacy fixed asset claim is dropped on migration: pending
        // intake must not carry pre-settlement claim authority.
        let _legacy_expected_assets = read_u128(bytes, &mut cursor)?;
        let requested_at_ns = templar_vault_kernel::TimestampNs(read_u64(bytes, &mut cursor)?);
        let withdrawal =
            PendingWithdrawal::migrated_legacy(owner, receiver, escrow_shares, requested_at_ns)
                .map_err(|_| RuntimeError::storage_error("withdraw queue page entry invalid"))?;
        entries.push((id, withdrawal));
    }
    finish_decode(bytes, cursor)?;
    Ok(entries)
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
            let count = bounded_count_for_fixed_items(
                bytes,
                *cursor,
                count,
                20,
                "allocation plan too large",
            )?;
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
            let count =
                bounded_count_for_fixed_items(bytes, *cursor, count, 4, "refresh plan too large")?;
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

fn encode_state_header_blob(state: &VaultState) -> Vec<u8> {
    let mut out = Vec::new();
    push_storage_header_version(&mut out, STORAGE_KIND_STATE, STORAGE_VERSION_STATE_EPOCH);
    push_u128(&mut out, state.total_assets);
    push_u128(&mut out, state.total_shares);
    push_u128(&mut out, state.idle_assets);
    push_u128(&mut out, state.external_assets);
    push_u128(&mut out, state.fee_anchor.total_assets);
    push_u64(&mut out, state.fee_anchor.timestamp_ns.as_u64());
    encode_op_state(&state.op_state, &mut out);
    encode_withdraw_queue_header(&state.withdraw_queue, &mut out);
    push_u64(&mut out, state.next_op_id);
    encode_epoch_state(&state.epoch, &mut out);
    out
}

/// Encode the epoch lifecycle law fields: phase, monotonic intake epoch,
/// cutoff binding, and the immutable last-settled snapshot. Persisting the
/// snapshot is what makes accepted-report replay across settlement
/// impossible: settlement only advances through [`EpochState::apply_settled`]
/// against this stored state.
pub(crate) fn encode_epoch_state(epoch: &EpochState, out: &mut Vec<u8>) {
    let phase = match epoch.phase {
        EpochPhase::Open => 0u8,
        EpochPhase::Cutoff => 1u8,
        EpochPhase::Settled => 2u8,
    };
    push_u8(out, phase);
    push_u64(out, epoch.intake_epoch.as_u64());
    match epoch.cutoff_ns {
        None => push_u8(out, 0),
        Some(cutoff_ns) => {
            push_u8(out, 1);
            push_u64(out, cutoff_ns.as_u64());
        }
    }
    match &epoch.last_settled {
        None => push_u8(out, 0),
        Some(snapshot) => {
            push_u8(out, 1);
            push_u64(out, snapshot.epoch_id().as_u64());
            push_u64(out, snapshot.report_seq());
            out.extend_from_slice(snapshot.report_hash());
            push_u64(out, snapshot.as_of_ns().as_u64());
            push_u128(out, snapshot.settlement_nav());
            push_u128(out, snapshot.eligible_supply());
            push_u64(out, snapshot.cutoff_ns().as_u64());
        }
    }
}

pub(crate) fn decode_epoch_state(bytes: &[u8], cursor: &mut usize) -> Result<EpochState, RuntimeError> {
    let phase = match read_u8(bytes, cursor)? {
        0 => EpochPhase::Open,
        1 => EpochPhase::Cutoff,
        2 => EpochPhase::Settled,
        _ => return Err(RuntimeError::storage_error("epoch phase invalid")),
    };
    let intake_epoch = EpochId::new(read_u64(bytes, cursor)?);
    let cutoff_ns = match read_u8(bytes, cursor)? {
        0 => None,
        1 => Some(templar_vault_kernel::TimestampNs(read_u64(bytes, cursor)?)),
        _ => return Err(RuntimeError::storage_error("epoch cutoff flag invalid")),
    };
    let last_settled = match read_u8(bytes, cursor)? {
        0 => None,
        1 => {
            let epoch_id = EpochId::new(read_u64(bytes, cursor)?);
            let report_seq = read_u64(bytes, cursor)?;
            let hash_bytes = read_exact(bytes, cursor, 32)?;
            let mut report_hash = [0u8; 32];
            report_hash.copy_from_slice(hash_bytes);
            let as_of_ns = templar_vault_kernel::TimestampNs(read_u64(bytes, cursor)?);
            let settlement_nav = read_u128(bytes, cursor)?;
            let eligible_supply = read_u128(bytes, cursor)?;
            let cutoff_ns = templar_vault_kernel::TimestampNs(read_u64(bytes, cursor)?);
            let snapshot = EpochSnapshot::bind(
                epoch_id,
                cutoff_ns,
                &ValuationReportRef {
                    report_seq,
                    as_of_ns,
                    report_hash,
                },
                settlement_nav,
                eligible_supply,
            )
            .map_err(|_| RuntimeError::storage_error("epoch snapshot invalid"))?;
            if snapshot.epoch_id() != epoch_id
                || snapshot.report_seq() != report_seq
                || snapshot.report_hash() != &report_hash
                || snapshot.as_of_ns() != as_of_ns
                || snapshot.settlement_nav() != settlement_nav
                || snapshot.eligible_supply() != eligible_supply
                || snapshot.cutoff_ns() != cutoff_ns
            {
                return Err(RuntimeError::storage_error("epoch snapshot mismatch"));
            }
            Some(snapshot)
        }
        _ => return Err(RuntimeError::storage_error("epoch snapshot flag invalid")),
    };
    let epoch = EpochState {
        phase,
        intake_epoch,
        cutoff_ns,
        last_settled,
    };
    if epoch.check_invariants() {
        Ok(epoch)
    } else {
        Err(RuntimeError::storage_error("epoch state invariant failed"))
    }
}

fn decode_state_header_blob(bytes: &[u8]) -> Result<StoredStateHeader, RuntimeError> {
    let (version, payload) = storage_payload(bytes, StorageKind::State)?;
    match version {
        // Dedicated epoch deployments are fresh: pre-epoch (v2) vault-state
        // headers can never legally exist in their storage. Reading one
        // fails closed instead of migrating an immediate vault's header
        // into a fresh epoch contract.
        #[cfg(feature = "immediate-entrypoints")]
        STORAGE_VERSION_STATE_PAGED => decode_state_header_blob_v2(payload),
        // Current storage law: paged header plus the epoch lifecycle tail.
        STORAGE_VERSION_STATE_EPOCH => decode_state_header_blob_v3(payload),
        _ => Err(RuntimeError::storage_error(
            "unsupported state storage version",
        )),
    }
}

/// The epoch lifecycle state mirrored into a stored vault-state header.
/// The header and the dedicated epoch record are two views of the same law:
/// a divergent view fails closed and never becomes a second epoch authority.
pub(crate) fn epoch_state_from_state_header_blob(
    bytes: &[u8],
) -> Result<EpochState, RuntimeError> {
    // The vault-state header may be stored inline behind the blob envelope
    // or supplied already unwrapped. Both views must resolve to the same
    // state header record before the epoch mirror is read; a corrupt or
    // paged envelope fails closed rather than inventing authority.
    let inner = match decode_blob_manifest(bytes)? {
        Some(BlobManifest::Inline) => {
            let mut cursor = 5usize;
            let len = read_u32(bytes, &mut cursor)? as usize;
            if bytes.len().saturating_sub(cursor) != len {
                return Err(RuntimeError::storage_error("invalid inline state header blob"));
            }
            &bytes[cursor..cursor + len]
        }
        Some(BlobManifest::Paged { .. }) => {
            return Err(RuntimeError::storage_error(
                "paged state header blob requires contract context",
            ));
        }
        None => bytes,
    };
    Ok(decode_state_header_blob(inner)?.epoch)
}

#[cfg(feature = "immediate-entrypoints")]
fn decode_state_header_blob_v2(bytes: &[u8]) -> Result<StoredStateHeader, RuntimeError> {
    let header = read_state_header_fields(bytes)?;
    Ok(StoredStateHeader {
        epoch: EpochState::genesis(),
        ..header
    })
}

fn decode_state_header_blob_v3(bytes: &[u8]) -> Result<StoredStateHeader, RuntimeError> {
    let mut cursor = 0usize;
    let total_assets = read_u128(bytes, &mut cursor)?;
    let total_shares = read_u128(bytes, &mut cursor)?;
    let idle_assets = read_u128(bytes, &mut cursor)?;
    let external_assets = read_u128(bytes, &mut cursor)?;
    let fee_anchor = FeeAccrualAnchor::new(
        read_u128(bytes, &mut cursor)?,
        templar_vault_kernel::TimestampNs(read_u64(bytes, &mut cursor)?),
    );
    let op_state = decode_op_state(bytes, &mut cursor)?;
    let withdraw_queue = decode_withdraw_queue_header(bytes, &mut cursor)?;
    let next_op_id = read_u64(bytes, &mut cursor)?;
    let epoch = decode_epoch_state(bytes, &mut cursor)?;
    finish_decode(bytes, cursor)?;
    Ok(StoredStateHeader {
        total_assets,
        total_shares,
        idle_assets,
        external_assets,
        fee_anchor,
        op_state,
        withdraw_queue,
        epoch,
        next_op_id,
    })
}

#[cfg(feature = "immediate-entrypoints")]
fn read_state_header_fields(bytes: &[u8]) -> Result<StoredStateHeader, RuntimeError> {
    let mut cursor = 0usize;
    let header = StoredStateHeader {
        total_assets: read_u128(bytes, &mut cursor)?,
        total_shares: read_u128(bytes, &mut cursor)?,
        idle_assets: read_u128(bytes, &mut cursor)?,
        external_assets: read_u128(bytes, &mut cursor)?,
        fee_anchor: FeeAccrualAnchor::new(
            read_u128(bytes, &mut cursor)?,
            templar_vault_kernel::TimestampNs(read_u64(bytes, &mut cursor)?),
        ),
        op_state: decode_op_state(bytes, &mut cursor)?,
        withdraw_queue: decode_withdraw_queue_header(bytes, &mut cursor)?,
        epoch: EpochState::genesis(),
        next_op_id: read_u64(bytes, &mut cursor)?,
    };
    finish_decode(bytes, cursor)?;
    Ok(header)
}
fn compose_state_from_header(
    header: StoredStateHeader,
    withdraw_queue: WithdrawQueue,
) -> Result<VaultState, RuntimeError> {
    if withdraw_queue.next_withdraw_to_execute != header.withdraw_queue.next_withdraw_to_execute
        || withdraw_queue.next_pending_withdrawal_id
            != header.withdraw_queue.next_pending_withdrawal_id
        || withdraw_queue.len() as u32 != header.withdraw_queue.pending_count
        || !withdraw_queue.check_invariants_with_max(SOROBAN_MAX_PENDING_WITHDRAWALS)
    {
        return Err(RuntimeError::storage_error(
            "withdraw queue pages do not match state header",
        ));
    }
    if !header.epoch.check_invariants() {
        return Err(RuntimeError::storage_error("epoch state invariant failed"));
    }
    Ok(VaultState {
        total_assets: header.total_assets,
        total_shares: header.total_shares,
        idle_assets: header.idle_assets,
        external_assets: header.external_assets,
        fee_anchor: header.fee_anchor,
        op_state: header.op_state,
        withdraw_queue,
        epoch: header.epoch,
        next_op_id: header.next_op_id,
    })
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

    let (Some(markets), Some(principals), Some(cap_groups), Some(leases), Some(supply_queue)) =
        (markets, principals, cap_groups, leases, supply_queue)
    else {
        return Err(RuntimeError::storage_error("partial policy state"));
    };

    let state = PolicyState::from_parts(markets, principals, cap_groups, leases, supply_queue)
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
    const SK_BLOBPAGE: Symbol = symbol_short!("blobpage");
    const SK_WQPAGE: Symbol = symbol_short!("wqpage");
    const SK_EPOCHSTATE: Symbol = symbol_short!("epochst");
    const SK_ACCEPTEDRPT: Symbol = symbol_short!("accrep");
    const SK_MAXRPTAGE: Symbol = symbol_short!("maxrage");

    fn versioned_record_key(kind: StorageKind) -> Option<Symbol> {
        match kind {
            StorageKind::EpochState => Some(Self::SK_EPOCHSTATE),
            StorageKind::AcceptedReport => Some(Self::SK_ACCEPTEDRPT),
            StorageKind::MaxReportAge => Some(Self::SK_MAXRPTAGE),
            _ => None,
        }
    }

    /// Persist a versioned law record (epoch lifecycle state, accepted
    /// valuation report tracking, or governance configuration) under the
    /// kind's dedicated persistent key. The record envelope must carry the
    /// current storage version and a payload within the law-record size
    /// bound, so a corrupt or oversized record can never be written.
    pub(crate) fn save_versioned(
        &self,
        kind: StorageKind,
        record: &[u8],
    ) -> Result<(), RuntimeError> {
        let Some(key) = Self::versioned_record_key(kind) else {
            return Err(RuntimeError::storage_error("unversioned storage kind"));
        };
        if record.len() > MAX_VERSIONED_RECORD_BYTES {
            return Err(RuntimeError::storage_error(
                "versioned law record exceeds size bound",
            ));
        }
        let (version, payload) = storage_payload(record, kind)?;
        if version != STORAGE_VERSION_CURRENT {
            return Err(RuntimeError::storage_error(
                "versioned law record storage version invalid",
            ));
        }
        if payload.is_empty() {
            return Err(RuntimeError::storage_error(
                "versioned law record payload empty",
            ));
        }
        self.save_blob(&key, record)?;
        self.extend_blob_ttl(&key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
        Ok(())
    }

    /// Load a versioned law record, returning `None` when the kind has no
    /// stored record. Any present record is validated against its envelope
    /// and size bound; corruption fails closed, never silently absent.
    pub(crate) fn load_versioned(
        &self,
        kind: StorageKind,
    ) -> Result<Option<Vec<u8>>, RuntimeError> {
        let Some(key) = Self::versioned_record_key(kind) else {
            return Err(RuntimeError::storage_error("unversioned storage kind"));
        };
        let Some(record) = self.load_blob(&key)? else {
            return Ok(None);
        };
        if record.len() > MAX_VERSIONED_RECORD_BYTES {
            return Err(RuntimeError::storage_error(
                "versioned law record exceeds size bound",
            ));
        }
        let (version, payload) = storage_payload(&record, kind)?;
        if version != STORAGE_VERSION_CURRENT {
            return Err(RuntimeError::storage_error(
                "versioned law record storage version invalid",
            ));
        }
        if payload.is_empty() {
            return Err(RuntimeError::storage_error(
                "versioned law record payload empty",
            ));
        }
        Ok(Some(record))
    }


    fn address_key(&self, kernel_addr: &Address) -> (Symbol, BytesN<32>) {
        (
            Self::SK_ADDRBOOK,
            BytesN::from_array(self.env, kernel_addr.as_bytes()),
        )
    }

    fn blob_page_key(&self, key: &Symbol, page: u32) -> (Symbol, Symbol, u32) {
        (Self::SK_BLOBPAGE, key.clone(), page)
    }

    fn withdraw_queue_page_key(&self, page: u64) -> (Symbol, u64) {
        (Self::SK_WQPAGE, page)
    }

    fn stored_blob_manifest(&self, key: &Symbol) -> Result<Option<BlobManifest>, RuntimeError> {
        let Some(bytes) = self.env.storage().persistent().get::<_, Bytes>(key) else {
            return Ok(None);
        };
        decode_blob_manifest(&bytes.to_alloc_vec())
    }

    fn load_blob(&self, key: &Symbol) -> Result<Option<Vec<u8>>, RuntimeError> {
        let Some(stored) = self.env.storage().persistent().get::<_, Bytes>(key) else {
            return Ok(None);
        };
        let stored = stored.to_alloc_vec();
        match decode_blob_manifest(&stored)? {
            Some(BlobManifest::Inline) => {
                let mut cursor = 5usize;
                let len = read_u32(&stored, &mut cursor)? as usize;
                Ok(Some(stored[cursor..cursor + len].to_vec()))
            }
            Some(BlobManifest::Paged { len, page_count }) => {
                let mut out = Vec::with_capacity(len.min(BLOB_PAGE_BYTES));
                let p = self.env.storage().persistent();
                for page in 0..page_count {
                    let page_key = self.blob_page_key(key, page);
                    let page_bytes = p
                        .get::<_, Bytes>(&page_key)
                        .ok_or_else(|| RuntimeError::storage_error("missing blob page"))?
                        .to_alloc_vec();
                    if page_bytes.len() > BLOB_PAGE_BYTES {
                        return Err(RuntimeError::storage_error("blob page too large"));
                    }
                    out.extend_from_slice(&page_bytes);
                }
                if out.len() != len {
                    return Err(RuntimeError::storage_error("invalid paged blob length"));
                }
                Ok(Some(out))
            }
            None => Err(RuntimeError::storage_error("invalid blob storage envelope")),
        }
    }

    fn save_blob(&self, key: &Symbol, bytes: &[u8]) -> Result<(), RuntimeError> {
        let previous_page_count = match self.stored_blob_manifest(key)? {
            Some(BlobManifest::Paged { page_count, .. }) => page_count,
            _ => 0,
        };

        let p = self.env.storage().persistent();
        if bytes.len() <= BLOB_PAGE_BYTES {
            let stored = encode_blob_inline(bytes);
            ensure_contract_data_entry_size(&stored)?;
            p.set(key, &Bytes::from_slice(self.env, &stored));
            for page in 0..previous_page_count {
                p.remove(&self.blob_page_key(key, page));
            }
            return Ok(());
        }

        let page_count = bytes.len().div_ceil(BLOB_PAGE_BYTES) as u32;
        let manifest = encode_blob_manifest(bytes.len(), page_count);
        ensure_contract_data_entry_size(&manifest)?;
        for (page, chunk) in bytes.chunks(BLOB_PAGE_BYTES).enumerate() {
            let page_key = self.blob_page_key(key, page as u32);
            p.set(&page_key, &Bytes::from_slice(self.env, chunk));
        }
        p.set(key, &Bytes::from_slice(self.env, &manifest));
        for page in page_count..previous_page_count {
            p.remove(&self.blob_page_key(key, page));
        }
        Ok(())
    }

    fn clear_blob(&self, key: &Symbol) {
        let page_count = match self.stored_blob_manifest(key) {
            Ok(Some(BlobManifest::Paged { page_count, .. })) => page_count,
            _ => 0,
        };
        let p = self.env.storage().persistent();
        p.remove(key);
        for page in 0..page_count {
            p.remove(&self.blob_page_key(key, page));
        }
    }

    fn extend_blob_ttl(&self, key: &Symbol, threshold: u32, extend_to: u32) {
        let p = self.env.storage().persistent();
        if !p.has(key) {
            return;
        }
        p.extend_ttl(key, threshold, extend_to);
        if let Ok(Some(BlobManifest::Paged { page_count, .. })) = self.stored_blob_manifest(key) {
            for page in 0..page_count {
                let page_key = self.blob_page_key(key, page);
                if p.has(&page_key) {
                    p.extend_ttl(&page_key, threshold, extend_to);
                }
            }
        }
    }

    fn load_withdraw_queue_page(
        &self,
        page: u64,
    ) -> Result<Option<Vec<(u64, PendingWithdrawal)>>, RuntimeError> {
        let key = self.withdraw_queue_page_key(page);
        self.env
            .storage()
            .persistent()
            .get::<_, Bytes>(&key)
            .map(|bytes| decode_withdraw_queue_page(&bytes.to_alloc_vec()))
            .transpose()
    }

    fn load_withdraw_queue_pages(
        &self,
        header: WithdrawQueueHeader,
    ) -> Result<WithdrawQueue, RuntimeError> {
        let Some((first_page, last_page)) = queue_header_page_range(header) else {
            return Ok(WithdrawQueue::with_state(
                Vec::<(u64, PendingWithdrawal)>::new(),
                header.next_pending_withdrawal_id,
                header.next_pending_withdrawal_id,
            ));
        };

        let mut entries = Vec::new();
        let mut last_id = None;
        for page in first_page..=last_page {
            if let Some(page_entries) = self.load_withdraw_queue_page(page)? {
                for (id, _) in &page_entries {
                    if queue_page_id(*id) != page
                        || *id < header.next_withdraw_to_execute
                        || *id >= header.next_pending_withdrawal_id
                        || last_id.is_some_and(|last| last >= *id)
                    {
                        return Err(RuntimeError::storage_error(
                            "withdraw queue page entries invalid",
                        ));
                    }
                    last_id = Some(*id);
                }
                entries.extend(page_entries);
            }
        }
        Ok(WithdrawQueue::with_state(
            entries,
            header.next_withdraw_to_execute,
            header.next_pending_withdrawal_id,
        ))
    }

    fn save_withdraw_queue_pages(&self, queue: &WithdrawQueue) -> Result<(), RuntimeError> {
        let previous_range = self
            .load_state_blob()?
            .and_then(|stored| decode_state_header_blob(&stored).ok())
            .and_then(|header| queue_header_page_range(header.withdraw_queue));

        let current_range = queue_page_range(queue);
        let p = self.env.storage().persistent();

        if let Some((first_page, last_page)) = current_range {
            for page in first_page..=last_page {
                let entries = queue
                    .iter()
                    .filter(|(id, _)| queue_page_id(*id) == page)
                    .collect::<Vec<_>>();
                let key = self.withdraw_queue_page_key(page);
                if entries.is_empty() {
                    p.remove(&key);
                    continue;
                }
                let encoded = encode_withdraw_queue_page(entries);
                ensure_contract_data_entry_size(&encoded)?;
                let current = p.get::<_, Bytes>(&key).map(|bytes| bytes.to_alloc_vec());
                if current.as_deref() != Some(encoded.as_slice()) {
                    p.set(&key, &Bytes::from_slice(self.env, &encoded));
                }
            }
        }

        if let Some((first_page, last_page)) = previous_range {
            for page in first_page..=last_page {
                let still_live = current_range
                    .map(|(first, last)| page >= first && page <= last)
                    .unwrap_or(false);
                if !still_live {
                    p.remove(&self.withdraw_queue_page_key(page));
                }
            }
        }

        Ok(())
    }

    fn extend_withdraw_queue_page_ttls(
        &self,
        header: WithdrawQueueHeader,
        threshold: u32,
        extend_to: u32,
    ) {
        let Some((first_page, last_page)) = queue_header_page_range(header) else {
            return;
        };
        let p = self.env.storage().persistent();
        for page in first_page..=last_page {
            let key = self.withdraw_queue_page_key(page);
            if p.has(&key) {
                p.extend_ttl(&key, threshold, extend_to);
            }
        }
    }

    /// Load a kernel-to-Soroban address mapping from persistent storage.
    pub fn load_address(&self, kernel_addr: &Address) -> Option<SdkAddress> {
        let key = self.address_key(kernel_addr);
        self.env.storage().persistent().get(&key)
    }

    /// Save a kernel-to-Soroban address mapping to persistent storage.
    pub fn save_address(
        &self,
        kernel_addr: &Address,
        soroban_addr: &SdkAddress,
    ) -> Result<(), RuntimeError> {
        let key = self.address_key(kernel_addr);
        self.env.storage().persistent().set(&key, soroban_addr);
        self.env.storage().persistent().extend_ttl(
            &key,
            DEFAULT_TTL_THRESHOLD,
            DEFAULT_TTL_EXTEND_TO,
        );
        self.extend_default_ttl();
        Ok(())
    }

    pub(crate) fn extend_address_ttl(&self, kernel_addr: &Address, threshold: u32, extend_to: u32) {
        let key = self.address_key(kernel_addr);
        let p = self.env.storage().persistent();
        if p.has(&key) {
            p.extend_ttl(&key, threshold, extend_to);
        }
    }

    fn extend_state_address_ttls(&self, state: &VaultState, threshold: u32, extend_to: u32) {
        for (_, withdrawal) in state.withdraw_queue.iter() {
            self.extend_address_ttl(&withdrawal.owner, threshold, extend_to);
            self.extend_address_ttl(&withdrawal.receiver, threshold, extend_to);
        }
        match &state.op_state {
            OpState::Withdrawing(withdrawing) => {
                self.extend_address_ttl(&withdrawing.owner, threshold, extend_to);
                self.extend_address_ttl(&withdrawing.receiver, threshold, extend_to);
            }
            OpState::Payout(payout) => {
                self.extend_address_ttl(&payout.owner, threshold, extend_to);
                self.extend_address_ttl(&payout.receiver, threshold, extend_to);
            }
            _ => {}
        }
    }

    pub(crate) fn load_state_blob(&self) -> Result<Option<Vec<u8>>, RuntimeError> {
        self.load_blob(&SorobanStorageKey::StateBlob)
    }

    pub(crate) fn save_state_blob(&self, state: &[u8]) -> Result<(), RuntimeError> {
        self.save_blob(&SorobanStorageKey::StateBlob, state)
    }

    pub fn load_policy_locks(&self) -> Result<Option<Vec<u8>>, RuntimeError> {
        self.load_blob(&SorobanStorageKey::PolicyLocks)
    }

    pub fn save_policy_locks(&self, state: &[u8]) -> Result<(), RuntimeError> {
        self.save_blob(&SorobanStorageKey::PolicyLocks, state)
    }

    pub fn load_policy_supply_queue(&self) -> Result<Option<Vec<u8>>, RuntimeError> {
        self.load_blob(&SorobanStorageKey::PolicySupplyQueue)
    }

    pub fn save_policy_supply_queue(&self, state: &[u8]) -> Result<(), RuntimeError> {
        self.save_blob(&SorobanStorageKey::PolicySupplyQueue, state)
    }

    pub fn load_policy_markets(&self) -> Result<Option<Vec<u8>>, RuntimeError> {
        self.load_blob(&SorobanStorageKey::PolicyMarkets)
    }

    pub fn save_policy_markets(&self, state: &[u8]) -> Result<(), RuntimeError> {
        self.save_blob(&SorobanStorageKey::PolicyMarkets, state)
    }

    pub fn load_policy_principals(&self) -> Result<Option<Vec<u8>>, RuntimeError> {
        self.load_blob(&SorobanStorageKey::PolicyPrincipals)
    }

    pub fn save_policy_principals(&self, state: &[u8]) -> Result<(), RuntimeError> {
        self.save_blob(&SorobanStorageKey::PolicyPrincipals, state)
    }

    pub fn load_policy_cap_groups(&self) -> Result<Option<Vec<u8>>, RuntimeError> {
        self.load_blob(&SorobanStorageKey::PolicyCapGroups)
    }

    pub fn save_policy_cap_groups(&self, state: &[u8]) -> Result<(), RuntimeError> {
        self.save_blob(&SorobanStorageKey::PolicyCapGroups, state)
    }

    /// Enumerate every persisted market adapter binding in ascending
    /// `TargetId` order.
    ///
    /// This is the single authoritative enumeration used by epoch
    /// settlement: the binding map persisted by governance is the only
    /// source, never a policy-state-only reconstruction. It fails closed
    /// when either the binding record or the markets record is absent or
    /// corrupt, rejects any binding whose target is not an exposed market,
    /// rejects any binding count that exceeds the exposed-market bound, and
    /// rejects duplicate binding targets. Callers must consume the returned
    /// list in order and read authenticated valuation metadata from each
    /// bound adapter; the list is derived only from authenticated ledger
    /// storage, never from caller input.
    pub fn enumerate_market_adapter_bindings(
        &self,
    ) -> Result<Vec<(TargetId, SdkAddress)>, RuntimeError> {
        let Some(bindings) = self
            .env
            .storage()
            .instance()
            .get::<_, soroban_sdk::Map<u32, SdkAddress>>(
                &crate::contract::VaultDataKey::AdapterBindings,
            )
        else {
            return Err(RuntimeError::storage_error("adapter bindings record missing"));
        };
        let Some(stored_markets) = self.load_policy_markets()? else {
            return Err(RuntimeError::storage_error("markets record missing"));
        };
        let markets = decode_markets(&stored_markets)?;
        if usize::try_from(bindings.len())
            .unwrap_or(usize::MAX)
            > usize::try_from(markets.len()).unwrap_or(usize::MAX)
        {
            return Err(RuntimeError::storage_error("adapter bindings exceed market bound"));
        }
        let mut enumerated = Vec::new();
        for (target_id, adapter) in bindings.iter() {
            if !markets.contains_key(&target_id) {
                return Err(RuntimeError::storage_error("adapter binding unknown target"));
            }
            enumerated.push((target_id, adapter));
        }
        enumerated.sort_unstable_by_key(|(target_id, _)| *target_id);
        if enumerated.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(RuntimeError::storage_error("duplicate adapter binding target"));
        }
        Ok(enumerated)
    }

    /// Load restrictions from persistent storage.
    pub fn load_restrictions(&self) -> Result<Option<Vec<u8>>, RuntimeError> {
        self.load_blob(&SorobanStorageKey::Restrictions)
    }

    /// Save restrictions to persistent storage.
    pub fn save_restrictions(&self, restrictions: &[u8]) -> Result<(), RuntimeError> {
        self.save_blob(&SorobanStorageKey::Restrictions, restrictions)
    }

    /// Clear restrictions from persistent storage.
    pub fn clear_restrictions(&self) {
        self.clear_blob(&SorobanStorageKey::Restrictions);
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
            self.extend_blob_ttl(key, threshold, extend_to);
        }
        self.extend_blob_ttl(&Self::SK_EPOCHSTATE, threshold, extend_to);
        self.extend_blob_ttl(&Self::SK_ACCEPTEDRPT, threshold, extend_to);
        self.extend_blob_ttl(&Self::SK_MAXRPTAGE, threshold, extend_to);
        if let Ok(Some(stored)) = self.load_state_blob() {
            if let Ok(header) = decode_state_header_blob(&stored) {
                self.extend_withdraw_queue_page_ttls(header.withdraw_queue, threshold, extend_to);
                if let Ok(queue) = self.load_withdraw_queue_pages(header.withdraw_queue) {
                    if let Ok(state) = compose_state_from_header(header, queue) {
                        self.extend_state_address_ttls(&state, threshold, extend_to);
                    }
                }
            }
        }
    }
    fn extend_default_ttl(&self) {
        self.extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
    }
}

impl Storage for SorobanStorage<'_> {
    fn load_state(&self) -> Result<Option<VaultState>, RuntimeError> {
        if let Some(stored) = self.load_state_blob()? {
            let header = decode_state_header_blob(&stored)?;
            let withdraw_queue = self.load_withdraw_queue_pages(header.withdraw_queue)?;
            return Ok(Some(compose_state_from_header(header, withdraw_queue)?));
        }

        Ok(None)
    }

    fn save_state(&mut self, state: &VaultState) -> Result<(), RuntimeError> {
        if !state
            .withdraw_queue
            .check_invariants_with_max(SOROBAN_MAX_PENDING_WITHDRAWALS)
        {
            return Err(RuntimeError::storage_error(
                "withdraw queue exceeds soroban cap",
            ));
        }
        self.save_withdraw_queue_pages(&state.withdraw_queue)?;
        let state_blob = encode_state_header_blob(state);
        self.save_state_blob(&state_blob)?;
        self.save_epoch_state(&state.epoch)?;
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
        let leases = match self.load_policy_locks()? {
            Some(stored) => Some(decode_policy_locks(&stored)?),
            None => None,
        };
        let supply_queue = match self.load_policy_supply_queue()? {
            Some(stored) => Some(decode_supply_queue(&stored)?),
            None => None,
        };
        let markets = match self.load_policy_markets()? {
            Some(stored) => Some(decode_markets(&stored)?),
            None => None,
        };
        let principals = match self.load_policy_principals()? {
            Some(stored) => Some(decode_principals(&stored)?),
            None => None,
        };
        let cap_groups = match self.load_policy_cap_groups()? {
            Some(stored) => Some(decode_cap_groups(&stored)?),
            None => None,
        };

        compose_policy_state(markets, principals, cap_groups, leases, supply_queue)
    }

    fn save_policy_state(&mut self, state: &PolicyState) -> Result<(), RuntimeError> {
        let locks = encode_policy_locks(state.leases());
        let supply_queue = encode_supply_queue(state.supply_queue());
        let markets = encode_markets(state.markets());
        let principals = encode_principals(state.principals());
        let cap_groups = encode_cap_groups(state.cap_groups());
        self.save_policy_locks(&locks)?;
        self.save_policy_supply_queue(&supply_queue)?;
        self.save_policy_markets(&markets)?;
        self.save_policy_principals(&principals)?;
        self.save_policy_cap_groups(&cap_groups)?;
        self.extend_default_ttl();
        Ok(())
    }

    fn load_restrictions(&self) -> Result<Option<Restrictions>, RuntimeError> {
        let restrictions = match SorobanStorage::load_restrictions(self)? {
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
            let len = match restrictions {
                Restrictions::Blacklist(addresses) | Restrictions::Whitelist(addresses) => {
                    addresses.len()
                }
            };
            if len > SOROBAN_MAX_RESTRICTION_ADDRESSES {
                return Err(RuntimeError::storage_error("restrictions too large"));
            }
            let bytes = encode_restrictions(restrictions);
            SorobanStorage::save_restrictions(self, &bytes)?;
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
        SorobanStorage::save_address(self, kernel_addr, soroban_addr)
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
