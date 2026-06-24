use std::ops::{Deref, DerefMut};

use near_sdk::{json_types::U64, near, AccountId};
use templar_primitives::number::Decimal;

use crate::{
    accumulator::{AccumulationRecord, Accumulator},
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount},
    event::MarketEvent,
    market::{Market, SnapshotProof},
    price::{Appraise, Convert, PricePair, Valuation},
    YEAR_PER_MS,
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
    /// [`crate::market::MarketConfiguration::borrow_mcr_maintenance`]. More
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
    pub borrow_asset_principal: BorrowAssetAmount,
    #[serde(alias = "borrow_asset_fees")]
    pub interest: Accumulator<BorrowAsset>,
    #[serde(default)]
    pub fees: BorrowAssetAmount,
    #[serde(default)]
    pub borrow_asset_in_flight: BorrowAssetAmount,
    #[serde(default)]
    pub collateral_asset_in_flight: CollateralAssetAmount,
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
            interest: Accumulator::new(current_snapshot_index),
            fees: 0.into(),
            borrow_asset_in_flight: 0.into(),
            collateral_asset_in_flight: 0.into(),
        }
    }

    pub fn get_borrow_asset_principal(&self) -> BorrowAssetAmount {
        self.borrow_asset_principal + self.borrow_asset_in_flight
    }

    pub fn get_total_borrow_asset_liability(&self) -> BorrowAssetAmount {
        self.borrow_asset_principal
            + self.borrow_asset_in_flight
            + self.interest.get_total()
            + self.fees
    }

    pub fn get_total_collateral_amount(&self) -> CollateralAssetAmount {
        self.collateral_asset_deposit
    }

    pub fn exists(&self) -> bool {
        !self.get_total_collateral_amount().is_zero()
            || !self.get_total_borrow_asset_liability().is_zero()
            || !self.collateral_asset_in_flight.is_zero()
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
    ) {
        if self.started_at_block_timestamp_ms.is_none()
            || self.get_total_borrow_asset_liability().is_zero()
        {
            self.started_at_block_timestamp_ms = Some(block_timestamp_ms.into());
        }
        self.borrow_asset_principal += amount;
    }

    pub fn liquidatable_collateral(
        &self,
        price_pair: &PricePair,
        mcr: Decimal,
        liquidator_spread: Decimal,
    ) -> CollateralAssetAmount {
        let liability = self.get_total_borrow_asset_liability();
        if liability.is_zero() {
            return CollateralAssetAmount::zero();
        }

        let valuation_liability = price_pair.valuation(liability);
        let collateral = self.get_total_collateral_amount();
        let valuation_collateral = price_pair.valuation(collateral);

        let Some(cr) = valuation_collateral.ratio(valuation_liability) else {
            // Zero-valued liability
            return CollateralAssetAmount::zero();
        };

        if cr <= Decimal::ONE {
            // Totally underwater
            return collateral;
        }

        if cr >= mcr {
            // Above MCR
            return CollateralAssetAmount::zero();
        }

        let collateral_dec = Decimal::from(collateral);
        let discount = Decimal::ONE - liquidator_spread;

        let liquidatable_amount = (mcr * price_pair.convert(liability) - collateral_dec)
            / (mcr * discount - Decimal::ONE);

        liquidatable_amount
            .to_u128_ceil()
            .map_or(collateral, CollateralAssetAmount::new)
            .min(collateral)
    }
}

#[must_use]
#[derive(Debug, Clone)]
#[near(serializers = [json])]
pub struct LiabilityReduction {
    pub to_fees: BorrowAssetAmount,
    pub to_interest: BorrowAssetAmount,
    pub to_principal: BorrowAssetAmount,
    pub to_refund: BorrowAssetAmount,
}

#[must_use]
#[derive(Debug, Clone)]
#[near(serializers = [json, borsh])]
pub struct Liquidation {
    pub liquidated: CollateralAssetAmount,
    pub refund: BorrowAssetAmount,
}

#[must_use]
#[derive(Debug, Clone)]
#[near(serializers = [json, borsh])]
pub struct InitialBorrow {
    pub amount: BorrowAssetAmount,
    pub fees: BorrowAssetAmount,
}

pub mod error {
    use thiserror::Error;

    use crate::asset::{BorrowAssetAmount, CollateralAssetAmount};

    #[derive(Error, Debug)]
    pub enum LiquidationError {
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

