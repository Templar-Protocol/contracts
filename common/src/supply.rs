use std::{
    borrow::{Borrow, BorrowMut},
    ops::{Deref, DerefMut},
};

use near_sdk::{env, json_types::U64, near, AccountId};

use crate::{
    accumulator::{AccumulationRecord, Accumulator},
    asset::{BorrowAsset, BorrowAssetAmount},
    event::MarketEvent,
    market::{Market, WithdrawalResolution},
    number::Decimal,
};

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct SupplyPosition {
    pub started_at_block_timestamp_ms: Option<U64>,
    borrow_asset_deposit: BorrowAssetAmount,
    pub borrow_asset_yield: Accumulator<BorrowAsset>,
}

impl SupplyPosition {
    pub fn new(current_snapshot_index: u32) -> Self {
        Self {
            started_at_block_timestamp_ms: None,
            borrow_asset_deposit: 0.into(),
            // We start at next log index so that the supply starts
            // accumulating yield from the _next_ log (since they were not
            // necessarily supplying for all of the current log).
            borrow_asset_yield: Accumulator::new(current_snapshot_index + 1),
        }
    }

    pub fn get_borrow_asset_deposit(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposit
    }

    pub fn get_started_at_block_timestamp_ms(&self) -> Option<u64> {
        self.started_at_block_timestamp_ms.map(u64::from)
    }

    pub fn exists(&self) -> bool {
        !self.borrow_asset_deposit.is_zero() || !self.borrow_asset_yield.get_total().is_zero()
    }

    /// MUST always be paired with a yield recalculation!
    pub(crate) fn increase_borrow_asset_deposit(
        &mut self,
        amount: BorrowAssetAmount,
        block_timestamp_ms: u64,
    ) -> Option<()> {
        if self.started_at_block_timestamp_ms.is_none() || self.borrow_asset_deposit.is_zero() {
            self.started_at_block_timestamp_ms = Some(block_timestamp_ms.into());
        }
        self.borrow_asset_deposit.join(amount)
    }

    /// MUST always be paired with a yield recalculation!
    pub(crate) fn decrease_borrow_asset_deposit(
        &mut self,
        amount: BorrowAssetAmount,
    ) -> Option<BorrowAssetAmount> {
        // No need to reset the timer; it is a permanent indication of the
        // initial supply event.
        self.borrow_asset_deposit.split(amount)
    }
}

pub struct LinkedSupplyPosition<M> {
    market: M,
    account_id: AccountId,
    position: SupplyPosition,
}

impl<M> LinkedSupplyPosition<M> {
    pub fn new(market: M, account_id: AccountId, position: SupplyPosition) -> Self {
        Self {
            market,
            account_id,
            position,
        }
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    pub fn inner(&self) -> &SupplyPosition {
        &self.position
    }
}

impl<M: Borrow<Market>> LinkedSupplyPosition<M> {
    pub fn with_pending_yield_estimate(&mut self) {
        self.position.borrow_asset_yield.pending_estimate = self.calculate_yield().get_amount();
        self.position
            .borrow_asset_yield
            .pending_estimate
            .join(self.calculate_last_snapshot_yield());
    }

    pub fn calculate_last_snapshot_yield(&self) -> BorrowAssetAmount {
        let deposit: Decimal = self.position.get_borrow_asset_deposit().into();
        if deposit.is_zero() {
            return BorrowAssetAmount::zero();
        }

        let last_snapshot = self.market.borrow().get_last_snapshot();
        let total_deposited: Decimal = last_snapshot.deposited.into();
        if total_deposited.is_zero() {
            // divzero safety
            return BorrowAssetAmount::zero();
        }
        let supply_weight = Decimal::from(
            self.market
                .borrow()
                .configuration
                .yield_weights
                .supply
                .get(),
        );
        // This is guaranteed to be nonzero, so no divzero issue.
        let total_weight = Decimal::from(
            self.market
                .borrow()
                .configuration
                .yield_weights
                .total_weight()
                .get(),
        );
        let total_yield_distribution: Decimal = last_snapshot.yield_distribution.into();
        let estimate_current_snapshot =
            total_yield_distribution * deposit * supply_weight / total_deposited / total_weight;

        estimate_current_snapshot.to_u128_floor().unwrap().into()
    }

