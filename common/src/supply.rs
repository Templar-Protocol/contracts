use std::{
    fmt::Display,
    ops::{Deref, DerefMut},
};

use near_sdk::{env, json_types::U64, near, require, AccountId};

use crate::{
    accumulator::{AccumulationRecord, Accumulator},
    asset::{BorrowAsset, BorrowAssetAmount, FungibleAssetAmount},
    event::MarketEvent,
    market::{Market, WithdrawalResolution},
    number::Decimal,
};

/// This struct can only be constructed after accumulating yield on a
/// supply position. This serves as proof that the yield has accrued, so it
/// is safe to perform certain other operations.
pub struct YieldAccumulationProof(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Deposit {
    pub active: BorrowAssetAmount,
    pub inactive: BorrowAssetAmount,
    pub activate_at_snapshot_index: u32,
}

impl Deposit {
    pub fn total(&self) -> BorrowAssetAmount {
        let mut total = self.active;
        total.join(self.inactive);
        total
    }

    pub fn activate_until(&mut self, until_snapshot_index: u32) {
        if until_snapshot_index > self.activate_at_snapshot_index {
            self.active.join(self.inactive);
            self.inactive = BorrowAssetAmount::zero();
            self.activate_at_snapshot_index = until_snapshot_index;
        }
    }
}

impl Deposit {
    pub fn new(snapshot_index: u32) -> Self {
        Self {
            active: 0.into(),
            inactive: 0.into(),
            activate_at_snapshot_index: snapshot_index,
        }
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum SupplyPositionStatus {
    #[default]
    Ready,
    Withdrawing,
}

impl Display for SupplyPositionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                SupplyPositionStatus::Ready => "Ready",
                SupplyPositionStatus::Withdrawing => "Withdrawing",
            }
        )
    }
}

pub mod error {
    use thiserror::Error;

    use super::SupplyPositionStatus;

    #[derive(Debug, Error)]
    #[error("This operation cannot be performed during `{0}` status")]
    pub struct InvalidOperationDuringStatusError(pub SupplyPositionStatus);
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct SupplyPosition {
    status: SupplyPositionStatus,
    started_at_block_timestamp_ms: Option<U64>,
    borrow_asset_deposit: Deposit,
    pub borrow_asset_yield: Accumulator<BorrowAsset>,
}

impl SupplyPosition {
    pub fn new(current_snapshot_index: u32) -> Self {
        Self {
            status: SupplyPositionStatus::Ready,
            started_at_block_timestamp_ms: None,
            borrow_asset_deposit: Deposit::new(current_snapshot_index),
            borrow_asset_yield: Accumulator::new(current_snapshot_index),
        }
    }

    pub fn get_deposit(&self) -> &Deposit {
        &self.borrow_asset_deposit
    }

    pub fn get_started_at_block_timestamp_ms(&self) -> Option<u64> {
        self.started_at_block_timestamp_ms.map(u64::from)
    }

    pub fn exists(&self) -> bool {
        !self.borrow_asset_deposit.total().is_zero()
            || !self.borrow_asset_yield.get_total().is_zero()
    }
}

pub struct SupplyPositionRef<M> {
    market: M,
    account_id: AccountId,
    position: SupplyPosition,
}

impl<M> SupplyPositionRef<M> {
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

    pub fn total_deposit(&self) -> BorrowAssetAmount {
        self.position.borrow_asset_deposit.total()
    }

    pub fn total_yield(&self) -> BorrowAssetAmount {
        self.position.borrow_asset_yield.get_total()
    }

    pub fn inner(&self) -> &SupplyPosition {
        &self.position
    }
}

impl<M: Deref<Target = Market>> SupplyPositionRef<M> {
    pub fn is_within_allowable_range(&self) -> bool {
        self.market
            .configuration
            .supply_range
            .contains(self.position.borrow_asset_deposit.total())
    }