    #[derive(Debug, Error)]
    pub enum InitialBorrowError {
        #[error("Insufficient borrow asset available")]
        InsufficientBorrowAssetAvailable,
        #[error("Fee calculation failed")]
        FeeCalculationFailure,
        #[error("Borrow position must be healthy after borrow")]
        Undercollateralization,
        #[error("New borrow position is outside of allowable range")]
        OutsideAllowableRange,
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
    pub fn calculate_interest(&self, snapshot_limit: u32) -> AccumulationRecord<BorrowAsset> {
        let principal: Decimal = self.position.get_borrow_asset_principal().into();
        let mut next_snapshot_index = self.position.interest.get_next_snapshot_index();

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
                        crate::panic_with_message(&format!(
                            "Invariant violation: Snapshot timestamp decrease at time chunk #{}.",
                            u64::from(snapshot.time_chunk.0),
                        ))
                    }),
            );
            accumulated += principal * snapshot.interest_rate * duration_ms * YEAR_PER_MS;

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

    pub fn within_allowable_borrow_range(&self) -> bool {
        self.market
            .configuration
            .borrow_range
            .contains(self.position.get_borrow_asset_principal())
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
        let to_fees = self.position.fees.min(amount);
        amount = amount.unwrap_sub(to_fees, "Invariant violation: min() precludes underflow");
        self.position.fees = self
            .position
            .fees
            .unwrap_sub(to_fees, "Invariant violation: min() precludes underflow");

        let to_interest = self.position.interest.get_total().min(amount);
        amount = amount.unwrap_sub(
            to_interest,
            "Invariant violation: min() precludes underflow",
        );
        self.position.interest.remove(to_interest);

        self.market.borrow_asset_paid_to_fees += to_fees + to_interest;

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
        amount = amount.unwrap_sub(
            to_principal,
            "Invariant violation: amount_to_principal > amount",
        );
        self.position.borrow_asset_principal = self.position.borrow_asset_principal.unwrap_sub(
            to_principal,
            "Invariant violation: amount_to_principal > borrow_asset_principal",
        );
        self.market.borrow_asset_borrowed = self.market.borrow_asset_borrowed.unwrap_sub(
            to_principal,
            "Invariant violation: amount_to_principal > market.borrow_asset_borrowed",
        );

        if self.position.borrow_asset_principal.is_zero() {
            // fully paid off
            self.position.started_at_block_timestamp_ms = None;
        }

        LiabilityReduction {
            to_fees,
            to_interest,
            to_principal,
            to_refund: amount,
        }
    }

    pub fn record_collateral_asset_deposit(
        &mut self,
        _proof: InterestAccumulationProof,
        amount: CollateralAssetAmount,
    ) {
        self.position.collateral_asset_deposit += amount;
        self.market.collateral_asset_deposited += amount;

        MarketEvent::CollateralDeposited {
            account_id: self.account_id.clone(),
            collateral_asset_amount: amount,
        }
        .emit();
    }

    pub fn record_collateral_asset_withdrawal_initial(
        &mut self,
        _proof: InterestAccumulationProof,
        amount: CollateralAssetAmount,
    ) {
        self.position.collateral_asset_in_flight += amount;
        self.position.collateral_asset_deposit -= amount;
        self.market.collateral_asset_deposited -= amount;
    }

    pub fn record_collateral_asset_withdrawal_final(
        &mut self,
        _proof: InterestAccumulationProof,
        amount: CollateralAssetAmount,
        success: bool,
    ) {
        self.position.collateral_asset_in_flight =
            self.position.collateral_asset_in_flight.unwrap_sub(
                amount,
                "Invariant violation: attempt to unlock more than locked as in-flight",
            );

        if success {
            MarketEvent::CollateralWithdrawn {
                account_id: self.account_id.clone(),
                collateral_asset_amount: amount,
            }
            .emit();
        } else {
            self.position.collateral_asset_deposit += amount;
            self.market.collateral_asset_deposited += amount;
        }
    }

    pub(crate) fn record_collateral_asset_withdrawal(
        &mut self,
        _proof: InterestAccumulationProof,
        amount: CollateralAssetAmount,
    ) {
        self.position.collateral_asset_deposit -= amount;
        self.market.collateral_asset_deposited -= amount;
    }

