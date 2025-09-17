use std::ops::{Deref, DerefMut};

use near_sdk::{env, json_types::U64, near, AccountId};

use crate::{
    accumulator::{AccumulationRecord, Accumulator},
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount},
    asset_op,
    event::MarketEvent,
    market::Market,
    number::Decimal,
    price::{Appraise, Convert, PricePair, Valuation},
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
    /// The position is in good standing.
    Healthy,
    /// Collateralization ratio is below
    /// [`market::MarketConfiguration::borrow_mcr_maintenance`]. More
    /// collateral should be deposited or repayment should occur.
    MaintenanceRequired,
    /// The position can be liquidated.
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
    pub in_flight: BorrowAssetAmount,
    pub liquidation_lock: CollateralAssetAmount,
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
            in_flight: 0.into(),
            liquidation_lock: 0.into(),
        }
    }

    pub fn get_borrow_asset_principal(&self) -> BorrowAssetAmount {
        self.borrow_asset_principal
    }

    pub fn get_total_borrow_asset_liability(&self) -> BorrowAssetAmount {
        let mut total = BorrowAssetAmount::zero();
        asset_op! {
            total += self.borrow_asset_principal;
            total += self.borrow_asset_fees.get_total();
            total += self.in_flight;
        };
        total
    }

    pub fn get_total_collateral_amount(&self) -> CollateralAssetAmount {
        let mut total = CollateralAssetAmount::zero();
        asset_op! {
            total += self.collateral_asset_deposit;
            total += self.liquidation_lock;
        };
        total
    }

    pub fn can_be_removed(&self) -> bool {
        self.collateral_asset_deposit.is_zero()
            && self.get_total_borrow_asset_liability().is_zero()
            && self.in_flight.is_zero()
            && self.liquidation_lock.is_zero()
    }

    pub fn exists(&self) -> bool {
        !self.collateral_asset_deposit.is_zero()
            || !self.get_total_borrow_asset_liability().is_zero()
    }

    /// Returns `None` if liability is zero.
    pub fn collateralization_ratio(&self, price_pair: &PricePair) -> Option<Decimal> {
        let borrow_liability = self.get_total_borrow_asset_liability();
        if borrow_liability.is_zero() {
            return None;
        }

        let collateral_valuation =
            Valuation::pessimistic(self.get_total_collateral_amount(), &price_pair.collateral);
        let borrow_valuation = Valuation::optimistic(borrow_liability, &price_pair.borrow);

        collateral_valuation.ratio(borrow_valuation)
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

    pub fn liquidatable_collateral(
        &self,
        price_pair: &PricePair,
        mcr: Decimal,
        liquidator_spread: Decimal,
    ) -> CollateralAssetAmount {
        // c = Value of collateral
        // b = Value of borrow (liability)
        // l = Value of amount of collateral to liquidate
        // m = Target MCR
        // d = Liquidation discount (spread)
        // *_p = Price of *
        // *_a = Amount of *
        //
        // We should be just at the target MCR after liquidating the
        // maximum amount of collateral:
        // (c - l) / (b - l) = m
        // l = (m * b - c) / (m - 1)
        //
        // l = l_a * c_p
        // c = c_a * c_p * (1 - d)
        // b = b_a * b_p
        //
        // l_a * c_p = (m * b_a * b_p - c_a * c_p * (1 - d)) / (m - 1)
        // l_a = m * (b_a * b_p / c_p - c_a * (1 - d) / m) / (m - 1)

        let collateral_amount = Decimal::from(self.collateral_asset_deposit);
        let liability_valuation = price_pair.valuation(self.get_total_borrow_asset_liability());

        #[allow(clippy::unwrap_used, reason = "not div0")]
        let scaled_liability = liability_valuation
            .ratio(price_pair.valuation(CollateralAssetAmount::new(1)))
            .unwrap();
        let scaled_collateral_amount = collateral_amount * (Decimal::ONE - liquidator_spread) / mcr;

        if scaled_liability <= scaled_collateral_amount {
            CollateralAssetAmount::zero()
        } else {
            // Multiplication by mcr here could cause overflow
            let unscaled_amount =
                (scaled_liability - scaled_collateral_amount) / (mcr - Decimal::ONE);
            if unscaled_amount >= collateral_amount / mcr {
                self.collateral_asset_deposit
            } else {
                let amount = mcr * unscaled_amount;
                amount
                    .to_u128_ceil()
                    .map_or(self.collateral_asset_deposit, CollateralAssetAmount::new)
            }
        }
    }
}

