//! Pending-deposit liability ledger, held OUTSIDE `VaultState` accounting.
//!
//! Pending deposit requests are unadmitted liabilities. They never enter
//! vault idle assets, external assets, total assets, or share supply until
//! epoch settlement admits them at the accepted post-settlement NAV. This
//! ledger stores requests in fixed, versioned pages keyed by monotonic
//! request id, and tracks the aggregate outstanding liability plus the
//! oldest unsettled intake epoch so admission and cutoff can fail closed.
//!
//! Each persisted record pairs the kernel `PendingDeposit` law fields with
//! the depositor's `min_shares_out` floor. The floor is written once with
//! the request and is only ever read back: admission must use the stored
//! floor, never a caller replacement. Removal deletes the floor together
//! with the request, and record presence is the at-most-once admission
//! authority: taking a record IS the consumption proof, so there is no
//! durable admitted marker. A failed transaction rolls back the removal
//! and the record persists for retry.
//!
//! Invariants enforced here:
//! - Request ids are monotonic, never reused, and capped at the kernel
//!   `MAX_PENDING_DEPOSITS` limit over contract lifetime.
//! - Every stored entry is validated through the kernel `PendingDeposit`
//!   law (nonzero assets, settlement-eligible epoch).
//! - Aggregates are recomputed from pages on every read/write and must
//!   match the stored aggregate or the ledger fails closed.

use alloc::vec::Vec;
use soroban_sdk::{contracttype, Bytes};
use templar_vault_kernel::{
    Address, EpochId, EpochPhase, PendingDeposit, TimestampNs, MAX_PENDING_DEPOSITS,
};

use crate::error::RuntimeError;

use super::{
    push_u128, push_u32, push_u64, push_u8, read_exact, read_u128, read_u32, read_u64,
    SorobanStorage, DEFAULT_TTL_EXTEND_TO, DEFAULT_TTL_THRESHOLD,
};

/// Fixed page size for pending deposit pages.
const PENDING_DEPOSIT_PAGE_SIZE: u32 = 32;
/// Maximum pending-deposit pages: bounded by the kernel lifetime cap.
const MAX_PENDING_DEPOSIT_PAGES: u32 =
    (MAX_PENDING_DEPOSITS as u64 / PENDING_DEPOSIT_PAGE_SIZE as u64) as u32;
const STORAGE_KIND_PENDING_DEPOSIT_PAGE: u8 = 10;
/// Storage kind tag for the pending deposit aggregate record.
const STORAGE_KIND_PENDING_DEPOSIT_AGGREGATE: u8 = 11;
/// Deposit page format: every slot carries the persisted share floor.
const PENDING_DEPOSIT_PAGE_FORMAT: u8 = 2;

#[contracttype]
#[derive(Clone, Debug)]
pub struct DepositPageKey {
    pub page: u32,
}

#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingSingletonKey {
    Aggregate,
    NextId,
}

#[allow(non_upper_case_globals)]
pub const AggregateKey: PendingSingletonKey = PendingSingletonKey::Aggregate;
#[allow(non_upper_case_globals)]
pub const NextIdKey: PendingSingletonKey = PendingSingletonKey::NextId;

/// Persisted pending-deposit record.
///
/// Parallels the kernel `PendingDeposit` law fields exactly and adds the
/// depositor's `min_shares_out` floor. There is no update path: the floor
/// travels with the request until removal deletes both together.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct PendingDepositRecord {
    pub owner: Address,
    pub assets: u128,
    pub min_shares_out: u128,
    pub requested_at_ns: TimestampNs,
    pub epoch_id: EpochId,
}

impl PendingDepositRecord {
    /// Build a record from kernel-law fields, re-validating through
    /// [`PendingDeposit::new`] so nothing bypasses the deposit law.
    fn validated(
        owner: Address,
        assets: u128,
        min_shares_out: u128,
        requested_at_ns: TimestampNs,
        epoch_id: EpochId,
    ) -> Result<Self, RuntimeError> {
        PendingDeposit::new(owner, assets, requested_at_ns, epoch_id)
            .map_err(|_| RuntimeError::storage_error("pending deposit law failed"))?;
        Ok(Self {
            owner,
            assets,
            min_shares_out,
            requested_at_ns,
            epoch_id,
        })
    }