    /// # Errors
    ///
    /// - If there is not enough borrow asset available to borrow.
    /// - If there is an error calculating the fee (e.g. overflow).
    pub fn record_borrow_initial(
        &mut self,
        _proof: SnapshotProof,
        _interest: InterestAccumulationProof,
        amount: BorrowAssetAmount,
        price_pair: &PricePair,
        block_timestamp_ms: u64,
    ) -> Result<InitialBorrow, error::InitialBorrowError> {
        // Ensure we have enough funds to dispense.
        let available_to_borrow = self.market.get_borrow_asset_available_to_borrow();
        if amount > available_to_borrow {
            return Err(error::InitialBorrowError::InsufficientBorrowAssetAvailable);
        }

        let origination_fee = self
            .market
            .configuration
            .borrow_origination_fee
            .of(amount)
            .ok_or(error::InitialBorrowError::FeeCalculationFailure)?;

        // Necessary because we track borrows in terms of whole snapshots, so
        // this covers the interest that could be missed because of ignoring
        // fractional snapshots.
        let single_snapshot_fee = self
            .market
            .single_snapshot_fee(amount)
            .ok_or(error::InitialBorrowError::FeeCalculationFailure)?;

        let mut fees = origination_fee;
        fees = fees
            .checked_add(single_snapshot_fee)
            .ok_or(error::InitialBorrowError::FeeCalculationFailure)?;

        self.market.borrow_asset_borrowed_in_flight += amount;
        self.position.borrow_asset_in_flight += amount;
        self.position.fees += fees;

        if !self.status(price_pair, block_timestamp_ms).is_healthy() {
            self.market.borrow_asset_borrowed_in_flight -= amount;
            self.position.borrow_asset_in_flight -= amount;
            self.position.fees -= fees;
            return Err(error::InitialBorrowError::Undercollateralization);
        }

        if !self.within_allowable_borrow_range() {
            self.market.borrow_asset_borrowed_in_flight -= amount;
            self.position.borrow_asset_in_flight -= amount;
            self.position.fees -= fees;
            return Err(error::InitialBorrowError::OutsideAllowableRange);
        }

        self.market.record_borrow_asset_yield_distribution(fees);
        self.market.borrow_asset_balance -= amount;

        Ok(InitialBorrow { amount, fees })
    }

    pub fn record_borrow_final(
        &mut self,
        _snapshot: SnapshotProof,
        interest: InterestAccumulationProof,
        borrow: &InitialBorrow,
        success: bool,
        block_timestamp_ms: u64,
    ) {
        // This should never panic, because a given amount of in-flight borrow
        // asset should always be added before it is removed.
        self.market.borrow_asset_borrowed_in_flight -= borrow.amount;
        self.position.borrow_asset_in_flight -= borrow.amount;

        if success {
            // GREAT SUCCESS
            //
            // Borrow position has already been created: finalize
            // withdrawal record.
            self.position.increase_borrow_asset_principal(
                interest,
                borrow.amount,
                block_timestamp_ms,
            );

            self.market.borrow_asset_borrowed += borrow.amount;

            MarketEvent::BorrowWithdrawn {
                account_id: self.account_id.clone(),
                borrow_asset_amount: borrow.amount,
            }
            .emit();
        } else {
            // Likely reasons for failure:
            //
            // 1. Price oracle is out-of-date. This is kind of bad, but
            //  not necessarily catastrophic nor unrecoverable. Probably,
            //  the oracle is just lagging and will be fine if the user
            //  tries again later.
            //
            // Mitigation strategy: Revert locks & state changes (i.e. do
            // nothing else).
            //
            // 2. MPC signing failed or took too long. Need to do a bit
            //  more research to see if it is possible for the signature to
            //  still show up on chain after the promise expires.
            //
            // Mitigation strategy: Retain locks until we know the
            // signature will not be issued. Note that we can't implement
            // this strategy until we implement asset transfer for MPC
            // assets, so we IGNORE THIS CASE FOR NOW.
            //
            // TODO: Implement case 2 mitigation.
            // NOTE: Not needed for chain-local (NEP-141-only) tokens.

            self.market.borrow_asset_balance += borrow.amount;
        }
    }

    /// Returns the amount that is left over after repaying the whole
    /// position. That is, the return value is the number of tokens that may
    /// be returned to the owner of the borrow position.
    pub fn record_repay(
        &mut self,
        proof: InterestAccumulationProof,
        amount: BorrowAssetAmount,
    ) -> BorrowAssetAmount {
        self.market.borrow_asset_balance += amount;
        let liability_reduction = self.reduce_borrow_asset_liability(proof, amount);

        MarketEvent::BorrowRepaid {
            account_id: self.account_id.clone(),
            amount_sent: amount,
            liability_reduction: liability_reduction.clone(),
            liability_remaining: self.position.get_total_borrow_asset_liability(),
        }
        .emit();

        self.market.borrow_asset_balance -= liability_reduction.to_refund;

        liability_reduction.to_refund
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

        self.position.interest.accumulate(accumulation_record);
    }