#[must_use]
#[derive(Debug, Clone)]
pub struct LiabilityReduction {
    pub to_fees: BorrowAssetAmount,
    pub to_principal: BorrowAssetAmount,
    pub remaining: BorrowAssetAmount,
}

#[must_use]
#[derive(Debug, Clone)]
#[near(serializers = [json, borsh])]
pub struct InitialLiquidation {
    pub liquidated: CollateralAssetAmount,
    pub recovered: BorrowAssetAmount,
    pub refund: BorrowAssetAmount,
}

pub mod error {
    use thiserror::Error;

    use crate::asset::{BorrowAssetAmount, CollateralAssetAmount};

    #[derive(Error, Debug)]
    #[error("This position is currently being liquidated.")]
    pub struct LiquidationLockError;

    #[derive(Error, Debug)]
    pub enum InitialLiquidationError {
        #[error("Borrow position is not eligible for liquidation")]
        Ineligible,
        #[error("Attempt to liquidate more collateral than is currently eligible: {requested} requested > {available} available")]
        ExcessiveLiquidation {
            requested: CollateralAssetAmount,
            available: CollateralAssetAmount,
        },
        #[error("Failed to calculate value of collateral")]
        ValueCalculationFailure,
        #[error("Liquidation offer too low: {offered} offered < {minimum_acceptable} minimum acceptable")]
        OfferTooLow {
            offered: BorrowAssetAmount,
            minimum_acceptable: BorrowAssetAmount,
        },
    }
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
    pub fn estimate_current_snapshot_interest(&self) -> BorrowAssetAmount {
        let prev_end_timestamp_ms = self.market.get_last_finalized_snapshot().end_timestamp_ms.0;
        let interest_in_current_snapshot = self.market.interest_rate()
            * (env::block_timestamp_ms().saturating_sub(prev_end_timestamp_ms))
            * Decimal::from(self.position.get_borrow_asset_principal())
            / *MS_IN_A_YEAR;
        #[allow(clippy::unwrap_used, reason = "Interest rate guaranteed <= APY_LIMIT")]
        interest_in_current_snapshot.to_u128_ceil().unwrap().into()
    }

    pub fn with_pending_interest(&mut self) {
        let mut pending_estimate = self.calculate_interest(u32::MAX).get_amount();
        asset_op!(pending_estimate += self.estimate_current_snapshot_interest());

        self.position.borrow_asset_fees.pending_estimate = pending_estimate;
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
            accumulated += principal * snapshot.interest_rate * duration_ms / *MS_IN_A_YEAR;

            prev_end_timestamp_ms = snapshot.end_timestamp_ms.0;
            next_snapshot_index = i as u32 + 1;
        }