    /// Reconstruct the kernel `PendingDeposit` for admission math.
    pub(crate) fn kernel_deposit(&self) -> Result<PendingDeposit, RuntimeError> {
        PendingDeposit::new(
            self.owner,
            self.assets,
            self.requested_at_ns,
            self.epoch_id,
        )
        .map_err(|_| RuntimeError::storage_error("pending deposit law failed"))
    }

    /// The immutable depositor share floor stored with this request.
    pub(crate) fn min_shares_out(&self) -> u128 {
        self.min_shares_out
    }

    const fn absent() -> Self {
        Self {
            owner: Address([0u8; 32]),
            assets: 0,
            min_shares_out: 0,
            requested_at_ns: TimestampNs::ZERO,
            epoch_id: EpochId::new(0),
        }
    }

    fn is_absent(&self) -> bool {
        self.owner == Address([0u8; 32])
            && self.assets == 0
            && self.min_shares_out == 0
            && self.requested_at_ns == TimestampNs::ZERO
            && self.epoch_id == EpochId::new(0)
    }
}

fn request_page_slot(request_id: u64) -> Option<(u32, usize)> {
    if request_id == 0 {
        return None;
    }
    let index = request_id - 1;
    let page = index / u64::from(PENDING_DEPOSIT_PAGE_SIZE);
    if page >= u64::from(MAX_PENDING_DEPOSIT_PAGES) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, reason = "bounded by page size above")]
    let slot = (index % u64::from(PENDING_DEPOSIT_PAGE_SIZE)) as usize;
    Some((page as u32, slot))
}

fn push_record(out: &mut Vec<u8>, record: &PendingDepositRecord) {
    out.extend_from_slice(record.owner.as_bytes());
    push_u128(out, record.assets);
    push_u128(out, record.min_shares_out);
    push_u64(out, record.requested_at_ns.as_u64());
    push_u64(out, record.epoch_id.as_u64());
}

fn read_record(bytes: &[u8], cursor: &mut usize) -> Result<PendingDepositRecord, RuntimeError> {
    let owner_bytes = read_exact(bytes, cursor, 32)?;
    let mut owner = [0u8; 32];
    owner.copy_from_slice(owner_bytes);
    Ok(PendingDepositRecord {
        owner: Address(owner),
        assets: read_u128(bytes, cursor)?,
        min_shares_out: read_u128(bytes, cursor)?,
        requested_at_ns: TimestampNs::from_nanos(read_u64(bytes, cursor)?),
        epoch_id: EpochId::new(read_u64(bytes, cursor)?),
    })
}

/// Validate a page slot exactly as decode does: absent slots must carry a
/// fully zeroed record, live slots must match their deterministic position
/// and satisfy the kernel `PendingDeposit` law.
fn check_slot(
    page: u32,
    slot: usize,
    request_id: u64,
    record: &PendingDepositRecord,
) -> Result<(), RuntimeError> {
    if request_id == 0 {
        if record.is_absent() {
            Ok(())
        } else {
            Err(RuntimeError::storage_error("pending deposit slot residue"))
        }
    } else {
        let expected = u64::from(page) * u64::from(PENDING_DEPOSIT_PAGE_SIZE) + slot as u64 + 1;
        if request_id != expected {
            return Err(RuntimeError::storage_error("pending deposit slot mismatch"));
        }
        if record.owner == Address([0u8; 32]) {
            return Err(RuntimeError::storage_error("pending deposit owner invalid"));
        }
        record.kernel_deposit()?;
        Ok(())
    }
}

/// Encode a fixed-size page of pending deposits. Absent slots use request id 0.
pub(crate) fn encode_pending_deposit_page(
    page: u32,
    slots: &[(u64, PendingDepositRecord)],
) -> Result<Vec<u8>, RuntimeError> {
    if page >= MAX_PENDING_DEPOSIT_PAGES {
        return Err(RuntimeError::storage_error("pending deposit page invalid"));
    }
    if slots.len() != usize::try_from(PENDING_DEPOSIT_PAGE_SIZE).unwrap_or(usize::MAX) {
        return Err(RuntimeError::storage_error("pending deposit page size invalid"));
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"TDP");
    push_u8(&mut out, PENDING_DEPOSIT_PAGE_FORMAT);
    push_u8(&mut out, STORAGE_KIND_PENDING_DEPOSIT_PAGE);
    push_u32(&mut out, page);
    push_u32(&mut out, u32::try_from(slots.len()).unwrap_or(u32::MAX));
    for (slot, (request_id, record)) in slots.iter().enumerate() {
        check_slot(page, slot, *request_id, record)?;
        push_u64(&mut out, *request_id);
        push_record(&mut out, record);
    }
    Ok(out)
}

