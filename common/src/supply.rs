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
pub struct Deposit {
    pub active: BorrowAssetAmount,
    pub incoming: BorrowAssetAmount,
    pub activate_incoming_at_snapshot_index: u32,
    pub outgoing: BorrowAssetAmount,
}

impl Deposit {
    pub fn total(&self) -> BorrowAssetAmount {
        let mut total = self.active;
        total.join(self.incoming);
        total.join(self.outgoing);
        total
    }
}

impl Deposit {
    pub fn new(snapshot_index: u32) -> Self {
        Self {
            active: 0.into(),
            incoming: 0.into(),
            activate_incoming_at_snapshot_index: snapshot_index,
            outgoing: 0.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct SupplyPosition {
    started_at_block_timestamp_ms: Option<U64>,
    borrow_asset_deposit: Deposit,
    pub borrow_asset_yield: Accumulator<BorrowAsset>,
}

impl SupplyPosition {
    pub fn new(current_snapshot_index: u32) -> Self {
        Self {
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
            let mut amount = u128::from(self.position.borrow_asset_deposit.active);
            if self
                .position
                .borrow_asset_deposit
                .activate_incoming_at_snapshot_index
                == self.market.finalized_snapshots.len()
            {
                amount += u128::from(self.position.borrow_asset_deposit.incoming);
            }
            let yield_in_current_snapshot =
                u128::from(self.market.current_snapshot.yield_distribution) * amount
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
                .activate_incoming_at_snapshot_index as usize
            {
                amount += u128::from(self.position.borrow_asset_deposit.incoming);
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

    fn add_active(&mut self, amount: BorrowAssetAmount) {
        self.position
            .borrow_asset_deposit
            .active
            .join(amount)
            .unwrap_or_else(|| {
                env::panic_str("Supply position `borrow_asset_deposit.active` overflow")
            });
        self.market
            .borrow_asset_deposited_active
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Market `borrow_asset_deposited_active` overflow"));
    }

    fn remove_active(&mut self, amount: BorrowAssetAmount) {
        self.position
            .borrow_asset_deposit
            .active
            .split(amount)
            .unwrap_or_else(|| {
                env::panic_str("Supply position `borrow_asset_deposit.active` underflow")
            });
        self.market
            .borrow_asset_deposited_active
            .split(amount)
            .unwrap_or_else(|| env::panic_str("Market `borrow_asset_deposited_active` underflow"));
    }

    fn add_incoming(&mut self, amount: BorrowAssetAmount, activate_at_snapshot_index: u32) {
        self.position
            .borrow_asset_deposit
            .incoming
            .join(amount)
            .unwrap_or_else(|| {
                env::panic_str("Supply position `borrow_asset_deposit.incoming` overflow")
            });
        self.position
            .borrow_asset_deposit
            .activate_incoming_at_snapshot_index = activate_at_snapshot_index;
    }

    fn remove_incoming(&mut self, amount: BorrowAssetAmount, activate_at_snapshot_index: u32) {
        self.position
            .borrow_asset_deposit
            .incoming
            .split(amount)
            .unwrap_or_else(|| {
                env::panic_str("Supply position `borrow_asset_deposit.incoming` underflow")
            });
        self.position
            .borrow_asset_deposit
            .activate_incoming_at_snapshot_index = activate_at_snapshot_index;
    }

    pub fn accumulate_yield_partial(&mut self, snapshot_limit: u32) {
        require!(snapshot_limit > 0, "snapshot_limit must be nonzero");
        self.market.snapshot();

        let accumulation_record = self.calculate_yield(snapshot_limit);
        let until_snapshot_index = accumulation_record.next_snapshot_index;
        if until_snapshot_index
            > self
                .position
                .borrow_asset_deposit
                .activate_incoming_at_snapshot_index
        {
            let amount = self.position.borrow_asset_deposit.incoming;
            self.remove_incoming(amount, until_snapshot_index);
            self.add_active(amount);
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

    pub fn record_withdrawal_initial(
        &mut self,
        _proof: YieldAccumulationProof,
        mut amount: BorrowAssetAmount,
        block_timestamp_ms: u64,
    ) -> WithdrawalResolution {
        self.position
            .borrow_asset_deposit
            .outgoing
            .join(amount)
            .unwrap_or_else(|| {
                env::panic_str("Supply position `borrow_asset_deposit.withdrawing` overflow")
            });

        if self.position.borrow_asset_deposit.incoming > amount {
            self.remove_incoming(amount, self.market.finalized_snapshots.len() + 1);
        } else {
            let amount_incoming = self.position.borrow_asset_deposit.incoming;
            let mut amount_remaining = amount;
            self.remove_incoming(amount_incoming, self.market.finalized_snapshots.len() + 1);
            amount_remaining.split(amount_incoming);
            self.remove_active(amount_remaining);
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

        WithdrawalResolution {
            account_id: self.account_id.clone(),
            amount_to_account: amount,
            amount_to_fees,
        }
    }

    pub fn record_withdrawal_final(
        &mut self,
        withdrawal_resolution: &WithdrawalResolution,
        success: bool,
    ) {
        let mut amount = withdrawal_resolution.amount_to_account;
        amount.join(withdrawal_resolution.amount_to_fees);

        self.position
            .borrow_asset_deposit
            .outgoing
            .split(amount)
            .unwrap_or_else(|| {
                env::panic_str("Supply position `borrow_asset_deposit.withdrawing` underflow")
            });

        if success {
            MarketEvent::SupplyWithdrawn {
                account_id: self.account_id.clone(),
                borrow_asset_amount_to_account: withdrawal_resolution.amount_to_account,
                borrow_asset_amount_to_fees: withdrawal_resolution.amount_to_fees,
            }
            .emit();
        } else {
            self.add_incoming(amount, self.market.finalized_snapshots.len() + 1);
        }
    }

    pub fn record_deposit(
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

        self.add_incoming(amount, self.market.finalized_snapshots.len() + 1);

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