        AccumulationRecord {
            #[allow(
                clippy::unwrap_used,
                reason = "Assume accumulated interest will never exceed u128::MAX"
            )]
            amount: accumulated.to_u128_floor().unwrap().into(),
            fraction_as_u128_dividend: accumulated.fractional_part_as_u128_dividend(),
            next_snapshot_index,
        }
    }

    pub fn status(&self, price_pair: &PricePair, block_timestamp_ms: u64) -> BorrowStatus {
        let collateralization_ratio = self.position.collateralization_ratio(price_pair);
        self.market.configuration.borrow_status(
            collateralization_ratio,
            self.position.started_at_block_timestamp_ms,
            block_timestamp_ms,
        )
    }

    pub fn liquidatable_collateral(&self, price_pair: &PricePair) -> CollateralAssetAmount {
        self.position.liquidatable_collateral(
            price_pair,
            self.market.configuration.borrow_mcr_maintenance,
            self.market.configuration.liquidation_maximum_spread,
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

    pub(crate) fn reduce_borrow_asset_liability(
        &mut self,
        _proof: InterestAccumulationProof,
        mut amount: BorrowAssetAmount,
    ) -> LiabilityReduction {
        // No bounds checks necessary here: the min() call prevents underflow.
        let to_fees = self.position.borrow_asset_fees.get_total().min(amount);
        asset_op! {
            @msg("Invariant violation: min() precludes underflow")
            amount -= to_fees;
        };
        self.position.borrow_asset_fees.remove(to_fees);

        let to_principal = {
            let minimum_amount = u128::from(self.market.configuration.borrow_range.minimum);
            let amount_remaining =
                u128::from(self.position.borrow_asset_principal).saturating_sub(u128::from(amount));
            if amount_remaining > 0 && amount_remaining < minimum_amount {
                u128::from(self.position.borrow_asset_principal)
                    .saturating_sub(minimum_amount)
                    .into()
            } else {
                self.position.borrow_asset_principal.min(amount)
            }
        };
        asset_op! {
            @msg("Invariant violation: amount_to_principal > amount")
            amount -= to_principal;
            @msg("Invariant violation: amount_to_principal > borrow_asset_principal")
            self.position.borrow_asset_principal -= to_principal;
            @msg("Invariant violation: amount_to_principal > market.borrow_asset_borrowed")
            self.market.borrow_asset_borrowed -= to_principal;
        };

        if self.position.borrow_asset_principal.is_zero() {
            // fully paid off
            self.position.started_at_block_timestamp_ms = None;
        }

        self.market.record_borrow_asset_yield_distribution(to_fees);

        LiabilityReduction {
            to_fees,
            to_principal,
            remaining: amount,
        }
    }

    pub fn record_collateral_asset_deposit(
        &mut self,
        _proof: InterestAccumulationProof,
        amount: CollateralAssetAmount,
    ) {
        asset_op! {
            self.position.collateral_asset_deposit += amount;
            self.market.collateral_asset_deposited += amount;
        };

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
        asset_op! {
            self.position.collateral_asset_deposit -= amount;
            self.market.collateral_asset_deposited -= amount;
        };

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
        asset_op! {
            self.market.borrow_asset_in_flight += amount;
            self.position.in_flight += amount;
            self.position.in_flight += fees;
        };
    }

    pub fn record_borrow_asset_in_flight_end(
        &mut self,
        _proof: InterestAccumulationProof,
        amount: BorrowAssetAmount,
        fees: BorrowAssetAmount,
    ) {
        // This should never panic, because a given amount of in-flight borrow
        // asset should always be added before it is removed.
        asset_op! {
            self.market.borrow_asset_in_flight -= amount;
            self.position.in_flight -= amount;
            self.position.in_flight -= fees;
        };
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

        asset_op!(self.market.borrow_asset_borrowed += amount);

        MarketEvent::BorrowWithdrawn {
            account_id: self.account_id.clone(),
            borrow_asset_amount: amount,
        }
        .emit();
    }

    /// Returns the amount that is left over after repaying the whole
    /// position. That is, the return value is the number of tokens that may
    /// be returned to the owner of the borrow position.
    ///
    /// # Errors
    ///
    /// - If any collateral is locked for liquidation.
    pub fn record_repay(
        &mut self,
        proof: InterestAccumulationProof,
        amount: BorrowAssetAmount,
    ) -> Result<BorrowAssetAmount, error::LiquidationLockError> {
        if !self.position.liquidation_lock.is_zero() {
            return Err(error::LiquidationLockError);
        }

        let current_snapshot_interest = self.estimate_current_snapshot_interest();
        // Amortize current snapshot fees so that when the current snapshot is
        // finalized, the fees are not doubled.
        self.position
            .borrow_asset_fees
            .amortize(current_snapshot_interest);

        let liability_reduction = self.reduce_borrow_asset_liability(proof, amount);

        MarketEvent::BorrowRepaid {
            account_id: self.account_id.clone(),
            borrow_asset_fees_repaid: liability_reduction.to_fees,
            borrow_asset_principal_repaid: liability_reduction.to_principal,
            borrow_asset_principal_remaining: self.position.get_borrow_asset_principal(),
        }
        .emit();

        Ok(liability_reduction.remaining)
    }

    pub fn accumulate_interest_partial(&mut self, snapshot_limit: u32) {
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

    pub fn liquidation_lock(&mut self, amount: CollateralAssetAmount) {
        asset_op!(
            @msg("Attempt to liquidate more collateral than position has deposited")
            self.position.collateral_asset_deposit -= amount;
            self.position.liquidation_lock += amount;
        );
    }

    pub fn liquidation_unlock(&mut self, amount: CollateralAssetAmount) {
        asset_op!(
            @msg("Invariant violation: Liquidation unlock of more collateral that was locked")
            self.position.liquidation_lock -= amount;
            self.position.collateral_asset_deposit += amount;
        );
    }

    /// # Errors
    ///
    /// - If this record is not eligible for liquidation.
    /// - If the liquidator requests to liquidate too much collateral from the
    ///     position.
    /// - If the calculation of the collateral value fails.
    /// - If the liquidator offers too little to purchase the collateral.
    pub fn record_liquidation_initial(
        &mut self,
        _proof: InterestAccumulationProof,
        liquidator_sent: BorrowAssetAmount,
        liquidator_request: Option<CollateralAssetAmount>,
        price_pair: &PricePair,
        block_timestamp_ms: u64,
    ) -> Result<InitialLiquidation, error::InitialLiquidationError> {
        let BorrowStatus::Liquidation(reason) = self.status(price_pair, block_timestamp_ms) else {
            return Err(error::InitialLiquidationError::Ineligible);
        };

        let liquidatable_collateral = match reason {
            LiquidationReason::Undercollateralization => self.liquidatable_collateral(price_pair),
            LiquidationReason::Expiration => self.position.collateral_asset_deposit,
        };

        // If liquidator doesn't specify an amount of collateral to liquidate,
        // attempt to liquidate all of the collateral that can be liquidated
        // from the position.
        let liquidator_request = liquidator_request.unwrap_or(liquidatable_collateral);

        if liquidator_request > liquidatable_collateral {
            return Err(error::InitialLiquidationError::ExcessiveLiquidation {
                requested: liquidator_request,
                available: liquidatable_collateral,
            });
        }

        let collateral_value = price_pair.convert(liquidator_request);

        let maximum_acceptable: BorrowAssetAmount = collateral_value
            .to_u128_ceil()
            .ok_or(error::InitialLiquidationError::ValueCalculationFailure)?
            .max(1)
            .into();
        #[allow(
            clippy::unwrap_used,
            reason = "Previous line guarantees this will not panic"
        )]
        let minimum_acceptable: BorrowAssetAmount = (collateral_value
            * (Decimal::ONE - self.market.configuration.liquidation_maximum_spread))
            .to_u128_ceil()
            .unwrap()
            .max(1)
            .into();

        if liquidator_sent < minimum_acceptable {
            return Err(error::InitialLiquidationError::OfferTooLow {
                offered: liquidator_sent,
                minimum_acceptable,
            });
        }

        self.liquidation_lock(liquidator_request);

        let mut refund = BorrowAssetAmount::zero();
        let mut recovered = liquidator_sent;
        if liquidator_sent > maximum_acceptable {
            recovered = maximum_acceptable;
            refund = liquidator_sent;
            asset_op!(refund -= recovered);
        }

        Ok(InitialLiquidation {
            liquidated: liquidator_request,
            recovered,
            refund,
        })
    }

    pub fn record_liquidation_final(
        &mut self,
        proof: InterestAccumulationProof,
        liquidator_id: AccountId,
        initial_liquidation: &InitialLiquidation,
    ) {
        let liability_reduction =
            self.reduce_borrow_asset_liability(proof, initial_liquidation.recovered);
        self.market
            .record_borrow_asset_yield_distribution(liability_reduction.remaining);
        self.liquidation_unlock(initial_liquidation.liquidated);
        self.record_collateral_asset_withdrawal(proof, initial_liquidation.liquidated);

        MarketEvent::Liquidation {
            liquidator_id,
            account_id: self.account_id.clone(),
            borrow_asset_recovered: initial_liquidation.recovered,
            collateral_asset_liquidated: initial_liquidation.liquidated,
        }
        .emit();
    }
}