/// Decode a pending-deposit page and validate every entry against kernel law.
pub(crate) fn decode_pending_deposit_page(
    bytes: &[u8],
) -> Result<(u32, Vec<(u64, PendingDepositRecord)>), RuntimeError> {
    if bytes.len() < 10
        || &bytes[0..3] != b"TDP"
        || bytes[3] != PENDING_DEPOSIT_PAGE_FORMAT
        || bytes[4] != STORAGE_KIND_PENDING_DEPOSIT_PAGE
    {
        return Err(RuntimeError::storage_error("pending deposit page invalid"));
    }
    let mut cursor = 5usize;
    let page = read_u32(bytes, &mut cursor)?;
    if page >= MAX_PENDING_DEPOSIT_PAGES {
        return Err(RuntimeError::storage_error("pending deposit page invalid"));
    }
    let count = read_u32(bytes, &mut cursor)?;
    if count != PENDING_DEPOSIT_PAGE_SIZE {
        return Err(RuntimeError::storage_error("pending deposit page overflow"));
    }
    let mut slots = Vec::new();
    for slot in 0..usize::try_from(PENDING_DEPOSIT_PAGE_SIZE).unwrap_or(usize::MAX) {
        let request_id = read_u64(bytes, &mut cursor)?;
        let record = read_record(bytes, &mut cursor)?;
        check_slot(page, slot, request_id, &record)?;
        slots.push((request_id, record));
    }
    if cursor != bytes.len() {
        return Err(RuntimeError::storage_error(
            "pending deposit page trailing bytes",
        ));
    }
    Ok((page, slots))
}

/// Aggregate outstanding pending-deposit liability, always recomputed from pages.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(any(test, feature = "testutils"), derive(Debug))]
pub struct PendingDepositAggregate {
    pub count: u64,
    pub total_assets: u128,
    pub oldest_epoch: Option<EpochId>,
}

impl PendingDepositAggregate {
    const fn zero() -> Self {
        Self {
            count: 0,
            total_assets: 0,
            oldest_epoch: None,
        }
    }
}

/// Encode the aggregate record. Stored for audits; reads always recompute.
pub(crate) fn encode_pending_deposit_aggregate(aggregate: &PendingDepositAggregate) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"TDA");
    push_u8(&mut out, 1);
    push_u8(&mut out, STORAGE_KIND_PENDING_DEPOSIT_AGGREGATE);
    push_u64(&mut out, aggregate.count);
    push_u128(&mut out, aggregate.total_assets);
    push_u64(&mut out, aggregate.oldest_epoch.map_or(0, EpochId::as_u64));
    out
}

/// Decode the aggregate record, validating its epoch reference.
pub(crate) fn decode_pending_deposit_aggregate(
    bytes: &[u8],
) -> Result<PendingDepositAggregate, RuntimeError> {
    if bytes.len() != 37
        || &bytes[0..3] != b"TDA"
        || bytes[3] != 1
        || bytes[4] != STORAGE_KIND_PENDING_DEPOSIT_AGGREGATE
    {
        return Err(RuntimeError::storage_error("aggregate record invalid"));
    }
    let mut cursor = 5usize;
    let count = read_u64(bytes, &mut cursor)?;
    let total_assets = read_u128(bytes, &mut cursor)?;
    let oldest = read_u64(bytes, &mut cursor)?;
    if cursor != bytes.len() {
        return Err(RuntimeError::storage_error("aggregate record length invalid"));
    }
    let oldest_epoch = if oldest == 0 {
        None
    } else {
        if !EpochId::new(oldest).is_settlement_epoch() {
            return Err(RuntimeError::storage_error("aggregate epoch invalid"));
        }
        Some(EpochId::new(oldest))
    };
    if count == 0 && oldest_epoch.is_some() {
        return Err(RuntimeError::storage_error("aggregate epoch orphaned"));
    }
    if count > 0 && oldest_epoch.is_none() {
        return Err(RuntimeError::storage_error("aggregate epoch missing"));
    }
    Ok(PendingDepositAggregate {
        count,
        total_assets,
        oldest_epoch,
    })
}

impl PendingStorage for SorobanStorage<'_> {}