    pub fn with_pending_yield_estimate(&mut self) {
        let mut pending_estimate = self.calculate_yield(u32::MAX).get_amount();
        if !self.market.current_snapshot.deposited_active.is_zero() {
            let yield_in_current_snapshot =
                u128::from(self.market.current_snapshot.yield_distribution)
                    * u128::from(self.position.borrow_asset_deposit.active)
                    / u128::from(self.market.current_snapshot.deposited_active);
            pending_estimate.join(yield_in_current_snapshot.into());
        }
        self.position.borrow_asset_yield.pending_estimate = pending_estimate;
    }

    pub fn calculate_yield(&self, snapshot_limit: u32) -> AccumulationRecord<BorrowAsset> {
        let mut next_snapshot_index = self.position.borrow_asset_yield.get_next_snapshot_index();

        let mut amount = u128::from(self.position.borrow_asset_deposit.active);
        let mut accumulated = Decimal::ZERO;

        #[allow(
            clippy::cast_possible_truncation,
            reason = "Assume # of snapshots is never >u32::MAX"
        )]
        for (i, snapshot) in self
            .market
            .finalized_snapshots
            .iter()
            .enumerate()
            .skip(next_snapshot_index as usize)
            .take(snapshot_limit as usize)
        {
            if i == self
                .position
                .borrow_asset_deposit
                .activate_at_snapshot_index as usize
            {
                amount += u128::from(self.position.borrow_asset_deposit.inactive);
            }

            if !snapshot.deposited_active.is_zero() {
                accumulated += amount * Decimal::from(snapshot.yield_distribution)
                    / Decimal::from(snapshot.deposited_active);
            }

            next_snapshot_index = i as u32 + 1;
        }

        AccumulationRecord {
            // Accumulated amount is derived from real balances, so it should
            // never overflow underlying data type.
            #[allow(clippy::unwrap_used, reason = "Derived from real balances")]
            amount: accumulated.to_u128_floor().unwrap().into(),
            next_snapshot_index,
        }
    }
}

pub struct SupplyPositionGuard<'a>(SupplyPositionRef<&'a mut Market>);

impl Drop for SupplyPositionGuard<'_> {
    fn drop(&mut self) {
        self.0
            .market
            .supply_positions
            .insert(&self.0.account_id, &self.0.position);
    }
}

