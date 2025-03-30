use near_sdk::{
    collections::{LookupMap, UnorderedMap},
    env, near, AccountId, BorshStorageKey, IntoStorageKey,
};

use crate::{
    asset::BorrowAssetAmount,
    borrow::{BorrowPosition, BorrowPositionGuard, BorrowPositionRef},
    chunked_append_only_list::ChunkedAppendOnlyList,
    event::MarketEvent,
    market::MarketConfiguration,
    number::Decimal,
    snapshot::Snapshot,
    static_yield::StaticYieldRecord,
    supply::{SupplyPosition, SupplyPositionGuard, SupplyPositionRef},
    withdrawal_queue::{error::WithdrawalQueueLockError, WithdrawalQueue},
};

use super::WithdrawalResolution;

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    SupplyPositions,
    BorrowPositions,
    Snapshots,
    WithdrawalQueue,
    StaticYield,
}

#[near]
pub struct Market {
    prefix: Vec<u8>,
    pub configuration: MarketConfiguration,
    pub borrow_asset_deposited: BorrowAssetAmount,
    pub borrow_asset_in_flight: BorrowAssetAmount,
    pub borrow_asset_borrowed: BorrowAssetAmount,
    pub(crate) supply_positions: UnorderedMap<AccountId, SupplyPosition>,
    pub(crate) borrow_positions: UnorderedMap<AccountId, BorrowPosition>,
    pub snapshots: ChunkedAppendOnlyList<Snapshot, 128>,
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
        let mut self_ = Self {
            prefix: prefix.clone(),
            configuration,
            borrow_asset_deposited: 0.into(),
            borrow_asset_in_flight: 0.into(),
            borrow_asset_borrowed: 0.into(),
            supply_positions: UnorderedMap::new(key!(SupplyPositions)),
            borrow_positions: UnorderedMap::new(key!(BorrowPositions)),
            snapshots: ChunkedAppendOnlyList::new(key!(Snapshots)),
            withdrawal_queue: WithdrawalQueue::new(key!(WithdrawalQueue)),
            static_yield: LookupMap::new(key!(StaticYield)),
        };

        // So we never have to worry about snapshots being empty.
        // This means that expressions like `self.snapshots.len() - 1` will never
        // underflow.
        self_.snapshot();

        self_
    }

    #[allow(clippy::unwrap_used, reason = "Snapshots are never empty")]
    pub fn get_last_snapshot(&self) -> &Snapshot {
        self.snapshots.get(self.snapshots.len() - 1).unwrap()
    }

    pub fn snapshot(&mut self) -> u32 {
        self.snapshot_with_yield_distribution(BorrowAssetAmount::zero())
    }

    fn snapshot_with_yield_distribution(&mut self, yield_distribution: BorrowAssetAmount) -> u32 {
        let time_chunk = self.configuration.time_chunk_configuration.now();

        if let Some((last_index, old_snapshot)) =
            self.snapshots.len().checked_sub(1).and_then(|last_index| {
                self.snapshots
                    .get(last_index)
                    .filter(|s| s.time_chunk == time_chunk)
                    .map(|s| (last_index, s))
            })
        {
            let new_snapshot = Snapshot {
                time_chunk,
                timestamp_ms: old_snapshot.timestamp_ms,
                deposited: self.borrow_asset_deposited,
                borrowed: self.borrow_asset_borrowed,
                yield_distribution: {
                    let mut y = old_snapshot.yield_distribution;
                    y.join(yield_distribution);
                    y
                },
            };
            self.snapshots.replace_last(new_snapshot);
            return last_index;
        }

        let index = self.snapshots.len();
        let new_snapshot = Snapshot {
            time_chunk,
            timestamp_ms: env::block_timestamp_ms().into(),
            deposited: self.borrow_asset_deposited,
            borrowed: self.borrow_asset_borrowed,
            yield_distribution,
        };
        self.snapshots.push(new_snapshot);
        if let Some(previous_snapshot_index) = index.checked_sub(1) {
            if let Some(previous_snapshot) = self.snapshots.get(previous_snapshot_index) {
                MarketEvent::SnapshotFinalized {
                    index: previous_snapshot_index,
                    snapshot: previous_snapshot.clone(),
                }
                .emit();
            }
        }
        index
    }

    pub fn get_borrow_asset_available_to_borrow(&self) -> BorrowAssetAmount {
        #[allow(
            clippy::unwrap_used,
            reason = "Factor is guaranteed to be <=1, so value must still fit in u128"
        )]
        let must_retain = ((1u32 - self.configuration.borrow_asset_maximum_usage_ratio)
            * Decimal::from(self.borrow_asset_deposited))
        .to_u128_ceil()
        .unwrap();

        u128::from(self.borrow_asset_deposited)
            .saturating_sub(u128::from(self.borrow_asset_borrowed))
            .saturating_sub(u128::from(self.borrow_asset_in_flight))
            .saturating_sub(must_retain)
            .into()
    }

    pub fn get_interest_rate_for_snapshot(&self, snapshot: &Snapshot) -> Decimal {
        self.configuration
            .borrow_interest_rate_strategy
            .at(snapshot.usage_ratio())
    }

    pub fn iter_supply_account_ids(&self) -> impl Iterator<Item = AccountId> + '_ {
        self.supply_positions.keys()
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

    pub fn iter_borrow_account_ids(&self) -> impl Iterator<Item = AccountId> + '_ {
        self.borrow_positions.keys()
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
                    let amount = supply_position
                        .inner()
                        .get_borrow_asset_deposit()
                        .min(requested_amount);

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
            supply_position.record_withdrawal(proof, amount, env::block_timestamp_ms());

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
        let total_amount = Decimal::from(u128::from(amount));
        let amount_per_weight = total_amount / total_weight;
        if !total_weight.is_zero() {
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
        }

        // Next, dynamic (supply-based) yield.
        self.snapshot_with_yield_distribution(amount);
    }
}