/// Ledger operations available to the vault storage implementation.
pub(crate) trait PendingStorage: private::Sealed {
    /// Recompute liability from stored pages only. Every page is decoded
    /// with the same strict page codec, slot placement law, and kernel
    /// deposit revalidation used by every read; the write path derives the
    /// aggregate it is about to persist from this function, and every read
    /// and final commit check requires the stored aggregate to equal it.
    fn recompute_pending_deposit_ledger(&self) -> Result<PendingDepositAggregate, RuntimeError> {
        let env = self.storage_env();
        let mut aggregate = PendingDepositAggregate::zero();
        for page in 0..MAX_PENDING_DEPOSIT_PAGES {
            let key = DepositPageKey { page };
            let Some(bytes) = env.storage().persistent().get::<_, Bytes>(&key) else {
                continue;
            };
            let (page_id, slots) = decode_pending_deposit_page(&bytes.to_alloc_vec())?;
            if page_id != page {
                return Err(RuntimeError::storage_error("pending deposit page mismatch"));
            }
            // A page stored under the wrong key would be skipped by the
            // id-derived lookup path and could strand live liability: every
            // nonempty slot must hash-map to the key it is stored under.
            for (request_id, _) in &slots {
                if *request_id == 0 {
                    continue;
                }
                let expected = request_page_slot(*request_id)
                    .ok_or_else(|| RuntimeError::storage_error("pending deposit id out of range"))?;
                if expected.0 != page {
                    return Err(RuntimeError::storage_error("pending deposit page misplacement"));
                }
            }
            for (request_id, record) in &slots {
                if *request_id == 0 {
                    continue;
                }
                aggregate.count = aggregate
                    .count
                    .checked_add(1)
                    .ok_or_else(|| RuntimeError::storage_error("aggregate count overflow"))?;
                aggregate.total_assets = aggregate.total_assets.checked_add(record.assets).ok_or_else(
                    || RuntimeError::storage_error("aggregate assets overflow"),
                )?;
                aggregate.oldest_epoch = Some(match aggregate.oldest_epoch {
                    Some(existing) => existing.min(record.epoch_id),
                    None => record.epoch_id,
                });
            }
        }
        Ok(aggregate)
    }
    /// Recompute liability state from stored pages. Every page is decoded
    /// against kernel law and every slot placement is checked; this is the
    /// only source of truth for reads, writes, and integrity checks.
    fn pending_deposit_ledger_view(
        &self,
    ) -> Result<
        (
            u64,
            PendingDepositAggregate,
            Vec<(u32, Vec<(u64, PendingDepositRecord)>)>,
        ),
        RuntimeError,
    > {
        let env = self.storage_env();
        let stored_watermark = match env.storage().persistent().get::<_, u64>(&NextIdKey) {
            Some(raw) => {
                if raw == 0 || raw > u64::try_from(MAX_PENDING_DEPOSITS).unwrap_or(u64::MAX) {
                    return Err(RuntimeError::storage_error("next id record invalid"));
                }
                raw
            }
            None => 0,
        };
        let stored_aggregate = match env
            .storage()
            .persistent()
            .get::<_, Bytes>(&AggregateKey)
        {
            Some(bytes) => Some(decode_pending_deposit_aggregate(&bytes.to_alloc_vec())?),
            None => None,
        };
        let mut last_issued = 0u64;
        let mut recomputed = PendingDepositAggregate::zero();
        let mut pages: Vec<(u32, Vec<(u64, PendingDepositRecord)>)> = Vec::new();
        for page in 0..MAX_PENDING_DEPOSIT_PAGES {
            let key = DepositPageKey { page };
            let Some(bytes) = env.storage().persistent().get::<_, Bytes>(&key) else {
                continue;
            };
            let (page_id, slots) = decode_pending_deposit_page(&bytes.to_alloc_vec())?;
            if page_id != page {
                return Err(RuntimeError::storage_error("pending deposit page mismatch"));
            }
            for (request_id, record) in &slots {
                if *request_id == 0 {
                    continue;
                }
                if stored_watermark == 0 || *request_id >= stored_watermark {
                    return Err(RuntimeError::storage_error("next id record invalid"));
                }
                last_issued = last_issued.max(*request_id);
                recomputed.count = recomputed
                    .count
                    .checked_add(1)
                    .ok_or_else(|| RuntimeError::storage_error("aggregate count overflow"))?;
                recomputed.total_assets = recomputed.total_assets.checked_add(record.assets).ok_or_else(
                    || RuntimeError::storage_error("aggregate assets overflow"),
                )?;
                recomputed.oldest_epoch = Some(match recomputed.oldest_epoch {
                    Some(existing) => existing.min(record.epoch_id),
                    None => record.epoch_id,
                });
            }
            pages.push((page_id, slots));
        }
        if stored_watermark == 0 && recomputed.count != 0 {
            return Err(RuntimeError::storage_error("next id record invalid"));
        }
        let _ = last_issued;
        if let Some(stored) = &stored_aggregate {
            if stored != &recomputed {
                return Err(RuntimeError::storage_error("aggregate mismatch"));
            }
        } else if recomputed.count != 0 {
            return Err(RuntimeError::storage_error("aggregate missing"));
        }
        Ok((stored_watermark, recomputed, pages))
    }

