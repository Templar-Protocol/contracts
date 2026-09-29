//! Explicit versioned migration off the fixed-claim withdrawal model.
//!
//! The previous generation of this contract stored an asset-denominated payout
//! estimate with every queued exit and paid from it. That estimate is gone. An
//! exit is now priced only by the accepted settlement snapshot of the epoch it
//! was requested in, so nothing may keep or pay a stored quote.
//!
//! Old state stays migratable. [`LegacyVaultState`] decodes the prior
//! serialized layout byte-for-byte, and [`LegacyVaultState::into_current`]
//! rebuilds it under current law: each obligation is carried over unchanged in
//! ownership, receiver, escrowed shares and request time, the stale estimate is
//! dropped, and every carried entry is tagged as migration intake so it can
//! only ever be priced by the first accepted settlement. Borsh encodes by
//! position, so the estimate survives only in the decoder's `stale_quote`
//! binding, which is never read, and therefore cannot reach settlement math.
//!
//! Nothing here reinterprets current state as legacy state: borsh is positional
//! and the layouts differ in length, so a wrong-version decode fails rather
//! than silently re-reading settlement-era fields.

use crate::{
    aum::AUM,
    governance::{Abdicator, Gate, Timelocks},
    policy::{MarketExecutionLock, SupplyQueue, WithdrawRoute},
    Contract, MarketRecord,
};
use near_sdk::{env, near, require, AccountId, PanicOnDefault};
use std::collections::BTreeMap;
use templar_common::{
    asset::{BorrowAsset, FungibleAsset},
    vault::{
        wad::Wad, CapGroupId, CapGroupRecord, FeeAccrualAnchor, Fees, MarketId, OpState,
    },
};
use templar_vault_kernel::{Address, EpochState, PendingWithdrawal, TimestampNs, WithdrawQueue};

/// Prior withdrawal record. `stale_quote` occupies the position of the removed
/// fixed payout estimate and is dropped by the migration.
#[near(serializers = [borsh])]
#[derive(Clone)]
struct LegacyWithdrawalRecord {
    owner: Address,
    receiver: Address,
    escrow_shares: u128,
    stale_quote: u128,
    requested_at_ns: TimestampNs,
}

/// Prior queue entry.
#[near(serializers = [borsh])]
#[derive(Clone)]
struct LegacyQueueEntry {
    id: u64,
    withdrawal: LegacyWithdrawalRecord,
}

/// Prior queued-withdrawal container.
#[near(serializers = [borsh])]
#[derive(Clone)]
struct LegacyQueuedEntries {
    entries: Vec<LegacyQueueEntry>,
}

/// Prior withdrawal queue. The cached quote total is decoded only so the
/// escrow total can be checked; the quote sum itself is discarded.
#[near(serializers = [borsh])]
#[derive(Clone)]
struct LegacyWithdrawalQueue {
    pending_withdrawals: LegacyQueuedEntries,
    next_withdraw_to_execute: u64,
    next_pending_withdrawal_id: u64,
    cached_total_escrow: u128,
    cached_stale_quote_total: u128,
}

/// Prior contract state, decoded only to migrate forward.
#[near(serializers = [borsh])]
#[derive(PanicOnDefault)]
pub struct LegacyVaultState {
    underlying_asset: FungibleAsset<BorrowAsset>,
    aum: AUM,
    fees: Fees<Wad>,
    skim_recipient: AccountId,
    fee_anchor: FeeAccrualAnchor,
    idle_balance: u128,
    op_state: OpState,
    next_op_id: u64,
    last_refresh_ns: u64,
    refresh_cooldown_ns: u64,
    withdrawal_cooldown_ns: u64,
    idle_resync_last_ns: u64,
    idle_resync_cooldown_ns: u64,
    idle_resync_inflight_op_id: u64,
    virtual_shares: u128,
    virtual_assets: u128,
    markets: BTreeMap<MarketId, MarketRecord>,
    market_ids: BTreeMap<AccountId, MarketId>,
    cap_groups: BTreeMap<CapGroupId, CapGroupRecord>,
    next_market_id: u32,
    governance_timelocks: Timelocks,
    supply_queue: SupplyQueue,
    withdraw_queue: LegacyWithdrawalQueue,
    address_book: BTreeMap<Address, AccountId>,
    market_execution_lock: MarketExecutionLock,
    withdraw_route: WithdrawRoute,
    abdicator: Abdicator,
    gate: Gate,
}