    pub fn accumulate_interest(&mut self) -> InterestAccumulationProof {
        self.accumulate_interest_partial(u32::MAX);
        InterestAccumulationProof(())
    }

    /// # Errors
    ///
    /// - If this record is not eligible for liquidation.
    /// - If the liquidator requests to liquidate too much collateral from the
    ///   position.
    /// - If the calculation of the collateral value fails.
    /// - If the liquidator offers too little to purchase the collateral.
    pub fn record_liquidation(
        &mut self,
        proof: InterestAccumulationProof,
        liquidator_id: AccountId,
        liquidator_sent: BorrowAssetAmount,
        liquidator_request: Option<CollateralAssetAmount>,
        price_pair: &PricePair,
        block_timestamp_ms: u64,
    ) -> Result<Liquidation, error::LiquidationError> {
        let BorrowStatus::Liquidation(reason) = self.status(price_pair, block_timestamp_ms) else {
            return Err(error::LiquidationError::Ineligible);
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
            return Err(error::LiquidationError::ExcessiveLiquidation {
                requested: liquidator_request,
                available: liquidatable_collateral,
            });
        }

        let collateral_value = price_pair.convert(liquidator_request);

        let maximum_acceptable: BorrowAssetAmount = collateral_value
            .to_u128_ceil()
            .ok_or(error::LiquidationError::ValueCalculationFailure)?
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
            return Err(error::LiquidationError::OfferTooLow {
                offered: liquidator_sent,
                minimum_acceptable,
            });
        }

        let (refund, recovered) = if liquidator_sent > maximum_acceptable {
            (liquidator_sent - maximum_acceptable, maximum_acceptable)
        } else {
            (BorrowAssetAmount::zero(), liquidator_sent)
        };

        self.record_collateral_asset_withdrawal(proof, liquidator_request);

        let liability_reduction = self.reduce_borrow_asset_liability(proof, recovered);
        self.market
            .record_borrow_asset_yield_distribution(liability_reduction.to_refund);

        self.market.borrow_asset_balance += recovered;

        MarketEvent::Liquidation {
            liquidator_id,
            account_id: self.account_id.clone(),
            borrow_asset_recovered: recovered,
            collateral_asset_liquidated: liquidator_request,
        }
        .emit();

        Ok(Liquidation {
            liquidated: liquidator_request,
            refund,
        })
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::{env, serde_json, test_utils::VMContextBuilder, testing_env};
    use rstest::rstest;

    use crate::{
        asset::FungibleAsset,
        fee::{Fee, TimeBasedFee},
        interest_rate_strategy::InterestRateStrategy,
        market::{MarketConfiguration, PriceOracleConfiguration, YieldWeights},
        oracle::pyth::{self, PriceIdentifier},
        snapshot::Snapshot,
        time_chunk::{TimeChunk, TimeChunkConfiguration},
    };

    use super::*;

    /// Build a minimal valid market configuration. The interest-rate strategy
    /// is irrelevant to [`calculate_interest`] because the test overwrites the
    /// `interest_rate` of each finalized snapshot directly.
    fn interest_test_configuration() -> MarketConfiguration {
        use templar_primitives::dec;

        MarketConfiguration {
            time_chunk_configuration: TimeChunkConfiguration::new(600_000),
            borrow_asset: FungibleAsset::nep141("borrow.near".parse().unwrap()),
            collateral_asset: FungibleAsset::nep141("collateral.near".parse().unwrap()),
            price_oracle_configuration: PriceOracleConfiguration {
                account_id: "pyth-oracle.near".parse().unwrap(),
                collateral_asset_price_id: PriceIdentifier([0xcc; 32]),
                collateral_asset_decimals: 24,
                borrow_asset_price_id: PriceIdentifier([0xbb; 32]),
                borrow_asset_decimals: 24,
                price_maximum_age_s: 60,
            },
            borrow_mcr_maintenance: dec!("1.25"),
            borrow_mcr_liquidation: dec!("1.2"),
            borrow_asset_maximum_usage_ratio: dec!("0.99"),
            borrow_origination_fee: Fee::zero(),
            borrow_interest_rate_strategy: InterestRateStrategy::zero(),
            borrow_maximum_duration_ms: None,
            borrow_range: (1, None).try_into().unwrap(),
            supply_range: (1, None).try_into().unwrap(),
            supply_withdrawal_range: (1, None).try_into().unwrap(),
            supply_withdrawal_fee: TimeBasedFee::zero(),
            yield_weights: YieldWeights::new_with_supply_weight(9)
                .with_static("revenue.tmplr.near".parse().unwrap(), 1),
            protocol_account_id: "revenue.tmplr.near".parse().unwrap(),
            liquidation_maximum_spread: dec!("0.05"),
        }
    }