    pub fn calculate_yield(&self) -> AccumulationRecord<BorrowAsset> {
        let mut next_snapshot_index = self.position.borrow_asset_yield.get_next_snapshot_index();

        if self.market.borrow().snapshots.is_empty() {
            return AccumulationRecord::empty(next_snapshot_index);
        }

        let amount: Decimal = self.position.get_borrow_asset_deposit().into();

        let mut accumulated = Decimal::ZERO;

        let mut it = self.market.borrow().snapshots.iter();
        // Skip the last snapshot, which may be incomplete.
        it.next_back();

        for (i, snapshot) in it.enumerate().skip(next_snapshot_index as usize).map(
            // Assume # of snapshots is never >u32::MAX.
            #[allow(clippy::cast_possible_truncation)]
            |(i, s)| (i as u32, s),
        ) {
            accumulated += amount * Decimal::from(snapshot.yield_distribution)
                / Decimal::from(snapshot.deposited);

            next_snapshot_index = i + 1;
        }

        AccumulationRecord {
            amount: accumulated.to_u128_floor().unwrap().into(),
            next_snapshot_index,
        }
    }
}

pub struct LinkedSupplyPositionMut<M: BorrowMut<Market>>(LinkedSupplyPosition<M>);

impl<M: BorrowMut<Market>> Drop for LinkedSupplyPositionMut<M> {
    fn drop(&mut self) {
        self.0
            .market
            .borrow_mut()
            .supply_positions
            .insert(&self.0.account_id, &self.0.position);
    }
}

impl<M: BorrowMut<Market>> Deref for LinkedSupplyPositionMut<M> {
    type Target = LinkedSupplyPosition<M>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<M: BorrowMut<Market>> DerefMut for LinkedSupplyPositionMut<M> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<M: BorrowMut<Market>> LinkedSupplyPositionMut<M> {
    pub fn new(market: M, account_id: AccountId, position: SupplyPosition) -> Self {
        Self(LinkedSupplyPosition::new(market, account_id, position))
    }

    /// In order for yield calculations to be accurate, this function MUST
    /// BE CALLED every time a supply position's deposit changes. This
    /// requirement is largely met by virtue of the fact that
    /// `SupplyPosition->borrow_asset_deposit` is a private field and can only
    /// be modified via methods on this type.
    pub fn accumulate_yield(&mut self) {
        self.market.borrow_mut().snapshot();

        let accumulation_record = self.calculate_yield();

        if !accumulation_record.amount.is_zero() {
            MarketEvent::YieldAccumulated {
                account_id: self.account_id.clone(),
                borrow_asset_amount: accumulation_record.amount,
            }
            .emit();
        }

        self.position
            .borrow_asset_yield
            .accumulate(accumulation_record);
    }

    pub fn record_withdrawal(
        &mut self,
        mut amount: BorrowAssetAmount,
        block_timestamp_ms: u64,
    ) -> WithdrawalResolution {
        self.accumulate_yield();

        self.position
            .decrease_borrow_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset underflow"));

        self.market
            .borrow_mut()
            .borrow_asset_deposited
            .split(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited underflow"));

        self.market.borrow_mut().snapshot();

        let started_at_block_timestamp_ms =
            self.0.position.started_at_block_timestamp_ms.unwrap().0;
        let supply_duration = block_timestamp_ms.saturating_sub(started_at_block_timestamp_ms);

        let amount_to_fees = self
            .market
            .borrow()
            .configuration
            .supply_withdrawal_fee
            .of(amount, supply_duration)
            .unwrap();

        amount.split(amount_to_fees).unwrap();

        MarketEvent::SupplyWithdrawn {
            account_id: self.account_id.clone(),
            borrow_asset_amount_to_account: amount,
            borrow_asset_amount_to_fees: amount_to_fees,
        }
        .emit();

        WithdrawalResolution {
            account_id: self.account_id.clone(),
            amount_to_account: amount,
            amount_to_fees,
        }
    }

    pub fn record_deposit(&mut self, amount: BorrowAssetAmount, block_timestamp_ms: u64) {
        self.accumulate_yield();

        self.position
            .increase_borrow_asset_deposit(amount, block_timestamp_ms)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset overflow"));

        self.market
            .borrow_mut()
            .borrow_asset_deposited
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited overflow"));

        self.market.borrow_mut().snapshot();

        MarketEvent::SupplyDeposited {
            account_id: self.account_id.clone(),
            borrow_asset_amount: amount,
        }
        .emit();
    }

    pub fn record_yield_withdrawal(
        &mut self,
        amount: BorrowAssetAmount,
    ) -> Option<BorrowAssetAmount> {
        self.0.position.borrow_asset_yield.remove(amount)
    }
}