    /// Whether any pending deposit is bound to an epoch earlier than `epoch`.
    fn any_pending_deposit_before_epoch(&self, epoch: EpochId) -> Result<bool, RuntimeError> {
        let (.., pages) = self.pending_deposit_ledger_view()?;
        Ok(pages.iter().any(|(.., slots)| {
            slots
                .iter()
                .any(|(id, record)| *id != 0 && record.epoch_id < epoch)
        }))
    }

    /// Deterministic lookup of a pending deposit record by request id,
    /// including the immutable depositor share floor.
    fn load_pending_deposit(
        &self,
        request_id: u64,
    ) -> Result<Option<PendingDepositRecord>, RuntimeError> {
        let Some((page, slot)) = request_page_slot(request_id) else {
            return Ok(None);
        };
        let env = self.storage_env();
        let Some(bytes) = env
            .storage()
            .persistent()
            .get::<_, Bytes>(&DepositPageKey { page })
        else {
            return Ok(None);
        };
        let (page_id, slots) = decode_pending_deposit_page(&bytes.to_alloc_vec())?;
        if page_id != page {
            return Err(RuntimeError::storage_error("pending deposit page mismatch"));
        }
        match slots.get(slot) {
            Some((id, record)) if *id == request_id => Ok(Some(record.clone())),
            Some((id, _)) if *id == 0 => Ok(None),
            Some(_) => Err(RuntimeError::storage_error("pending deposit id mismatch")),
            None => Ok(None),
        }
    }

    #[cfg(test)]
    /// The immutable share floor persisted for `request_id`, if any.
    /// Admission must use this value and must not substitute a caller floor.
    fn load_pending_deposit_min_shares_out(
        &self,
        request_id: u64,
    ) -> Result<Option<u128>, RuntimeError> {
        Ok(self
            .load_pending_deposit(request_id)?
            .map(|record| record.min_shares_out()))
    }

    #[cfg(test)]
    /// Persist the depositor share floor for an existing request.
    ///
    /// The floor is written when the record is created. This helper exists
    /// for lifecycle callers only: re-saving the identical value is
    /// idempotent, replacing a stored floor fails closed, and a missing or
    /// removed record fails closed. The floor is erased atomically with
    /// the liability record by removal; there is no standalone update.
    fn save_pending_deposit_min_shares_out(
        &self,
        request_id: u64,
        min_shares_out: u128,
    ) -> Result<(), RuntimeError> {
        match self.load_pending_deposit(request_id)? {
            Some(record) if record.min_shares_out == min_shares_out => Ok(()),
            Some(_) => Err(RuntimeError::storage_error("deposit share floor immutable")),
            None => Err(RuntimeError::storage_error("pending deposit not found")),
        }
    }

    /// Issue the next monotonic request id, or fail closed if the lifetime
    /// cap is exhausted or ids would regress.
    fn next_deposit_request_id(&self) -> Result<u64, RuntimeError> {
        let (stored_watermark, ..) = self.pending_deposit_ledger_view()?;
        let cap = u64::try_from(MAX_PENDING_DEPOSITS).unwrap_or(u64::MAX);
        let next = if stored_watermark == 0 {
            1
        } else {
            stored_watermark
        };
        if next > cap {
            return Err(RuntimeError::storage_error("deposit id cap exhausted"));
        }
        Ok(next)
    }

    /// Total outstanding pending-deposit liability, recomputed from pages.
    fn pending_deposit_stats(&self) -> Result<PendingDepositAggregate, RuntimeError> {
        let (.., aggregate, _) = self.pending_deposit_ledger_view()?;
        Ok(aggregate)
    }

