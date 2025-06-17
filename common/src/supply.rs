use std::ops::{Deref, DerefMut};

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
pub struct InactiveDeposit {
    pub amount: BorrowAssetAmount,
    pub activate_at_snapshot_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct SupplyPosition {
    started_at_block_timestamp_ms: Option<U64>,
    borrow_asset_deposit_active: BorrowAssetAmount,
    inactive_deposit: InactiveDeposit,
    pub borrow_asset_yield: Accumulator<BorrowAsset>,
}

impl SupplyPosition {
    pub fn new(current_snapshot_index: u32) -> Self {
        Self {
            started_at_block_timestamp_ms: None,
            borrow_asset_deposit_active: 0.into(),
            inactive_deposit: InactiveDeposit {
                amount: 0.into(),
                activate_at_snapshot_index: current_snapshot_index,
            },
            borrow_asset_yield: Accumulator::new(current_snapshot_index),
        }
    }

    pub fn get_borrow_asset_deposit_active(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposit_active
    }

    pub fn get_inactive_deposit(&self) -> InactiveDeposit {
        self.inactive_deposit
    }

    pub fn get_borrow_asset_deposit_total(&self) -> BorrowAssetAmount {
        let mut a = self.borrow_asset_deposit_active;
        a.join(self.inactive_deposit.amount);
        a
    }

    pub fn get_started_at_block_timestamp_ms(&self) -> Option<u64> {
        self.started_at_block_timestamp_ms.map(u64::from)
    }

    pub fn exists(&self) -> bool {
        !self.borrow_asset_deposit_active.is_zero()
            || !self.inactive_deposit.amount.is_zero()
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

    pub fn inner(&self) -> &SupplyPosition {
        &self.position
    }
}

impl<M: Deref<Target = Market>> SupplyPositionRef<M> {
    pub fn with_pending_yield_estimate(&mut self) {
        let mut pending_estimate = self.calculate_yield(u32::MAX).get_amount();
        if !self.market.current_snapshot.deposited_active.is_zero() {
            let yield_in_current_snapshot =
                u128::from(self.market.current_snapshot.yield_distribution)
                    * u128::from(self.position.get_borrow_asset_deposit_active())
                    / u128::from(self.market.current_snapshot.deposited_active);
            pending_estimate.join(yield_in_current_snapshot.into());
        }
        self.position.borrow_asset_yield.pending_estimate = pending_estimate;
    }

    pub fn calculate_yield(&self, snapshot_limit: u32) -> AccumulationRecord<BorrowAsset> {
        let mut next_snapshot_index = self.position.borrow_asset_yield.get_next_snapshot_index();

        let mut amount = u128::from(self.position.get_borrow_asset_deposit_active());
        let amount_inactive = self.position.get_inactive_deposit();
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
            if i == amount_inactive.activate_at_snapshot_index as usize {
                amount += u128::from(amount_inactive.amount);
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
            fraction_as_u128_dividend: accumulated.fractional_part_as_u128_dividend(),
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
        if accumulation_record.next_snapshot_index
            > self.position.inactive_deposit.activate_at_snapshot_index
        {
            // Moved to the next snapshot.
            let amount_inactive = self.position.inactive_deposit.amount;
            self.position.inactive_deposit.amount = 0.into();
            self.position
                .borrow_asset_deposit_active
                .join(amount_inactive);
            self.position.inactive_deposit.activate_at_snapshot_index =
                accumulation_record.next_snapshot_index;
        }

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

    pub fn record_withdrawal(
        &mut self,
        _proof: YieldAccumulationProof,
        mut amount: BorrowAssetAmount,
        block_timestamp_ms: u64,
    ) -> WithdrawalResolution {
        if self.position.inactive_deposit.amount > amount {
            self.position.inactive_deposit.amount.split(amount);
            self.market.borrow_asset_deposited_inactive.split(amount);
        } else {
            let amount_inactive = self.position.inactive_deposit.amount;
            let mut amount_remaining = amount;
            amount_remaining.split(amount_inactive);
            self.market
                .borrow_asset_deposited_inactive
                .split(amount_inactive);
            self.position.inactive_deposit.amount = 0.into();

            self.position
                .borrow_asset_deposit_active
                .split(amount_remaining)
                .unwrap_or_else(|| {
                    env::panic_str("Supply position `borrow_asset_deposit_active` underflow")
                });
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

        WithdrawalResolution {
            account_id: self.account_id.clone(),
            amount_to_account: amount,
            amount_to_fees,
        }
    }

    pub fn record_deposit(
        &mut self,
        _proof: YieldAccumulationProof,
        amount: BorrowAssetAmount,
        block_timestamp_ms: u64,
    ) {
        if self.position.started_at_block_timestamp_ms.is_none()
            || self.position.borrow_asset_deposit_active.is_zero()
        {
            self.position.started_at_block_timestamp_ms = Some(block_timestamp_ms.into());
        }

        self.position
            .inactive_deposit
            .amount
            .join(amount)
            .unwrap_or_else(|| {
                env::panic_str("Supply position `borrow_asset_deposit_inactive` overflow")
            });

        self.position.inactive_deposit.activate_at_snapshot_index =
            self.market.finalized_snapshots.len() + 1;

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