    /// Push a finalized snapshot with an explicitly chosen end timestamp and
    /// interest rate. Only `end_timestamp_ms`, `interest_rate`, and
    /// `time_chunk` (for panic messages) feed into [`calculate_interest`]; the
    /// remaining balance fields are irrelevant and left at zero.
    fn push_finalized_snapshot(market: &mut Market, end_timestamp_ms: u64, interest_rate: Decimal) {
        market.finalized_snapshots.push(Snapshot {
            time_chunk: TimeChunk(end_timestamp_ms.into()),
            end_timestamp_ms: end_timestamp_ms.into(),
            borrow_asset_deposited_active: 0.into(),
            borrow_asset_borrowed: 0.into(),
            collateral_asset_deposited: 0.into(),
            yield_distribution: BorrowAssetAmount::zero(),
            interest_rate,
        });
    }

    /// Deterministic, node-free unit test of the interest-accrual math in
    /// [`BorrowPositionRef::calculate_interest`].
    ///
    /// The market is constructed in the in-process near-sdk test VM (no neard
    /// sandbox). `Market::new` pushes finalized snapshot #0, whose
    /// `end_timestamp_ms` is the construction block timestamp (`T0`). We then
    /// push two more finalized snapshots at chosen timestamps with chosen
    /// interest rates, and build a borrow position whose interest accumulator
    /// begins at snapshot index 1 (so accrual spans snapshots #1 and #2).
    ///
    /// For each finalized snapshot the function accrues:
    ///     principal * interest_rate * duration_ms * YEAR_PER_MS
    /// where `duration_ms` is the gap to the previous snapshot's end timestamp,
    /// then returns `floor(sum)`.
    ///
    /// Chosen inputs:
    ///   principal P  = 1_000_000_000_000          (1e12)
    ///   T0 (snap #0) = 1_000_000 ms
    ///   snap #1: end = T0 + 2_592_000_000 ms (30 days), rate = 0.10
    ///   snap #2: end = +  5_184_000_000 ms (60 days), rate = 0.20
    ///
    /// Approx accrual (using YEAR_PER_MS ~= 3.168873850681e-11):
    ///   snap #1: 1e12 * 0.10 * 2.592e9 * 3.168873e-11 ~= 8.21372e9
    ///   snap #2: 1e12 * 0.20 * 5.184e9 * 3.168873e-11 ~= 3.28549e10
    ///   total  ~= 4.10686e10
    /// The exact floored integer is pinned to `EXPECTED` below.
    #[test]
    fn calculate_interest_two_snapshots_exact() {
        const T0_MS: u64 = 1_000_000;
        const DURATION_1_MS: u64 = 2_592_000_000; // 30 days
        const DURATION_2_MS: u64 = 5_184_000_000; // 60 days
        const T1_MS: u64 = T0_MS + DURATION_1_MS;
        const T2_MS: u64 = T1_MS + DURATION_2_MS;
        const PRINCIPAL: u128 = 1_000_000_000_000;

        let rate_1 = templar_primitives::dec!("0.10");
        let rate_2 = templar_primitives::dec!("0.20");

        // In-process near-sdk VM: set the construction-time block timestamp so
        // that finalized snapshot #0 lands at T0_MS (block timestamp is in ns).
        let context = VMContextBuilder::new()
            .block_timestamp(T0_MS * 1_000_000)
            .build();
        testing_env!(context);

        let mut market = Market::new(b"m", interest_test_configuration());
        // Sanity: Market::new pushed exactly one snapshot (#0) at T0_MS.
        assert_eq!(market.finalized_snapshots.len(), 1);
        assert_eq!(
            market.get_last_finalized_snapshot().end_timestamp_ms.0,
            T0_MS
        );

        push_finalized_snapshot(&mut market, T1_MS, rate_1); // index 1
        push_finalized_snapshot(&mut market, T2_MS, rate_2); // index 2
        assert_eq!(market.finalized_snapshots.len(), 3);

        // Borrow position with the chosen principal; its interest accumulator
        // starts at snapshot index 1, so accrual covers snapshots #1 and #2.
        let mut position = BorrowPosition::new(1);
        position.borrow_asset_principal = BorrowAssetAmount::new(PRINCIPAL);
        assert_eq!(position.interest.get_next_snapshot_index(), 1);

        let position_ref =
            BorrowPositionRef::new(&market, "borrower.near".parse().unwrap(), position);

        let record = position_ref.calculate_interest(u32::MAX);

        // Independently recompute the expected value with the exact same
        // fixed-point operations and accumulation order as the function under
        // test, so fixed-point rounding matches bit-for-bit.
        let principal = Decimal::from(PRINCIPAL);
        let mut expected_acc = Decimal::ZERO;
        expected_acc += principal * rate_1 * Decimal::from(DURATION_1_MS) * YEAR_PER_MS;
        expected_acc += principal * rate_2 * Decimal::from(DURATION_2_MS) * YEAR_PER_MS;
        let expected = expected_acc.to_u128_floor().unwrap();

        // Pin the exact integer so this is not merely a re-run of the impl.
        const EXPECTED: u128 = 41_068_605_104;
        assert_eq!(
            expected, EXPECTED,
            "hand-derived Decimal computation drifted from the pinned literal",
        );

        assert_eq!(
            u128::from(record.amount),
            EXPECTED,
            "calculate_interest accrued the wrong amount",
        );
        // The accumulator advances to one-past the last consumed snapshot.
        assert_eq!(record.next_snapshot_index, 3);
    }