    #[cfg(test)]
    /// Oldest unsettled intake epoch across live pending deposits, if any.
    fn pending_deposit_oldest_epoch(&self) -> Result<Option<EpochId>, RuntimeError> {
        Ok(self.pending_deposit_stats()?.oldest_epoch)
    }

    /// Insert one pending deposit at its monotonic id together with the
    /// depositor's immutable share floor. The assets stay in vault custody
    /// outside `VaultState`; nothing is credited or minted.
    fn create_pending_deposit(
        &self,
        deposit: &PendingDeposit,
        min_shares_out: u128,
    ) -> Result<u64, RuntimeError> {
        let env = self.storage_env();
        if !self.epoch_runtime_enabled() {
            return Err(RuntimeError::storage_error("epoch mode disabled"));
        }
        let header = self.load_epoch_state()?;
        if header.phase != EpochPhase::Open || deposit.epoch_id != header.intake_epoch {
            return Err(RuntimeError::storage_error("deposit epoch mismatch"));
        }
        let record = PendingDepositRecord::validated(
            deposit.owner,
            deposit.assets,
            min_shares_out,
            deposit.requested_at_ns,
            deposit.epoch_id,
        )?;
        let request_id = self.next_deposit_request_id()?;
        let Some((page, slot)) = request_page_slot(request_id) else {
            return Err(RuntimeError::storage_error("deposit id out of range"));
        };
        let mut slots: [(u64, PendingDepositRecord); PENDING_DEPOSIT_PAGE_SIZE as usize] =
            core::array::from_fn(|_| (0u64, PendingDepositRecord::absent()));
        // Rebuild the page from ledger state, then place the new entry.
        let (.., pages) = self.pending_deposit_ledger_view()?;
        for (page_id, existing) in &pages {
            if *page_id == page {
                for (idx, (id, entry)) in existing.iter().enumerate() {
                    if *id != 0 {
                        if let Some(target) = slots.get_mut(idx) {
                            *target = (*id, entry.clone());
                        }
                    }
                }
            }
        }
        let Some(target) = slots.get_mut(slot) else {
            return Err(RuntimeError::storage_error("slot unavailable"));
        };
        if target.0 != 0 {
            return Err(RuntimeError::storage_error("slot in use"));
        }
        let watermark = request_id.checked_add(1).ok_or_else(|| {
            RuntimeError::storage_error("deposit id cap exhausted")
        })?;
        env.storage().persistent().set(&NextIdKey, &watermark);
        *target = (request_id, record);
        let page_bytes = encode_pending_deposit_page(page, &slots)?;
        env.storage()
            .persistent()
            .set(&DepositPageKey { page }, &Bytes::from_slice(env, &page_bytes));
        let aggregate = self.recompute_pending_deposit_ledger()?;
        env.storage().persistent().set(
            &AggregateKey,
            &Bytes::from_slice(env, &encode_pending_deposit_aggregate(&aggregate)),
        );
        env.storage().persistent().extend_ttl(
            &DepositPageKey { page },
            DEFAULT_TTL_THRESHOLD,
            DEFAULT_TTL_EXTEND_TO,
        );
        env.storage().persistent().extend_ttl(
            &AggregateKey,
            DEFAULT_TTL_THRESHOLD,
            DEFAULT_TTL_EXTEND_TO,
        );
        env.storage().persistent().extend_ttl(
            &NextIdKey,
            DEFAULT_TTL_THRESHOLD,
            DEFAULT_TTL_EXTEND_TO,
        );
        self.pending_deposit_ledger_view()?;
        Ok(request_id)
    }

    /// Owner-bound cancellation of a pending deposit at its request id.
    ///
    /// The caller is compared against the persisted owner before any
    /// mutation: a non-owner cancellation cannot change ledger state. The
    /// share floor is deleted together with the request. Refund execution
    /// belongs to the runtime; this removal is the liability bookend that
    /// makes the cancelled request unrecoverable for admission.
    fn cancel_pending_deposit(
        &self,
        caller: &Address,
        request_id: u64,
    ) -> Result<(), RuntimeError> {
        let Some(record) = self.load_pending_deposit(request_id)? else {
            return Err(RuntimeError::storage_error("pending deposit not found"));
        };
        if record.owner != *caller {
            return Err(RuntimeError::storage_error("pending deposit not owner"));
        }
        self.remove_pending_deposit_record(request_id)
    }

