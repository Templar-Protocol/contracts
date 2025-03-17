use std::{
    borrow::{Borrow, BorrowMut},
    ops::{Deref, DerefMut},
};

use near_sdk::{env, json_types::U64, near, AccountId};

use crate::{
    accumulator::{AccumulationRecord, Accumulator},
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount},
    event::MarketEvent,
    market::{Market, PricePair},
    number::Decimal,
    MS_IN_A_YEAR,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum BorrowStatus {
    Healthy,
    Liquidation(LiquidationReason),
}

impl BorrowStatus {
    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy)
    }

    pub fn is_liquidation(&self) -> bool {
        matches!(self, Self::Liquidation(..))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum LiquidationReason {
    Undercollateralization,
    Expiration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct BorrowPosition {
    pub started_at_block_timestamp_ms: Option<U64>,
    pub collateral_asset_deposit: CollateralAssetAmount,
    borrow_asset_principal: BorrowAssetAmount,
    pub borrow_asset_fees: Accumulator<BorrowAsset>,
    pub temporary_lock: BorrowAssetAmount,
    pub liquidation_lock: bool,
}

impl BorrowPosition {
    pub fn new(current_snapshot_index: u32) -> Self {
        Self {
            started_at_block_timestamp_ms: None,
            collateral_asset_deposit: 0.into(),
            borrow_asset_principal: 0.into(),
            // Start from current (not next) snapshot to avoid the possibility
            // of borrowing "for free". e.g. if TimeChunk units are epochs (12
            // hours), this prevents someone from getting 11 hours of free
            // borrowing if they create the borrow 1 hour into the epoch.
            borrow_asset_fees: Accumulator::new(current_snapshot_index),
            temporary_lock: 0.into(),
            liquidation_lock: false,
        }
    }

    pub(crate) fn full_liquidation(&mut self, current_snapshot_index: u32) {
        self.liquidation_lock = false;
        self.started_at_block_timestamp_ms = None;
        self.collateral_asset_deposit = 0.into();
        self.borrow_asset_principal = 0.into();
        self.borrow_asset_fees.clear(current_snapshot_index);
    }

    pub fn get_borrow_asset_principal(&self) -> BorrowAssetAmount {
        self.borrow_asset_principal
    }

    pub fn get_total_borrow_asset_liability(&self) -> BorrowAssetAmount {
        let mut total = BorrowAssetAmount::zero();
        total.join(self.borrow_asset_principal);
        total.join(self.borrow_asset_fees.get_total());
        total.join(self.temporary_lock);
        total
    }

    pub fn exists(&self) -> bool {
        !self.collateral_asset_deposit.is_zero()
            || !self.get_total_borrow_asset_liability().is_zero()
    }

    pub(crate) fn increase_collateral_asset_deposit(
        &mut self,
        amount: CollateralAssetAmount,
    ) -> Option<()> {
        self.collateral_asset_deposit.join(amount)
    }

    pub(crate) fn decrease_collateral_asset_deposit(
        &mut self,
        amount: CollateralAssetAmount,
    ) -> Option<CollateralAssetAmount> {
        self.collateral_asset_deposit.split(amount)
    }

    pub(crate) fn increase_borrow_asset_principal(
        &mut self,
        amount: BorrowAssetAmount,
        block_timestamp_ms: u64,
    ) -> Option<()> {
        if self.started_at_block_timestamp_ms.is_none()
            || self.get_total_borrow_asset_liability().is_zero()
        {
            self.started_at_block_timestamp_ms = Some(block_timestamp_ms.into());
        }
        self.borrow_asset_principal.join(amount)
    }

    pub(crate) fn reduce_borrow_asset_liability(
        &mut self,
        mut amount: BorrowAssetAmount,
    ) -> Result<LiabilityReduction, error::LiquidationLockError> {
        if self.liquidation_lock {
            return Err(error::LiquidationLockError);
        }

        // No bounds checks necessary here: the min() call prevents underflow.

        let amount_to_fees = self.borrow_asset_fees.get_total().min(amount);
        amount.split(amount_to_fees);
        self.borrow_asset_fees.remove(amount_to_fees);

        let amount_to_principal = self.borrow_asset_principal.min(amount);
        amount.split(amount_to_principal);
        self.borrow_asset_principal.split(amount_to_principal);

        if self.borrow_asset_principal.is_zero() {
            // fully paid off
            self.started_at_block_timestamp_ms = None;
        }

        Ok(LiabilityReduction {
            amount_to_fees,
            amount_to_principal,
            amount_remaining: amount,
        })
    }
}

pub struct LiabilityReduction {
    pub amount_to_fees: BorrowAssetAmount,
    pub amount_to_principal: BorrowAssetAmount,
    pub amount_remaining: BorrowAssetAmount,
}

pub mod error {
    use thiserror::Error;

    #[derive(Error, Debug)]
    #[error("This position is currently being liquidated.")]
    pub struct LiquidationLockError;
}

pub struct LinkedBorrowPosition<M> {
    market: M,
    account_id: AccountId,
    position: BorrowPosition,
}

impl<M> LinkedBorrowPosition<M> {
    pub fn new(market: M, account_id: AccountId, position: BorrowPosition) -> Self {
        Self {
            market,
            account_id,
            position,
        }
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    pub fn inner(&self) -> &BorrowPosition {
        &self.position
    }
}

impl<M: Borrow<Market>> LinkedBorrowPosition<M> {
    pub fn with_pending_interest(&mut self) {
        self.position.borrow_asset_fees.pending_estimate =
            self.calculate_interest(u32::MAX).get_amount();
        self.position
            .borrow_asset_fees
            .pending_estimate
            .join(self.calculate_last_snapshot_interest());
    }

    pub(crate) fn calculate_last_snapshot_interest(&self) -> BorrowAssetAmount {
        let market = self.market.borrow();
        let last_snapshot = market.get_last_snapshot();
        let interest_rate = market.get_interest_rate_for_snapshot(last_snapshot);
        let duration_ms = Decimal::from(env::block_timestamp_ms() - last_snapshot.timestamp_ms.0);
        let ms_in_a_year = Decimal::from(MS_IN_A_YEAR);
        let interest_rate_part = interest_rate * duration_ms / ms_in_a_year;
        let interest = interest_rate_part
            * Decimal::from(self.position.get_borrow_asset_principal().as_u128());

        interest.to_u128_ceil().unwrap().into()
    }

    pub(crate) fn calculate_interest(&self, limit: u32) -> AccumulationRecord<BorrowAsset> {
        let principal = Decimal::from(self.position.get_borrow_asset_principal().as_u128());
        let mut next_snapshot_index = self.position.borrow_asset_fees.get_next_snapshot_index();

        let mut accumulated = Decimal::ZERO;

        // Assume # of snapshots will never be > u32::MAX.
        #[allow(clippy::cast_possible_truncation)]
        let mut it = self
            .market
            .borrow()
            .snapshots
            .iter()
            .enumerate()
            .skip(next_snapshot_index as usize)
            .take(limit as usize)
            .map(|(i, s)| (i as u32, s))
            .peekable();

        let ms_in_a_year = Decimal::from(MS_IN_A_YEAR);

        while let Some((i, snapshot)) = it.next() {
            let Some(end_timestamp_ms) = it
                .peek()
                .map(|(_, next_snapshot)| next_snapshot.timestamp_ms.0)
            else {
                // Cannot calculate duration.
                break;
            };

            let total_borrowed = Decimal::from(snapshot.borrowed.as_u128());
            let total_deposited = Decimal::from(snapshot.deposited.as_u128());
            let utilization_ratio = total_borrowed / total_deposited;
            let interest_rate_per_year = self
                .market
                .borrow()
                .configuration
                .borrow_interest_rate_strategy
                .at(utilization_ratio);
            let duration_ms: Decimal = end_timestamp_ms
                .checked_sub(snapshot.timestamp_ms.0)
                .unwrap_or_else(|| {
                    env::panic_str(&format!(
                        "Invariant violation: Snapshot timestamp decrease at #{}.",
                        i + 1,
                    ))
                })
                .into();

            let interest = principal * interest_rate_per_year * duration_ms / ms_in_a_year;

            accumulated += interest;

            next_snapshot_index = i + 1;
        }

        AccumulationRecord {
            amount: accumulated.to_u128_ceil().unwrap().into(),
            next_snapshot_index,
        }
    }

    pub fn can_be_liquidated(&self, price_pair: &PricePair, block_timestamp_ms: u64) -> bool {
        self.market
            .borrow()
            .configuration
            .borrow_status(&self.position, price_pair, block_timestamp_ms)
            .is_liquidation()
    }

    pub fn is_within_minimum_initial_collateral_ratio(&self, price_pair: &PricePair) -> bool {
        self.market
            .borrow()
            .configuration
            .is_within_minimum_initial_collateral_ratio(&self.position, price_pair)
    }

    pub fn is_within_minimum_collateral_ratio(&self, price_pair: &PricePair) -> bool {
        self.market
            .borrow()
            .configuration
            .is_within_minimum_collateral_ratio(&self.position, price_pair)
    }

    pub fn minimum_acceptable_liquidation_amount(
        &self,
        price_pair: &PricePair,
    ) -> BorrowAssetAmount {
        self.market
            .borrow()
            .configuration
            .minimum_acceptable_liquidation_amount(
                self.position.collateral_asset_deposit,
                price_pair,
            )
    }
}

pub struct LinkedBorrowPositionMut<M: BorrowMut<Market>>(LinkedBorrowPosition<M>);

impl<M: BorrowMut<Market>> Drop for LinkedBorrowPositionMut<M> {
    fn drop(&mut self) {
        self.0
            .market
            .borrow_mut()
            .borrow_positions
            .insert(&self.0.account_id, &self.0.position);
    }
}

impl<M: BorrowMut<Market>> Deref for LinkedBorrowPositionMut<M> {
    type Target = LinkedBorrowPosition<M>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<M: BorrowMut<Market>> DerefMut for LinkedBorrowPositionMut<M> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<M: BorrowMut<Market>> LinkedBorrowPositionMut<M> {
    pub fn new(market: M, account_id: AccountId, position: BorrowPosition) -> Self {
        Self(LinkedBorrowPosition::new(market, account_id, position))
    }

    pub fn record_collateral_asset_deposit(&mut self, amount: CollateralAssetAmount) {
        self.accumulate_interest();

        self.position
            .increase_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset overflow"));

        MarketEvent::CollateralDeposited {
            account_id: self.account_id.clone(),
            collateral_asset_amount: amount,
        }
        .emit();
    }

    pub fn record_collateral_asset_withdrawal(&mut self, amount: CollateralAssetAmount) {
        self.accumulate_interest();

        self.position
            .decrease_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset underflow"));

        MarketEvent::CollateralWithdrawn {
            account_id: self.account_id.clone(),
            collateral_asset_amount: amount,
        }
        .emit();
    }

    pub fn record_borrow_asset_in_flight_start(
        &mut self,
        amount: BorrowAssetAmount,
        fees: BorrowAssetAmount,
    ) {
        self.accumulate_interest();

        self.market
            .borrow_mut()
            .borrow_asset_in_flight
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset in flight amount overflow"));
        self.position
            .temporary_lock
            .join(amount)
            .and_then(|()| self.position.temporary_lock.join(fees))
            .unwrap_or_else(|| env::panic_str("Borrow position in flight amount overflow"));
    }

    pub fn record_borrow_asset_in_flight_end(
        &mut self,
        amount: BorrowAssetAmount,
        fees: BorrowAssetAmount,
    ) {
        self.accumulate_interest();

        // This should never panic, because a given amount of in-flight borrow
        // asset should always be added before it is removed.
        self.market
            .borrow_mut()
            .borrow_asset_in_flight
            .split(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset in flight amount underflow"));
        self.position
            .temporary_lock
            .split(amount)
            .and_then(|_| self.position.temporary_lock.split(fees))
            .unwrap_or_else(|| env::panic_str("Borrow position in flight amount underflow"));
    }

    pub fn record_borrow_asset_withdrawal(
        &mut self,
        amount: BorrowAssetAmount,
        fees: BorrowAssetAmount,
    ) {
        self.accumulate_interest();

        self.position.borrow_asset_fees.add_once(fees);
        self.position
            .increase_borrow_asset_principal(amount, env::block_timestamp_ms())
            .unwrap_or_else(|| env::panic_str("Increase borrow asset principal overflow"));

        self.market
            .borrow_mut()
            .borrow_asset_borrowed
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset borrowed overflow"));
        self.market.borrow_mut().snapshot();

        MarketEvent::BorrowWithdrawn {
            account_id: self.account_id.clone(),
            borrow_asset_amount: amount,
        }
        .emit();
    }

    pub fn record_repay(&mut self, amount: BorrowAssetAmount) -> BorrowAssetAmount {
        self.accumulate_interest();

        let liability_reduction = self
            .position
            .reduce_borrow_asset_liability(amount)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));

        self.market
            .borrow_mut()
            .record_borrow_asset_yield_distribution(liability_reduction.amount_to_fees);

        // SAFETY: It should be impossible to panic here, since assets that
        // have not yet been borrowed cannot be repaid.
        self.market
            .borrow_mut()
            .borrow_asset_borrowed
            .split(liability_reduction.amount_to_principal)
            .unwrap_or_else(|| env::panic_str("Borrow asset borrowed underflow"));

        self.market.borrow_mut().snapshot();

        MarketEvent::BorrowRepaid {
            account_id: self.account_id.clone(),
            borrow_asset_fees_repaid: liability_reduction.amount_to_fees,
            borrow_asset_principal_repaid: liability_reduction.amount_to_principal,
            borrow_asset_principal_remaining: self.position.get_borrow_asset_principal(),
        }
        .emit();

        liability_reduction.amount_remaining
    }

    pub fn accumulate_interest(&mut self) {
        self.market.borrow_mut().snapshot();

        let accumulation_record = self.calculate_interest(u32::MAX);

        MarketEvent::InterestAccumulated {
            account_id: self.account_id.clone(),
            borrow_asset_amount: accumulation_record.amount,
        }
        .emit();

        self.position
            .borrow_asset_fees
            .accumulate(accumulation_record);
    }

    pub fn liquidation_lock(&mut self) {
        self.position.liquidation_lock = true;
    }

    pub fn liquidation_unlock(&mut self) {
        self.position.liquidation_lock = false;
    }

    pub fn record_full_liquidation(
        &mut self,
        liquidator_id: AccountId,
        mut recovered_amount: BorrowAssetAmount,
    ) {
        let principal = self.position.get_borrow_asset_principal();
        let collateral_asset_liquidated = self.position.collateral_asset_deposit;

        MarketEvent::FullLiquidation {
            liquidator_id,
            account_id: self.account_id.clone(),
            borrow_asset_principal: principal,
            borrow_asset_recovered: recovered_amount,
            collateral_asset_liquidated,
        }
        .emit();

        let snapshot_index = self.market.borrow_mut().snapshot();
        self.position.full_liquidation(snapshot_index);

        self.market
            .borrow_mut()
            .borrow_asset_borrowed
            .split(principal);

        // TODO: Is it correct to only care about the original principal here?
        if recovered_amount.split(principal).is_some() {
            // distribute yield
            // record_borrow_asset_yield_distribution will take snapshot, no need to do it.
            self.market
                .borrow_mut()
                .record_borrow_asset_yield_distribution(recovered_amount);
        } else {
            // we took a loss
            // TODO: some sort of recovery for suppliers
            //
            // Might look something like this:
            // self.borrow_asset_deposited.split(principal);
            // (?)
            todo!("Took a loss during liquidation");
        }
    }
}