    #[rstest]
    #[test]
    fn liquidatable_collateral(
        #[values("1.2", "1.25", "1.5", "2")] mcr: Decimal,
        #[values(11, 1000, 1005, 999_999)] collateral_price: i64,
        #[values(1000, 1005, 999_999)] borrow_price: i64,
        #[values(0, 10)] conf: u64,
    ) {
        use templar_primitives::dec;

        use crate::oracle::pyth::PythTimestamp;

        let c = VMContextBuilder::new()
            .block_timestamp(1_000_000_000_000_000)
            .build();
        testing_env!(c.clone());

        let configuration = MarketConfiguration {
            time_chunk_configuration: TimeChunkConfiguration::new(600_000),
            borrow_asset: FungibleAsset::nep141("borrow.near".parse().unwrap()),
            collateral_asset: FungibleAsset::nep141("collateral.near".parse().unwrap()),
            price_oracle_configuration: PriceOracleConfiguration {
                account_id: "pyth-oracle.near".parse().unwrap(),
                collateral_asset_price_id: PriceIdentifier([0xcc; 32]),
                collateral_asset_decimals: 24,
                borrow_asset_price_id: PriceIdentifier([0xbb; 32]),
                borrow_asset_decimals: 24,
                price_maximum_age_s: 60,
            },
            borrow_mcr_maintenance: mcr,
            borrow_mcr_liquidation: mcr,
            borrow_asset_maximum_usage_ratio: dec!("0.99"),
            borrow_origination_fee: Fee::zero(),
            borrow_interest_rate_strategy: InterestRateStrategy::zero(),
            borrow_maximum_duration_ms: None,
            borrow_range: (1, None).try_into().unwrap(),
            supply_range: (1, None).try_into().unwrap(),
            supply_withdrawal_range: (1, None).try_into().unwrap(),
            supply_withdrawal_fee: TimeBasedFee::zero(),
            yield_weights: YieldWeights::new_with_supply_weight(9)
                .with_static("revenue.tmplr.near".parse().unwrap(), 1),
            protocol_account_id: "revenue.tmplr.near".parse().unwrap(),
            liquidation_maximum_spread: dec!("0.05"),
        };

        let mut market = Market::new(b"m", configuration.clone());
        market.borrow_asset_deposited_active += BorrowAssetAmount::new(100_000_000_000);
        market.borrow_asset_balance += BorrowAssetAmount::new(100_000_000_000);
        let snapshot_proof = market.snapshot();

        let mut position = BorrowPositionGuard(BorrowPositionRef {
            market: &mut market,
            account_id: "borrower".parse().unwrap(),
            position: BorrowPosition::new(1),
        });

        let interest_proof = position.accumulate_interest();
        position.record_collateral_asset_deposit(
            interest_proof,
            CollateralAssetAmount::new(100_000_000),
        );
        let initial_price_pair = PricePair::new(
            &pyth::Price {
                price: 5.into(),
                conf: 0.into(),
                expo: 24,
                publish_time: PythTimestamp::from_secs(10),
            },
            24,
            &pyth::Price {
                price: 1.into(),
                conf: 0.into(),
                expo: 24,
                publish_time: PythTimestamp::from_secs(10),
            },
            24,
        )
        .unwrap();
        assert_eq!(
            position.liquidatable_collateral(&initial_price_pair),
            CollateralAssetAmount::zero(),
        );
        let initial_borrow = position
            .record_borrow_initial(
                snapshot_proof,
                interest_proof,
                BorrowAssetAmount::new(85_000_000),
                &initial_price_pair,
                env::block_timestamp_ms(),
            )
            .unwrap();
        position.record_borrow_final(
            snapshot_proof,
            interest_proof,
            &initial_borrow,
            true,
            env::block_timestamp_ms(),
        );
        let price_pair = PricePair::new(
            &pyth::Price {
                price: collateral_price.into(),
                conf: conf.into(),
                expo: 24,
                publish_time: PythTimestamp::from_secs(10),
            },
            24,
            &pyth::Price {
                price: borrow_price.into(),
                conf: conf.into(),
                expo: 24,
                publish_time: PythTimestamp::from_secs(10),
            },
            24,
        )
        .unwrap();
        let starting_cr = position.inner().collateralization_ratio(&price_pair);
        eprintln!("Starting collateralization ratio: {starting_cr:?}");
        let liquidatable_collateral = position.liquidatable_collateral(&price_pair);

        let minimum_acceptable = configuration
            .minimum_acceptable_liquidation_amount(liquidatable_collateral, &price_pair)
            .unwrap();

        eprintln!("Liquidatable collateral: {liquidatable_collateral}");
        eprintln!("Minimum acceptable: {minimum_acceptable}");

        match collateral_price.ilog10().cmp(&borrow_price.ilog10()) {
            std::cmp::Ordering::Less => {
                // Completely underwater
                assert_eq!(
                    liquidatable_collateral,
                    CollateralAssetAmount::new(100_000_000),
                    "All collateral should be eligible for liquidation"
                );
            }
            std::cmp::Ordering::Equal => {
                // Partial liquidation

                let _liquidation = position
                    .record_liquidation(
                        interest_proof,
                        "liquidator".parse().unwrap(),
                        minimum_acceptable,
                        Some(liquidatable_collateral),
                        &price_pair,
                        env::block_timestamp_ms(),
                    )
                    .unwrap();

                let finishing_cr = position
                    .inner()
                    .collateralization_ratio(&price_pair)
                    .unwrap();
                eprintln!("Finishing collateralization ratio: {finishing_cr}");
                eprintln!("Target MCR: {mcr}");

                assert!(finishing_cr >= mcr);
                let delta = finishing_cr.abs_diff(mcr);
                assert!(delta < Decimal::ONE.mul_pow10(-4).unwrap());
            }
            std::cmp::Ordering::Greater => {
                // No liquidation

                assert_eq!(
                    liquidatable_collateral,
                    CollateralAssetAmount::zero(),
                    "No collateral should be liquidatable"
                );
            }
        }
    }

