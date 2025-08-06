use std::collections::HashMap;

use near_sdk::{
    collections::{LookupMap, UnorderedMap},
    env, near, AccountId, BorshStorageKey, IntoStorageKey,
};

use crate::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowPositionGuard, BorrowPositionRef},
    chunked_append_only_list::ChunkedAppendOnlyList,
    event::MarketEvent,
    market::{MarketConfiguration, WithdrawalResolution},
    number::Decimal,
    snapshot::Snapshot,
    static_yield::StaticYieldRecord,
    supply::{SupplyPosition, SupplyPositionGuard, SupplyPositionRef},
    withdrawal_queue::{error::WithdrawalQueueLockError, WithdrawalQueue},
};

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    SupplyPositions,
    BorrowPositions,
    FinalizedSnapshots,
    WithdrawalQueue,
    StaticYield,
}

#[near]
pub struct Market {
    prefix: Vec<u8>,
    pub configuration: MarketConfiguration,
    /// Total amount of borrow asset earning interest in the market.
    pub borrow_asset_deposited_active: BorrowAssetAmount,
    /// Mapping of upcoming snapshot indices to amounts of borrow asset that will be activated.
    pub borrow_asset_deposited_incoming: HashMap<u32, BorrowAssetAmount>,
    /// Sending borrow asset out, because if somebody sends the contract borrow asset, it's ok for the
    /// contract to attempt to fulfill withdrawal request, even if the market thinks it doesn't have
    /// enough to fulfill.
    pub borrow_asset_in_flight: BorrowAssetAmount,
    /// Amount of borrow asset that has been withdrawn (is in use by) by borrowers.
    ///
    /// `borrow_asset_deposited_active - borrow_asset_borrowed >= 0` should always be true.
    pub borrow_asset_borrowed: BorrowAssetAmount,
    /// Market-wide collateral asset deposit tracking.
    pub collateral_asset_deposited: CollateralAssetAmount,
    pub(crate) supply_positions: UnorderedMap<AccountId, SupplyPosition>,
    pub(crate) borrow_positions: UnorderedMap<AccountId, BorrowPosition>,
    pub current_snapshot: Snapshot,
    pub finalized_snapshots: ChunkedAppendOnlyList<Snapshot, 128>,
    pub withdrawal_queue: WithdrawalQueue,
    pub static_yield: LookupMap<AccountId, StaticYieldRecord>,
}

impl Market {
    pub fn new(prefix: impl IntoStorageKey, configuration: MarketConfiguration) -> Self {
        if let Err(e) = configuration.validate() {
            env::panic_str(&e.to_string());
        }

        let prefix = prefix.into_storage_key();
        macro_rules! key {
            ($key: ident) => {
                [
                    prefix.as_slice(),
                    StorageKey::$key.into_storage_key().as_slice(),
                ]
                .concat()
            };
        }

        let first_snapshot = Snapshot::new(configuration.time_chunk_configuration.previous());
        let mut current_snapshot = first_snapshot.clone();
        current_snapshot.set_time_chunk(configuration.time_chunk_configuration.now());

        let mut self_ = Self {
            prefix: prefix.clone(),
            configuration,
            borrow_asset_deposited_active: 0.into(),
            borrow_asset_deposited_incoming: HashMap::new(),
            borrow_asset_in_flight: 0.into(),
            borrow_asset_borrowed: 0.into(),
            collateral_asset_deposited: 0.into(),
            supply_positions: UnorderedMap::new(key!(SupplyPositions)),
            borrow_positions: UnorderedMap::new(key!(BorrowPositions)),
            current_snapshot,
            finalized_snapshots: ChunkedAppendOnlyList::new(key!(FinalizedSnapshots)),
            withdrawal_queue: WithdrawalQueue::new(key!(WithdrawalQueue)),
            static_yield: LookupMap::new(key!(StaticYield)),
        };

        self_.finalized_snapshots.push(first_snapshot);

        self_
    }

    pub fn total_incoming(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposited_incoming
            .values()
            .fold(BorrowAssetAmount::zero(), |mut a, b| {
                a.join(*b);
                a
            })
    }

    pub fn get_last_finalized_snapshot(&self) -> &Snapshot {
        #[allow(clippy::unwrap_used, reason = "Snapshots are never empty")]
        self.finalized_snapshots
            .get(self.finalized_snapshots.len() - 1)
            .unwrap()
    }

