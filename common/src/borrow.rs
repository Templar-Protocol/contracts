use std::ops::{Deref, DerefMut};

use near_sdk::{env, json_types::U64, near, AccountId};

use crate::{
    accumulator::{AccumulationRecord, Accumulator},
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount},
    event::MarketEvent,
    market::{Market, PricePair},
    number::Decimal,
    snapshot::Snapshot,
    MS_IN_A_YEAR,
};

/// This struct can only be constructed after accumulating interest on a
/// borrow position. This serves as proof that the interest has accrued, so it
/// is safe to perform certain other operations.
#[derive(Clone, Copy)]
pub struct InterestAccumulationProof(());

#[cfg(test)]
impl InterestAccumulationProof {
    pub fn test() -> Self {
        Self(())
    }
}

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

    /// Interest accumulation MUST be applied before calling this function.
    pub(crate) fn increase_borrow_asset_principal(
        &mut self,
        _proof: InterestAccumulationProof,
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

    /// Interest accumulation MUST be applied before calling this function.
    pub(crate) fn reduce_borrow_asset_liability(
        &mut self,
        _proof: InterestAccumulationProof,
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

pub struct BorrowPositionRef<M> {
    market: M,
    account_id: AccountId,
    position: BorrowPosition,
}

impl<M> BorrowPositionRef<M> {
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

impl<M: Deref<Target = Market>> BorrowPositionRef<M> {
    pub fn with_pending_interest(&mut self) {
        let mut pending_estimate = self.calculate_interest(u32::MAX).get_amount();
        let prev_end_timestamp_ms = self.market.get_last_finalized_snapshot().end_timestamp_ms.0;
        let interest_in_current_snapshot =
            self.calculate_interest_rate_for_snapshot(
                prev_end_timestamp_ms,
                &self.market.current_snapshot,
            ) * Decimal::from(self.position.get_borrow_asset_principal());
        #[allow(clippy::unwrap_used, reason = "This is a view method")]
        pending_estimate.join(interest_in_current_snapshot.to_u128_ceil().unwrap().into());

        self.position.borrow_asset_fees.pending_estimate = pending_estimate;
    }

    pub(crate) fn calculate_interest_rate_for_snapshot(
        &self,
        prev_end_timestamp_ms: u64,
        snapshot: &Snapshot,
    ) -> Decimal {
        let interest_rate_per_year = self
            .market
            .configuration
            .borrow_interest_rate_strategy
            .at(snapshot.usage_ratio());
        let duration_ms = Decimal::from(
            snapshot
                .end_timestamp_ms
                .0
                .checked_sub(prev_end_timestamp_ms)
                .unwrap_or_else(|| {
                    env::panic_str(&format!(
                        "Invariant violation: Snapshot timestamp decrease at time chunk #{}.",
                        u64::from(snapshot.time_chunk.0),
                    ))
                }),
        );

        interest_rate_per_year * duration_ms / *MS_IN_A_YEAR
    }

    pub(crate) fn calculate_interest(
        &self,
        snapshot_limit: u32,
    ) -> AccumulationRecord<BorrowAsset> {
        let principal: Decimal = self.position.get_borrow_asset_principal().into();
        let mut next_snapshot_index = self.position.borrow_asset_fees.get_next_snapshot_index();

        let mut accumulated = Decimal::ZERO;
        #[allow(clippy::unwrap_used, reason = "1 finalized snapshot guaranteed")]
        let mut prev_end_timestamp_ms = self
            .market
            .finalized_snapshots
            .get(next_snapshot_index.checked_sub(1).unwrap())
            .unwrap()
            .end_timestamp_ms
            .0;

        #[allow(
            clippy::cast_possible_truncation,
            reason = "Assume # of snapshots will never be > u32::MAX"
        )]
        for (i, snapshot) in self
            .market
            .finalized_snapshots
            .iter()
            .enumerate()
            .skip(next_snapshot_index as usize)
            .take(snapshot_limit as usize)
        {
            accumulated += principal
                * self.calculate_interest_rate_for_snapshot(prev_end_timestamp_ms, snapshot);

            prev_end_timestamp_ms = snapshot.end_timestamp_ms.0;
            next_snapshot_index = i as u32 + 1;
        }

        AccumulationRecord {
            #[allow(
                clippy::unwrap_used,
                reason = "Assume accumulated interest will never exceed u128::MAX"
            )]
            amount: accumulated.to_u128_ceil().unwrap().into(),
            next_snapshot_index,
        }
    }

    pub fn can_be_liquidated(&self, price_pair: &PricePair, block_timestamp_ms: u64) -> bool {
        self.market
            .configuration
            .borrow_status(&self.position, price_pair, block_timestamp_ms)
            .is_liquidation()
    }

    pub fn is_within_minimum_initial_collateral_ratio(&self, price_pair: &PricePair) -> bool {
        self.market
            .configuration
            .is_within_minimum_initial_collateral_ratio(&self.position, price_pair)
    }

    pub fn is_within_minimum_collateral_ratio(&self, price_pair: &PricePair) -> bool {
        self.market
            .configuration
            .is_within_minimum_collateral_ratio(&self.position, price_pair)
    }

    pub fn minimum_acceptable_liquidation_amount(
        &self,
        price_pair: &PricePair,
    ) -> Option<BorrowAssetAmount> {
        self.market
            .configuration
            .minimum_acceptable_liquidation_amount(
                self.position.collateral_asset_deposit,
                price_pair,
            )
    }
}