    #[test]
    fn test_borrow_position_deserialize_new_format() {
        // New market format with interest field
        let json = r#"{
            "started_at_block_timestamp_ms": "1699564800000",
            "collateral_asset_deposit": "1000000000000000000000000",
            "borrow_asset_principal": "100000000",
            "interest": {
                "total": "0",
                "fraction_as_u128_dividend": "0",
                "next_snapshot_index": 42,
                "pending_estimate": "0"
            },
            "fees": "500000",
            "borrow_asset_in_flight": "50000000",
            "collateral_asset_in_flight": "0",
            "liquidation_lock": "0"
        }"#;

        let position: BorrowPosition =
            serde_json::from_str(json).expect("Failed to deserialize new format");
        assert_eq!(position.fees, BorrowAssetAmount::new(500_000));
        assert_eq!(
            position.get_borrow_asset_principal(),
            BorrowAssetAmount::new(50_000_000 + 100_000_000)
        );
    }

    #[test]
    fn test_borrow_position_deserialize_old_format_with_borrow_asset_fees() {
        // Old market format with borrow_asset_fees instead of interest
        let json = r#"{
            "started_at_block_timestamp_ms": "1699564800000",
            "collateral_asset_deposit": "1000000000000000000000000",
            "borrow_asset_principal": "100000000",
            "borrow_asset_fees": {
                "total": "0",
                "fraction_as_u128_dividend": "0",
                "next_snapshot_index": 42,
                "pending_estimate": "0"
            },
            "fees": "500000",
            "borrow_asset_in_flight": "0",
            "collateral_asset_in_flight": "0",
            "liquidation_lock": "0"
        }"#;

        let position: BorrowPosition =
            serde_json::from_str(json).expect("Failed to deserialize old format");
        assert_eq!(position.fees, BorrowAssetAmount::new(500_000));
        assert_eq!(
            position.get_borrow_asset_principal(),
            BorrowAssetAmount::new(100_000_000)
        );
    }

    #[test]
    fn test_borrow_position_deserialize_mixed_old_new_format() {
        // Mixed format: old field name for interest (borrow_asset_fees), new field names for others
        let json = r#"{
            "started_at_block_timestamp_ms": "1699564800000",
            "collateral_asset_deposit": "1000000000000000000000000",
            "borrow_asset_principal": "100000000",
            "borrow_asset_fees": {
                "total": "0",
                "fraction_as_u128_dividend": "0",
                "next_snapshot_index": 42,
                "pending_estimate": "0"
            },
            "fees": "500000",
            "borrow_asset_in_flight": "0",
            "collateral_asset_in_flight": "0",
            "liquidation_lock": "0"
        }"#;

        let position: BorrowPosition =
            serde_json::from_str(json).expect("Failed to deserialize mixed format");
        assert_eq!(position.fees, BorrowAssetAmount::new(500_000));
        assert_eq!(
            position.get_borrow_asset_principal(),
            BorrowAssetAmount::new(100_000_000)
        );
        assert_eq!(
            position.get_total_collateral_amount(),
            CollateralAssetAmount::new(1_000_000_000_000_000_000_000_000)
        );
    }

    #[test]
    fn test_borrow_position_deserialize_defaults() {
        // Minimal JSON with only required fields, others should use defaults
        let json = r#"{
            "collateral_asset_deposit": "1000000000000000000000000",
            "borrow_asset_principal": "100000000",
            "interest": {
                "total": "0",
                "fraction_as_u128_dividend": "0",
                "next_snapshot_index": 42,
                "pending_estimate": "0"
            }
        }"#;

        let position: BorrowPosition =
            serde_json::from_str(json).expect("Failed to deserialize with defaults");
        assert_eq!(position.started_at_block_timestamp_ms, None);
        assert_eq!(position.fees, BorrowAssetAmount::new(0));
        assert_eq!(
            position.get_total_collateral_amount(),
            CollateralAssetAmount::new(1_000_000_000_000_000_000_000_000)
        );
    }

    // Backstop for `fuzz_borrow_overflow`: that target asserts correctness up
    // to the boundary but cannot observe the abort itself (libfuzzer-sys
    // aborts on panics before catch_unwind sees them). The contract's safety
    // property — overflow on principal + in_flight aborts — is asserted here.
    #[test]
    #[should_panic(expected = "attempt to add with overflow")]
    fn liability_overflow_aborts_on_principal_plus_in_flight() {
        let mut position = BorrowPosition::new(0);
        position.borrow_asset_principal = BorrowAssetAmount::new(u128::MAX);
        position.borrow_asset_in_flight = BorrowAssetAmount::new(1);
        let _ = position.get_total_borrow_asset_liability();
    }

    // Backstop for `fuzz_liquidations` (ENG-342). That target predicts and
    // skips the `1 < cr < mcr` band when `mcr * (1 - spread) <= 1`, because the
    // denominator `mcr * discount - 1` underflows there and libfuzzer-sys can't
    // distinguish that abort from a real crash. The abort — the actual symptom
    // of the tracked bug — is asserted here so the suppressed region is still
    // pinned down. (`Decimal`'s U512 subtraction underflow panics with
    // "arithmetic operation overflow", unlike a std-integer add.)
    #[test]
    #[should_panic(expected = "arithmetic operation overflow")]
    fn liquidatable_collateral_denominator_underflow_aborts() {
        use templar_primitives::dec;

        use crate::oracle::pyth::PythTimestamp;

        // Equal prices/decimals ⇒ cr is the pure amount ratio. collateral=102,
        // liability=100 ⇒ cr=1.02, inside (1, mcr=1.05). With spread=0.1,
        // discount=0.9 and mcr*discount=0.945 <= 1, so `mcr*discount - 1`
        // underflows on unsigned `Decimal` subtraction.
        let price = pyth::Price {
            price: 1.into(),
            conf: 0.into(),
            expo: 0,
            publish_time: PythTimestamp::from_secs(0),
        };
        let price_pair = PricePair::new(&price, 0, &price, 0).unwrap();

        let mut position = BorrowPosition::new(0);
        position.collateral_asset_deposit = CollateralAssetAmount::new(102);
        position.borrow_asset_principal = BorrowAssetAmount::new(100);

        let _ = position.liquidatable_collateral(&price_pair, dec!("1.05"), dec!("0.1"));
    }
}