    /// Take the exact persisted record for an already-authorized admission
    /// and remove it atomically. Crate-internal only: the caller must have
    /// passed the kernel `AdmitPendingDeposit` allocator authority check for
    /// this request. The returned record carries the persisted assets and
    /// share floor so the runtime applies the kernel action with stored
    /// fields only. Record presence is the at-most-once authority: a second
    /// take after a successful removal fails closed with no record found,
    /// and a failed transaction rolls the removal back together with the
    /// applied effects. There is no durable admitted marker.
    fn take_pending_deposit(&self, request_id: u64) -> Result<PendingDepositRecord, RuntimeError> {
        let Some(record) = self.load_pending_deposit(request_id)? else {
            return Err(RuntimeError::storage_error("pending deposit not found"));
        };
        self.remove_pending_deposit_record(request_id)?;
        Ok(record)
    }

    /// Remove a persisted record and its share floor, then repair the page
    /// and aggregate. Shared by owner-bound cancellation and admission
    /// consumption so both authorities produce identical ledger effects.
    fn remove_pending_deposit_record(&self, request_id: u64) -> Result<(), RuntimeError> {
        let Some((page, slot)) = request_page_slot(request_id) else {
            return Err(RuntimeError::storage_error("deposit id out of range"));
        };
        let env = self.storage_env();
        let Some(bytes) = env
            .storage()
            .persistent()
            .get::<_, Bytes>(&DepositPageKey { page })
        else {
            return Err(RuntimeError::storage_error("pending deposit not found"));
        };
        let (page_id, mut slots) = decode_pending_deposit_page(&bytes.to_alloc_vec())?;
        if page_id != page {
            return Err(RuntimeError::storage_error("pending deposit page mismatch"));
        }
        let Some((id, record)) = slots.get_mut(slot) else {
            return Err(RuntimeError::storage_error("pending deposit not found"));
        };
        if *id != request_id || record.is_absent() {
            return Err(RuntimeError::storage_error("pending deposit not found"));
        }
        *id = 0;
        *record = PendingDepositRecord::absent();
        let any_live = slots.iter().any(|(live_id, _)| *live_id != 0);
        if any_live {
            let page_bytes = encode_pending_deposit_page(page, &slots)?;
            env.storage()
                .persistent()
                .set(&DepositPageKey { page }, &Bytes::from_slice(env, &page_bytes));
        } else {
            env.storage().persistent().remove(&DepositPageKey { page });
        }
        let aggregate = self.recompute_pending_deposit_ledger()?;
        if aggregate.count == 0 {
            env.storage().persistent().remove(&AggregateKey);
        } else {
            env.storage().persistent().set(
                &AggregateKey,
                &Bytes::from_slice(env, &encode_pending_deposit_aggregate(&aggregate)),
            );
        }
        self.pending_deposit_ledger_view()?;
        Ok(())
    }

    #[cfg(test)]
    /// Owner-bound cancellation lookup: list request ids owned by `owner`.
    fn pending_deposit_ids_of_owner(&self, owner: &Address) -> Result<Vec<u64>, RuntimeError> {
        let (.., pages) = self.pending_deposit_ledger_view()?;
        let mut ids = Vec::new();
        for (.., slots) in pages {
            for (id, record) in slots {
                if id != 0 && record.owner == *owner {
                    ids.push(id);
                }
            }
        }
        Ok(ids)
    }

    /// Recompute all page data and confirm it matches the aggregate. Any
    /// mismatch fails closed so admission can never trust stale liabilities.
    fn verify_pending_deposit_integrity(&self) -> Result<(), RuntimeError> {
        self.pending_deposit_ledger_view()?;
        Ok(())
    }
}

mod private {
    use super::SorobanStorage;
    pub trait Sealed {
        fn storage_env(&self) -> &soroban_sdk::Env;
        fn epoch_runtime_enabled(&self) -> bool;
        fn load_epoch_state(
            &self,
        ) -> Result<templar_vault_kernel::EpochState, crate::error::RuntimeError>;
    }
    impl Sealed for SorobanStorage<'_> {
        fn storage_env(&self) -> &soroban_sdk::Env {
            &self.env
        }
        fn epoch_runtime_enabled(&self) -> bool {
            SorobanStorage::epoch_mode_active(self)
        }
        fn load_epoch_state(
            &self,
        ) -> Result<templar_vault_kernel::EpochState, crate::error::RuntimeError> {
            SorobanStorage::load_epoch_state(self)
        }
    }
}