impl<'a> Deref for SupplyPositionGuard<'a> {
    type Target = SupplyPositionRef<&'a mut Market>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SupplyPositionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a> SupplyPositionGuard<'a> {
    pub fn new(market: &'a mut Market, account_id: AccountId, position: SupplyPosition) -> Self {
        Self(SupplyPositionRef::new(market, account_id, position))
    }

    pub fn accumulate_yield_partial(&mut self, snapshot_limit: u32) {
        require!(snapshot_limit > 0, "snapshot_limit must be nonzero");
        self.market.snapshot();

        let accumulation_record = self.calculate_yield(snapshot_limit);
        self.position
            .borrow_asset_deposit
            .activate_until(accumulation_record.next_snapshot_index);

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

    pub fn accumulate_yield(&mut self) -> YieldAccumulationProof {
        self.accumulate_yield_partial(u32::MAX);
        YieldAccumulationProof(())
    }

    /// # Errors
    ///
    /// - If the position is not marked as withdrawing.
    pub fn try_end_withdrawal(&mut self) -> Result<(), error::InvalidOperationDuringStatusError> {
        if self.position.status != SupplyPositionStatus::Withdrawing {
            return Err(error::InvalidOperationDuringStatusError(
                self.position.status,
            ));
        }

        self.position.status = SupplyPositionStatus::Ready;
        Ok(())
    }

    /// # Errors
    ///
    ///  - If the position is already marked as withdrawing.
    pub fn try_start_withdrawal(
        &mut self,
        _proof: YieldAccumulationProof,
        mut amount: BorrowAssetAmount,
        block_timestamp_ms: u64,
    ) -> Result<WithdrawalResolution, error::InvalidOperationDuringStatusError> {
        if self.position.status != SupplyPositionStatus::Ready {
            return Err(error::InvalidOperationDuringStatusError(
                self.position.status,
            ));
        }

        self.position.status = SupplyPositionStatus::Withdrawing;

        if self.position.borrow_asset_deposit.inactive > amount {
            self.position.borrow_asset_deposit.inactive.split(amount);
            self.market.borrow_asset_deposited_inactive.split(amount);
        } else {
            let amount_inactive = self.position.borrow_asset_deposit.inactive;
            let mut amount_remaining = amount;
            amount_remaining.split(amount_inactive);
            self.market
                .borrow_asset_deposited_inactive
                .split(amount_inactive);
            self.position.borrow_asset_deposit.inactive = 0.into();

            self.position
                .borrow_asset_deposit
                .active
                .split(amount_remaining)
                .unwrap_or_else(|| env::panic_str("Supply position `deposit.active` underflow"));
            self.market
                .borrow_asset_deposited_active
                .split(amount_remaining)
                .unwrap_or_else(|| {
                    env::panic_str("Market `borrow_asset_deposited_active` underflow")
                });
        }

        self.market.snapshot();

        // The only way to withdraw from a position is if it already has a deposit.
        // Adding a deposit guarantees started_at_block_timestamp_ms != None
        #[allow(clippy::unwrap_used, reason = "Guaranteed to never panic")]
        let started_at_block_timestamp_ms =
            self.0.position.started_at_block_timestamp_ms.unwrap().0;
        let supply_duration = block_timestamp_ms.saturating_sub(started_at_block_timestamp_ms);

        let amount_to_fees = self
            .market
            .configuration
            .supply_withdrawal_fee
            .of(amount, supply_duration)
            .unwrap_or_else(|| env::panic_str("Fee calculation overflow"));

        if amount.split(amount_to_fees).is_none() {
            amount = FungibleAssetAmount::zero();
        }

        MarketEvent::SupplyWithdrawn {
            account_id: self.account_id.clone(),
            borrow_asset_amount_to_account: amount,
            borrow_asset_amount_to_fees: amount_to_fees,
        }
        .emit();

        Ok(WithdrawalResolution {
            account_id: self.account_id.clone(),
            amount_to_account: amount,
            amount_to_fees,
        })
    }

    /// # Errors
    ///
    /// - If the position is currently withdrawing.
    pub fn try_record_deposit(
        &mut self,
        proof: YieldAccumulationProof,
        amount: BorrowAssetAmount,
        block_timestamp_ms: u64,
    ) -> Result<(), error::InvalidOperationDuringStatusError> {
        if self.position.status != SupplyPositionStatus::Ready {
            return Err(error::InvalidOperationDuringStatusError(
                self.position.status,
            ));
        }

        self.record_deposit_inner(proof, amount, block_timestamp_ms);
        Ok(())
    }

    // pub fn record_withdrawal_refund(
    //     &mut self,
    //     proof: YieldAccumulationProof,
    //     amount: BorrowAssetAmount,
    //     block_timestamp_ms: u64,
    // ) {
    //     self.record_deposit_inner(proof, amount, block_timestamp_ms);
    // }

    fn record_deposit_inner(
        &mut self,
        _proof: YieldAccumulationProof,
        amount: BorrowAssetAmount,
        block_timestamp_ms: u64,
    ) {
        if self.position.started_at_block_timestamp_ms.is_none()
            || self.position.borrow_asset_deposit.active.is_zero()
        {
            self.position.started_at_block_timestamp_ms = Some(block_timestamp_ms.into());
        }

        self.position
            .borrow_asset_deposit
            .inactive
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Supply position `deposit.inactive` overflow"));

        self.position
            .borrow_asset_deposit
            .activate_at_snapshot_index = self.market.finalized_snapshots.len() + 1;

        self.market
            .borrow_asset_deposited_inactive
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Market `borrow_asset_deposited_inactive` overflow"));

        self.market.snapshot();

        if !amount.is_zero() {
            MarketEvent::SupplyDeposited {
                account_id: self.account_id.clone(),
                borrow_asset_amount: amount,
            }
            .emit();
        }
    }

    pub fn record_yield_withdrawal(
        &mut self,
        amount: BorrowAssetAmount,
    ) -> Option<BorrowAssetAmount> {
        self.0.position.borrow_asset_yield.remove(amount)
    }
}