pub struct BorrowPositionGuard<'a>(BorrowPositionRef<&'a mut Market>);

impl Drop for BorrowPositionGuard<'_> {
    fn drop(&mut self) {
        self.0
            .market
            .borrow_positions
            .insert(&self.0.account_id, &self.0.position);
    }
}

impl<'a> Deref for BorrowPositionGuard<'a> {
    type Target = BorrowPositionRef<&'a mut Market>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for BorrowPositionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a> BorrowPositionGuard<'a> {
    pub fn new(market: &'a mut Market, account_id: AccountId, position: BorrowPosition) -> Self {
        Self(BorrowPositionRef::new(market, account_id, position))
    }

    pub fn record_collateral_asset_deposit(
        &mut self,
        _proof: InterestAccumulationProof,
        amount: CollateralAssetAmount,
    ) {
        self.position
            .increase_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset overflow"));

        MarketEvent::CollateralDeposited {
            account_id: self.account_id.clone(),
            collateral_asset_amount: amount,
        }
        .emit();
    }

    pub fn record_collateral_asset_withdrawal(
        &mut self,
        _proof: InterestAccumulationProof,
        amount: CollateralAssetAmount,
    ) {
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
        _proof: InterestAccumulationProof,
        amount: BorrowAssetAmount,
        fees: BorrowAssetAmount,
    ) {
        self.market
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
        _proof: InterestAccumulationProof,
        amount: BorrowAssetAmount,
        fees: BorrowAssetAmount,
    ) {
        // This should never panic, because a given amount of in-flight borrow
        // asset should always be added before it is removed.
        self.market
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
        proof: InterestAccumulationProof,
        amount: BorrowAssetAmount,
        fees: BorrowAssetAmount,
    ) {
        self.position.borrow_asset_fees.add_once(fees);
        self.position
            .increase_borrow_asset_principal(proof, amount, env::block_timestamp_ms())
            .unwrap_or_else(|| env::panic_str("Increase borrow asset principal overflow"));

        self.market
            .borrow_asset_borrowed
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset borrowed overflow"));
        self.market.snapshot();

        MarketEvent::BorrowWithdrawn {
            account_id: self.account_id.clone(),
            borrow_asset_amount: amount,
        }
        .emit();
    }

    /// Returns the amount that is left over after repaying the whole
    /// position. That is, the return value is the number of tokens that may
    /// be returned to the owner of the borrow position.
    pub fn record_repay(
        &mut self,
        proof: InterestAccumulationProof,
        amount: BorrowAssetAmount,
    ) -> BorrowAssetAmount {
        let liability_reduction = self
            .position
            .reduce_borrow_asset_liability(proof, amount)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));

        self.market
            .record_borrow_asset_yield_distribution(liability_reduction.amount_to_fees);

        // SAFETY: It should be impossible to panic here, since assets that
        // have not yet been borrowed cannot be repaid.
        self.market
            .borrow_asset_borrowed
            .split(liability_reduction.amount_to_principal)
            .unwrap_or_else(|| env::panic_str("Borrow asset borrowed underflow"));

        self.market.snapshot();

        MarketEvent::BorrowRepaid {
            account_id: self.account_id.clone(),
            borrow_asset_fees_repaid: liability_reduction.amount_to_fees,
            borrow_asset_principal_repaid: liability_reduction.amount_to_principal,
            borrow_asset_principal_remaining: self.position.get_borrow_asset_principal(),
        }
        .emit();

        liability_reduction.amount_remaining
    }

    pub fn accumulate_interest_partial(&mut self, snapshot_limit: u32) {
        self.market.snapshot();

        let accumulation_record = self.calculate_interest(snapshot_limit);

        if !accumulation_record.amount.is_zero() {
            MarketEvent::InterestAccumulated {
                account_id: self.account_id.clone(),
                borrow_asset_amount: accumulation_record.amount,
            }
            .emit();
        }

        self.position
            .borrow_asset_fees
            .accumulate(accumulation_record);
    }

    pub fn accumulate_interest(&mut self) -> InterestAccumulationProof {
        self.accumulate_interest_partial(u32::MAX);
        InterestAccumulationProof(())
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

        let snapshot_index = self.market.snapshot();
        self.position.full_liquidation(snapshot_index);

        self.market.borrow_asset_borrowed.split(principal);

        if recovered_amount.split(principal).is_some() {
            // Distribute yield.
            // record_borrow_asset_yield_distribution will take snapshot, no need to do it.
            self.market
                .record_borrow_asset_yield_distribution(recovered_amount);
        } else {
            // Took a loss on liquidation.
            // This can be detected from the event (borrow_asset_principal > borrow_asset_recovered?).
            // Deficit should be covered by protocol insurance.
            // No need for additional action.
        }
    }
}
