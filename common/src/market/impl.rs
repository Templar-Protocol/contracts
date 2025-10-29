use near_sdk::{
    collections::{LookupMap, UnorderedMap},
    env, near, AccountId, BorshStorageKey, IntoStorageKey,
};

use crate::{
    accumulator::{AccumulationRecord, Accumulator},
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowPositionGuard, BorrowPositionRef},
    chunked_append_only_list::ChunkedAppendOnlyList,
    event::MarketEvent,
    incoming_deposit::IncomingDeposit,
    market::MarketConfiguration,
    number::Decimal,
    snapshot::Snapshot,
    supply::{SupplyPosition, SupplyPositionGuard, SupplyPositionRef},
    time_chunk::TimeChunk,
    withdrawal_queue::WithdrawalQueue,
    YEAR_PER_MS,
};

#[derive(Debug, Copy, Clone)]
pub struct SnapshotProof(());

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
    /// Upcoming snapshot indices with amounts of borrow asset that will be activated.
    pub borrow_asset_deposited_incoming: Vec<IncomingDeposit>,
    pub borrow_asset_withdrawal_in_flight: BorrowAssetAmount,
    /// Sending borrow asset out, because if somebody sends the contract borrow asset, it's ok for the
    /// contract to attempt to fulfill withdrawal request, even if the market thinks it doesn't have
    /// enough to fulfill.
    pub borrow_asset_borrowed_in_flight: BorrowAssetAmount,
    /// Amount of borrow asset that has been withdrawn (is in use by) by borrowers.
    ///
    /// `borrow_asset_deposited_active - borrow_asset_borrowed - borrow_asset_borrowed_in_flight >= 0` should always be true.
    pub borrow_asset_borrowed: BorrowAssetAmount,
    /// Market-wide collateral asset deposit tracking.
    pub collateral_asset_deposited: CollateralAssetAmount,
    pub(crate) supply_positions: UnorderedMap<AccountId, SupplyPosition>,
    pub(crate) borrow_positions: UnorderedMap<AccountId, BorrowPosition>,
    pub current_time_chunk: TimeChunk,
    pub current_yield_distribution: BorrowAssetAmount,
    pub finalized_snapshots: ChunkedAppendOnlyList<Snapshot, 32>,
    pub withdrawal_queue: WithdrawalQueue,
    pub static_yield: LookupMap<AccountId, Accumulator<BorrowAsset>>,
    single_snapshot_maximum_interest_precomputed: Decimal,
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
        let last_time_chunk = configuration.time_chunk_configuration.now();

        let single_snapshot_maximum_interest_precomputed =
            configuration.single_snapshot_maximum_interest();

        let mut self_ = Self {
            prefix: prefix.clone(),
            configuration,
            borrow_asset_deposited_active: 0.into(),
            borrow_asset_deposited_incoming: Vec::new(),
            borrow_asset_withdrawal_in_flight: 0.into(),
            borrow_asset_borrowed_in_flight: 0.into(),
            borrow_asset_borrowed: 0.into(),
            collateral_asset_deposited: 0.into(),
            supply_positions: UnorderedMap::new(key!(SupplyPositions)),
            borrow_positions: UnorderedMap::new(key!(BorrowPositions)),
            current_time_chunk: last_time_chunk,
            current_yield_distribution: 0.into(),
            finalized_snapshots: ChunkedAppendOnlyList::new(key!(FinalizedSnapshots)),
            withdrawal_queue: WithdrawalQueue::new(key!(WithdrawalQueue)),
            static_yield: LookupMap::new(key!(StaticYield)),
            single_snapshot_maximum_interest_precomputed,
        };

        self_.finalized_snapshots.push(first_snapshot);

        self_
    }

    pub fn borrowed(&self) -> BorrowAssetAmount {
        self.borrow_asset_borrowed + self.borrow_asset_borrowed_in_flight
    }

    pub fn total_incoming(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposited_incoming
            .iter()
            .fold(BorrowAssetAmount::zero(), |total_incoming, incoming| {
                total_incoming + incoming.amount
            })
    }

    pub fn incoming_at(&self, snapshot_index: u32) -> BorrowAssetAmount {
        self.borrow_asset_deposited_incoming
            .iter()
            .find_map(|incoming| {
                (incoming.activate_at_snapshot_index == snapshot_index).then_some(incoming.amount)
            })
            .unwrap_or(0.into())
    }

    pub fn get_last_finalized_snapshot(&self) -> &Snapshot {
        #[allow(clippy::unwrap_used, reason = "Snapshots are never empty")]
        self.finalized_snapshots
            .get(self.finalized_snapshots.len() - 1)
            .unwrap()
    }

    pub fn current_snapshot(&self) -> Snapshot {
        let current_snapshot_index = self.finalized_snapshots.len();
        let incoming = self.incoming_at(current_snapshot_index);

        let active = self.borrow_asset_deposited_active + incoming;

        let borrowed = self.borrowed();

        let interest_rate = self
            .configuration
            .borrow_interest_rate_strategy
            .at(usage_ratio(active, borrowed));

        Snapshot {
            time_chunk: self.current_time_chunk,
            end_timestamp_ms: env::block_timestamp_ms().into(),
            borrow_asset_deposited_active: active,
            borrow_asset_borrowed: borrowed,
            collateral_asset_deposited: self.collateral_asset_deposited,
            yield_distribution: self.current_yield_distribution,
            interest_rate,
        }
    }

    pub fn snapshot(&mut self) -> SnapshotProof {
        let now = self.configuration.time_chunk_configuration.now();

        // Do we need to finalize the current snapshot?
        if self.current_time_chunk == now {
            return SnapshotProof(());
        }

        let snapshot = self.current_snapshot();
        let current_snapshot_index = self.finalized_snapshots.len();

        // Emit event and push finalized snapshot
        MarketEvent::SnapshotFinalized {
            index: current_snapshot_index,
            snapshot: snapshot.clone(),
        }
        .emit();
        self.finalized_snapshots.push(snapshot);

        // We just pushed a snapshot
        let current_snapshot_index = current_snapshot_index + 1;

        // Activate incoming funds
        for i in 0..self.borrow_asset_deposited_incoming.len() {
            let incoming = &self.borrow_asset_deposited_incoming[i];
            if incoming.activate_at_snapshot_index == current_snapshot_index {
                self.borrow_asset_deposited_active += incoming.amount;
                self.borrow_asset_deposited_incoming.remove(i);
                break;
            }
        }

        // Reset for the new time chunk
        self.current_time_chunk = now;
        self.current_yield_distribution = 0.into();

        SnapshotProof(())
    }

    pub fn single_snapshot_fee(&self, amount: BorrowAssetAmount) -> Option<BorrowAssetAmount> {
        (u128::from(amount) * self.single_snapshot_maximum_interest_precomputed)
            .to_u128_ceil()
            .map(Into::into)
    }

    pub fn interest_rate(&self) -> Decimal {
        self.configuration
            .borrow_interest_rate_strategy
            .at(usage_ratio(
                self.borrow_asset_deposited_active,
                self.borrowed(),
            ))
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
            .saturating_sub(u128::from(self.borrowed()))
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

    pub fn supply_position_guard(
        &mut self,
        _proof: SnapshotProof,
        account_id: AccountId,
    ) -> Option<SupplyPositionGuard> {
        self.supply_positions
            .get(&account_id)
            .map(|position| SupplyPositionGuard::new(self, account_id, position))
    }

    pub fn get_or_create_supply_position_guard(
        &mut self,
        _proof: SnapshotProof,
        account_id: AccountId,
    ) -> SupplyPositionGuard {
        let position = self
            .supply_positions
            .get(&account_id)
            .unwrap_or_else(|| SupplyPosition::new(self.finalized_snapshots.len()));

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

    pub fn borrow_position_guard(
        &mut self,
        _proof: SnapshotProof,
        account_id: AccountId,
    ) -> Option<BorrowPositionGuard> {
        self.borrow_positions
            .get(&account_id)
            .map(|position| BorrowPositionGuard::new(self, account_id, position))
    }

    pub fn get_or_create_borrow_position_guard(
        &mut self,
        _proof: SnapshotProof,
        account_id: AccountId,
    ) -> BorrowPositionGuard {
        let position = self
            .borrow_positions
            .get(&account_id)
            .unwrap_or_else(|| BorrowPosition::new(self.finalized_snapshots.len()));

        BorrowPositionGuard::new(self, account_id, position)
    }

    pub fn cleanup_borrow_position(&mut self, account_id: &AccountId) -> bool {
        self.borrow_positions
            .get(account_id)
            .filter(|p| !p.exists())
            .and_then(|_| self.borrow_positions.remove(account_id))
            .is_some()
    }

    pub fn record_borrow_asset_protocol_yield(&mut self, amount: BorrowAssetAmount) {
        let mut yield_record = self
            .static_yield
            .get(&self.configuration.protocol_account_id)
            .unwrap_or_else(|| Accumulator::new(1));

        yield_record.add_once(amount);

        self.static_yield
            .insert(&self.configuration.protocol_account_id, &yield_record);
    }

    pub fn record_borrow_asset_yield_distribution(&mut self, amount: BorrowAssetAmount) {
        // Sanity.
        if amount.is_zero() {
            return;
        }

        self.current_yield_distribution += amount;
    }

    /// Accumulate static yield for an account.
    ///
    /// # Errors
    ///
    /// - When the account is not configured to earn static yield.
    pub fn accumulate_static_yield(
        &mut self,
        account_id: &AccountId,
        snapshot_limit: u32,
    ) -> Result<(), UnknownAccount> {
        let weight_numerator = *self
            .configuration
            .yield_weights
            .r#static
            .get(account_id)
            .ok_or(UnknownAccount)?;
        let weight_denominator = self.configuration.yield_weights.total_weight().get();
        let mut accumulator = self
            .static_yield
            .get(account_id)
            .unwrap_or_else(|| Accumulator::new(1));

        let mut next_snapshot_index = accumulator.get_next_snapshot_index();
        let mut accumulated = Decimal::ZERO;

        #[allow(clippy::unwrap_used, reason = "Guaranteed previous snapshot exists")]
        let mut prev_end_timestamp_ms = self
            .finalized_snapshots
            .get(next_snapshot_index.checked_sub(1).unwrap())
            .unwrap()
            .end_timestamp_ms
            .0;

        #[allow(
            clippy::cast_possible_truncation,
            reason = "Assume # of snapshots is never >u32::MAX"
        )]
        for (i, snapshot) in self
            .finalized_snapshots
            .iter()
            .enumerate()
            .skip(next_snapshot_index as usize)
            .take(snapshot_limit as usize)
        {
            let snapshot_duration_ms = snapshot.end_timestamp_ms.0 - prev_end_timestamp_ms;
            let interest_paid_by_borrowers = Decimal::from(snapshot.borrow_asset_borrowed)
                * snapshot.interest_rate
                * snapshot_duration_ms
                * YEAR_PER_MS;
            let other_yield = Decimal::from(snapshot.yield_distribution);
            accumulated +=
                (interest_paid_by_borrowers + other_yield) * weight_numerator / weight_denominator;

            next_snapshot_index = i as u32 + 1;
            prev_end_timestamp_ms = snapshot.end_timestamp_ms.0;
        }

        let accumulation_record = AccumulationRecord {
            // Accumulated amount is derived from real balances, so it should
            // never overflow underlying data type.
            #[allow(clippy::unwrap_used, reason = "Derived from real balances")]
            amount: accumulated.to_u128_floor().unwrap().into(),
            fraction_as_u128_dividend: accumulated.fractional_part_as_u128_dividend(),
            next_snapshot_index,
        };

        accumulator.accumulate(accumulation_record);

        self.static_yield.insert(account_id, &accumulator);

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("This account does not earn static yield")]
pub struct UnknownAccount;

fn usage_ratio(active: BorrowAssetAmount, borrowed: BorrowAssetAmount) -> Decimal {
    if active.is_zero() || borrowed.is_zero() {
        Decimal::ZERO
    } else if borrowed >= active {
        Decimal::ONE
    } else {
        Decimal::from(borrowed) / Decimal::from(active)
    }
}