impl LegacyVaultState {
    /// Rebuild this state under current law.
    ///
    /// Every obligation is carried over and retagged as migration intake; the
    /// stale estimate is never read. An entry that escrowed no shares owed
    /// nothing and cannot exist now, so it is dropped and the FIFO head
    /// advances to the next obligation rather than aborting the migration.
    #[must_use]
    pub(crate) fn into_current(self) -> Contract {
        let LegacyVaultState {
            underlying_asset,
            aum,
            fees,
            skim_recipient,
            fee_anchor,
            idle_balance,
            op_state,
            next_op_id,
            last_refresh_ns,
            refresh_cooldown_ns,
            withdrawal_cooldown_ns,
            idle_resync_last_ns,
            idle_resync_cooldown_ns,
            idle_resync_inflight_op_id,
            virtual_shares,
            virtual_assets,
            markets,
            market_ids,
            cap_groups,
            next_market_id,
            governance_timelocks,
            supply_queue,
            withdraw_queue,
            address_book,
            market_execution_lock,
            withdraw_route,
            abdicator,
            gate,
        } = self;

        let LegacyWithdrawalQueue {
            pending_withdrawals,
            next_withdraw_to_execute,
            next_pending_withdrawal_id,
            cached_total_escrow,
            cached_stale_quote_total: _,
        } = withdraw_queue;

        let mut carried_escrow = 0u128;
        let mut carried: BTreeMap<u64, PendingWithdrawal> = BTreeMap::new();
        for entry in &pending_withdrawals.entries {
            let record = &entry.withdrawal;
            if let Some(carried_entry) = PendingWithdrawal::migrated_legacy(
                record.owner,
                record.receiver,
                record.escrow_shares,
                record.requested_at_ns,
            )
            .ok()
            {
                carried_escrow = carried_escrow.saturating_add(carried_entry.escrow_shares);
                carried.insert(entry.id, carried_entry);
            }
        }
        require!(
            carried_escrow == cached_total_escrow,
            "migration changed escrowed shares"
        );

        // Preserve FIFO head positions. Only a head that escrowed nothing is
        // replaced, and then only by the next obligation in queue order.
        let head = if carried.contains_key(&next_withdraw_to_execute) {
            next_withdraw_to_execute
        } else if let Some(first) = carried.keys().next() {
            *first
        } else {
            next_withdraw_to_execute
        };
        let tail = next_pending_withdrawal_id.max(
            carried
                .keys()
                .next_back()
                .map_or(0, |last| last.saturating_add(1)),
        );
        let withdraw_queue = WithdrawQueue::with_state(
            carried.iter().map(|(id, entry)| (*id, entry.clone())),
            head,
            tail,
        );
        if !withdraw_queue.is_empty() {
            require!(
                withdraw_queue.head().is_some(),
                "withdraw queue head missing during migration"
            );
            require!(
                withdraw_queue.has_migrated_intake(),
                "migrated obligations must be tagged migration intake"
            );
        }

        Contract {
            underlying_asset,
            aum,
            fees: crate::kernel_fees_from_boundary(fees),
            skim_recipient,
            fee_anchor,
            idle_balance,
            op_state,
            next_op_id,
            last_refresh_ns,
            refresh_cooldown_ns,
            withdrawal_cooldown_ns,
            idle_resync_last_ns,
            idle_resync_cooldown_ns,
            idle_resync_inflight_op_id,
            virtual_shares,
            virtual_assets,
            markets,
            market_ids,
            cap_groups,
            next_market_id,
            governance_timelocks,
            supply_queue,
            epoch: EpochState::genesis(),
            pending_deposits: BTreeMap::new(),
            next_pending_deposit_id: 1,
            pending_deposit_min_shares_out: BTreeMap::new(),
            pending_deposit_assets: 0,
            custody_measured_at_ns: 0,
            withdraw_queue,
            address_book,
            market_execution_lock,
            withdraw_route,
            abdicator,
            gate,
        }
    }
}

/// Migrate from the prior fixed-claim state, when that is what is stored.
///
/// Returns `None` when no such state exists, leaving the caller to try older
/// versions. Only decodes the layout above, so it cannot silently re-read
/// current-generation state.
#[must_use]
pub(crate) fn migrate_from_legacy_state() -> Option<Contract> {
    let legacy: LegacyVaultState = env::state_read()?;
    Some(legacy.into_current())
}