    pub fn snapshot(&mut self) -> u32 {
        self.snapshot_with_yield_distribution(BorrowAssetAmount::zero())
    }

    fn snapshot_with_yield_distribution(&mut self, yield_distribution: BorrowAssetAmount) -> u32 {
        let time_chunk = self.configuration.time_chunk_configuration.now();

        // If still in current time chunk, just update the current snapshot.
        if self.current_snapshot.time_chunk() == &time_chunk {
            self.current_snapshot.update_active(
                self.borrow_asset_deposited_active,
                self.borrow_asset_borrowed,
                self.collateral_asset_deposited,
                &self.configuration.borrow_interest_rate_strategy,
            );
            self.current_snapshot.add_yield(yield_distribution);
            self.current_snapshot.set_borrow_asset_deposited_incoming(*self
                .borrow_asset_deposited_incoming
                .get(&self.finalized_snapshots.len())
                .unwrap_or(&0.into()));
        } else {
            // Otherwise, finalize the current snapshot and create a new one.
            let deposited_incoming = self
                .borrow_asset_deposited_incoming
                .remove(&self.finalized_snapshots.len())
                .unwrap_or(0.into());
            self.borrow_asset_deposited_active.join(deposited_incoming);
            let mut snapshot = Snapshot::new(time_chunk);
            snapshot.set_yield_distribution(yield_distribution);
            snapshot.set_borrow_asset_deposited_incoming(deposited_incoming);
            snapshot.update_active(
                self.borrow_asset_deposited_active,
                self.borrow_asset_borrowed,
                self.collateral_asset_deposited,
                &self.configuration.borrow_interest_rate_strategy,
            );
            std::mem::swap(&mut snapshot, &mut self.current_snapshot);
            MarketEvent::SnapshotFinalized {
                index: self.finalized_snapshots.len(),
                snapshot: snapshot.clone(),
            }
            .emit();
            self.finalized_snapshots.push(snapshot);
        }

        self.finalized_snapshots.len()
    }

    pub fn get_borrow_asset_available_to_borrow(&self) -> BorrowAssetAmount {
        #[allow(
            clippy::unwrap_used,
            reason = "Factor is guaranteed to be <=1, so value must still fit in u128"
        )]
        let must_retain = ((1u32 - self.configuration.borrow_asset_maximum_usage_ratio)
            * Decimal::from(self.borrow_asset_deposited_active))
        .to_u128_ceil()
        .unwrap();

        u128::from(self.borrow_asset_deposited_active)
            .saturating_sub(u128::from(self.borrow_asset_borrowed))
            .saturating_sub(u128::from(self.borrow_asset_in_flight))
            .saturating_sub(must_retain)
            .into()
    }

    pub fn iter_supply_positions(&self) -> impl Iterator<Item = (AccountId, SupplyPosition)> + '_ {
        self.supply_positions.iter()
    }

    pub fn supply_position_ref(&self, account_id: AccountId) -> Option<SupplyPositionRef<&Self>> {
        self.supply_positions
            .get(&account_id)
            .map(|position| SupplyPositionRef::new(self, account_id, position))
    }

    pub fn supply_position_guard(&mut self, account_id: AccountId) -> Option<SupplyPositionGuard> {
        self.supply_positions
            .get(&account_id)
            .map(|position| SupplyPositionGuard::new(self, account_id, position))
    }

    pub fn get_or_create_supply_position_guard(
        &mut self,
        account_id: AccountId,
    ) -> SupplyPositionGuard {
        let position = self
            .supply_positions
            .get(&account_id)
            .unwrap_or_else(|| SupplyPosition::new(self.snapshot()));

        SupplyPositionGuard::new(self, account_id, position)
    }

    pub fn cleanup_supply_position(&mut self, account_id: &AccountId) -> bool {
        self.supply_positions
            .get(account_id)
            .filter(SupplyPosition::can_be_removed)
            .and_then(|_| self.supply_positions.remove(account_id))
            .is_some()
    }

    pub fn iter_borrow_positions(&self) -> impl Iterator<Item = (AccountId, BorrowPosition)> + '_ {
        self.borrow_positions.iter()
    }

    pub fn borrow_position_ref(&self, account_id: AccountId) -> Option<BorrowPositionRef<&Self>> {
        self.borrow_positions
            .get(&account_id)
            .map(|position| BorrowPositionRef::new(self, account_id, position))
    }

    pub fn borrow_position_guard(&mut self, account_id: AccountId) -> Option<BorrowPositionGuard> {
        self.borrow_positions
            .get(&account_id)
            .map(|position| BorrowPositionGuard::new(self, account_id, position))
    }

    pub fn get_or_create_borrow_position_guard(
        &mut self,
        account_id: AccountId,
    ) -> BorrowPositionGuard {
        let position = self
            .borrow_positions
            .get(&account_id)
            .unwrap_or_else(|| BorrowPosition::new(self.snapshot()));

        BorrowPositionGuard::new(self, account_id, position)
    }

    pub fn cleanup_borrow_position(&mut self, account_id: &AccountId) -> bool {
        self.borrow_positions
            .get(account_id)
            .filter(BorrowPosition::can_be_removed)
            .and_then(|_| self.borrow_positions.remove(account_id))
            .is_some()
    }

    /// # Errors
    /// - If the withdrawal queue is already locked.
    /// - If the withdrawal queue is empty.
    pub fn try_lock_next_withdrawal_request(
        &mut self,
    ) -> Result<Option<WithdrawalResolution>, WithdrawalQueueLockError> {
        let (account_id, requested_amount) = self.withdrawal_queue.try_lock()?;

        let Some((amount, mut supply_position)) =
            self.supply_position_guard(account_id)
                .and_then(|supply_position| {
                    // Cap withdrawal amount to deposit amount at most.
                    let amount = supply_position.total_deposit().min(requested_amount);

                    (!amount.is_zero()).then_some((amount, supply_position))
                })
        else {
            // The amount that the entry is eligible to withdraw is zero, so skip it.
            self.withdrawal_queue
                .try_pop()
                .unwrap_or_else(|| env::panic_str("Inconsistent state")); // we just locked the queue
            return Ok(None);
        };

        let proof = supply_position.accumulate_yield();
        let resolution =
            supply_position.record_withdrawal_initial(proof, amount, env::block_timestamp_ms());

        Ok(Some(resolution))
    }

    pub fn record_borrow_asset_protocol_yield(&mut self, amount: BorrowAssetAmount) {
        let mut yield_record = self
            .static_yield
            .get(&self.configuration.protocol_account_id)
            .unwrap_or_default();

        yield_record.borrow_asset.join(amount);

        self.static_yield
            .insert(&self.configuration.protocol_account_id, &yield_record);
    }

    pub fn record_borrow_asset_yield_distribution(&mut self, mut amount: BorrowAssetAmount) {
        // Sanity.
        if amount.is_zero() {
            return;
        }

        MarketEvent::GlobalYieldDistributed {
            borrow_asset_amount: amount,
        }
        .emit();

        // First, static yield.

        let total_weight =
            Decimal::from(u16::from(self.configuration.yield_weights.total_weight()));
        let total_amount = Decimal::from(amount);
        let amount_per_weight = total_amount / total_weight;

        for (account_id, share_weight) in &self.configuration.yield_weights.r#static {
            #[allow(clippy::unwrap_used, reason = "share_weight / total_weight <= 1")]
            let share = amount
                .split((*share_weight * amount_per_weight).to_u128_floor().unwrap())
                // Safety:
                // Guaranteed share_weight <= total_weight
                // Guaranteed sum(share_weights) == total_weight
                // Guaranteed sum(floor(total_amount * share_weight / total_weight) for each share_weight in share_weights) <= total_amount
                // Therefore this should never panic.
                .unwrap();

            let mut yield_record = self.static_yield.get(account_id).unwrap_or_default();
            // Assuming borrow_asset is implemented correctly:
            // this only panics if the circulating supply is somehow >u128::MAX
            // and we have somehow obtained >u128::MAX amount.
            //
            // NOTE: This is not necessary when working with NEP-141
            // tokens, which are required by standard to use 128-bit balances.
            //
            // Otherwise, borrow_asset is implemented incorrectly.
            // TODO: If that is the case, how to deal?
            //
            // Probably, it is okay to ignore this case. We can assume
            // that the configuration will only specify
            // correctly-implemented token contracts.
            #[allow(
                clippy::unwrap_used,
                reason = "Assume borrow asset is implemented correctly"
            )]
            yield_record.borrow_asset.join(share).unwrap();
            self.static_yield.insert(account_id, &yield_record);
        }

        // Next, dynamic (supply-based) yield.
        self.snapshot_with_yield_distribution(amount);
    }
}
