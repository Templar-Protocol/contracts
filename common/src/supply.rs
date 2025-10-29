use std::ops::{Deref, DerefMut};

use near_sdk::{env, json_types::U64, near, require, AccountId};

use crate::{
    accumulator::{AccumulationRecord, Accumulator},
    asset::{BorrowAsset, BorrowAssetAmount},
    event::MarketEvent,
    incoming_deposit::IncomingDeposit,
    market::{Market, Withdrawal},
    number::Decimal,
    YEAR_PER_MS,
};

/// This struct can only be constructed after accumulating yield on a
/// supply position. This serves as proof that the yield has accrued, so it
/// is safe to perform certain other operations.
pub struct YieldAccumulationProof(());

#[derive(Default, Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Deposit {
    pub active: BorrowAssetAmount,
    pub incoming: Vec<IncomingDeposit>,
    pub outgoing: BorrowAssetAmount,
}

impl Deposit {
    pub fn total(&self) -> BorrowAssetAmount {
        let mut total = self.active;
        for incoming in &self.incoming {
            total += incoming.amount;
        }
        total += self.outgoing;
        total
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
            borrow_asset_deposit: Deposit::default(),
            borrow_asset_yield: Accumulator::new(current_snapshot_index),
        }
    }

    pub fn get_deposit(&self) -> &Deposit {
        &self.borrow_asset_deposit
    }

    pub fn total_incoming(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposit
            .incoming
            .iter()
            .fold(BorrowAssetAmount::zero(), |total_incoming, incoming| {
                total_incoming + incoming.amount
            })
    }

    pub fn get_started_at_block_timestamp_ms(&self) -> Option<u64> {
        self.started_at_block_timestamp_ms.map(u64::from)
    }

    pub fn exists(&self) -> bool {
        !self.borrow_asset_deposit.total().is_zero()
            || !self.borrow_asset_yield.get_total().is_zero()
    }

    pub fn can_be_removed(&self) -> bool {
        !self.exists()
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
        self.position.borrow_asset_yield.pending_estimate =
            self.calculate_yield(u32::MAX).get_amount();
    }

    pub fn calculate_yield(&self, snapshot_limit: u32) -> AccumulationRecord<BorrowAsset> {
        let mut next_snapshot_index = self.position.borrow_asset_yield.get_next_snapshot_index();

        let mut amount = u128::from(self.position.borrow_asset_deposit.active);
        let mut accumulated = Decimal::ZERO;
        let mut next_incoming = 0;

        #[allow(clippy::unwrap_used, reason = "Guaranteed previous snapshot exists")]
        let mut prev_end_timestamp_ms = self
            .market
            .finalized_snapshots
            .get(next_snapshot_index.checked_sub(1).unwrap())
            .unwrap()
            .end_timestamp_ms
            .0;

        let weight_numerator = self.market.configuration.yield_weights.supply.get();
        let weight_denominator = self.market.configuration.yield_weights.total_weight().get();

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
            while let Some(incoming) = self
                .position
                .borrow_asset_deposit
                .incoming
                .get(next_incoming)
                .filter(|incoming| incoming.activate_at_snapshot_index as usize == i)
            {
                next_incoming += 1;
                amount += u128::from(incoming.amount);
            }

            if !snapshot.borrow_asset_deposited_active.is_zero() {
                let snapshot_duration_ms = snapshot.end_timestamp_ms.0 - prev_end_timestamp_ms;
                let interest_paid_by_borrowers = Decimal::from(snapshot.borrow_asset_borrowed)
                    * snapshot.interest_rate
                    * snapshot_duration_ms
                    * YEAR_PER_MS;
                let other_yield = Decimal::from(snapshot.yield_distribution);
                accumulated +=
                    (interest_paid_by_borrowers + other_yield) * amount * weight_numerator
                        / u128::from(snapshot.borrow_asset_deposited_active)
                        / weight_denominator;
            }

            next_snapshot_index = i as u32 + 1;
            prev_end_timestamp_ms = snapshot.end_timestamp_ms.0;
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

    fn activate_incoming(&mut self, through_snapshot_index: u32) {
        let mut incoming = self
            .position
            .borrow_asset_deposit
            .incoming
            .clone()
            .into_iter()
            .peekable();
        while let Some(deposit) =
            incoming.next_if(|d| d.activate_at_snapshot_index <= through_snapshot_index)
        {
            self.position.borrow_asset_deposit.active += deposit.amount;
        }
        self.position.borrow_asset_deposit.incoming = incoming.collect();

        // Calling market.snapshot() performs the market accounting
    }

    fn remove_active(&mut self, amount: BorrowAssetAmount) {
        self.position.borrow_asset_deposit.active -= amount;
        self.market.borrow_asset_deposited_active -= amount;
    }

    fn add_incoming(&mut self, amount: BorrowAssetAmount, activate_at_snapshot_index: u32) {
        let incoming = &mut self.position.borrow_asset_deposit.incoming;
        if let Some(deposit) = incoming
            .last_mut()
            .filter(|i| i.activate_at_snapshot_index == activate_at_snapshot_index)
        {
            deposit.amount += amount;
        } else {
            const MAX_INCOMING: usize = 4;
            require!(
                incoming.len() < MAX_INCOMING,
                "Too many deposits without running accumulation",
            );
            incoming.push(IncomingDeposit {
                activate_at_snapshot_index,
                amount,
            });
        }

        if let Some(incoming) = self
            .market
            .borrow_asset_deposited_incoming
            .iter_mut()
            .find(|incoming| incoming.activate_at_snapshot_index == activate_at_snapshot_index)
        {
            incoming.amount += amount;
        } else {
            self.market
                .borrow_asset_deposited_incoming
                .push(IncomingDeposit {
                    activate_at_snapshot_index,
                    amount,
                });
        }
    }

    /// Returns the amount successfully removed from incoming.
    fn remove_incoming(&mut self, amount: BorrowAssetAmount) -> BorrowAssetAmount {
        let mut total = BorrowAssetAmount::zero();
        while let Some(newest) = self.position.borrow_asset_deposit.incoming.pop() {
            total += newest.amount;

            let Some(market_incoming) = self
                .market
                .borrow_asset_deposited_incoming
                .iter_mut()
                .find(|incoming| {
                    incoming.activate_at_snapshot_index == newest.activate_at_snapshot_index
                })
            else {
                env::panic_str("Invariant violation: Market incoming entry should exist if position incoming entry exists");
            };
            market_incoming.amount = market_incoming.amount.unwrap_sub(newest.amount, "Invariant violation: Market incoming >= position incoming should hold for all snapshot indices");

            #[allow(clippy::comparison_chain)]
            if total == amount {
                return amount;
            } else if total > amount {
                self.add_incoming(total - amount, newest.activate_at_snapshot_index);
                return amount;
            }
        }

        total
    }

    pub fn accumulate_yield_partial(&mut self, snapshot_limit: u32) {
        require!(snapshot_limit > 0, "snapshot_limit must be nonzero");

        let accumulation_record = self.calculate_yield(snapshot_limit);
        self.activate_incoming(accumulation_record.next_snapshot_index);

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
        requested_amount: BorrowAssetAmount,
        block_timestamp_ms: u64,
    ) -> WithdrawalAttempt {
        //
        // Check liquidity & eligibility
        //

        let incoming = self.position.total_incoming();
        let active = self.position.get_deposit().active;
        let entitled_to_withdraw = incoming + active;

        if entitled_to_withdraw.is_zero() {
            return WithdrawalAttempt::EmptyPosition;
        }

        let requested_amount = requested_amount.min(entitled_to_withdraw);
        let generally_available = self
            .market
            .borrow_asset_deposited_active
            .saturating_sub(self.market.borrowed());
        let available_to_me = generally_available + incoming;
        let can_withdraw_now = entitled_to_withdraw.min(available_to_me);

        if can_withdraw_now.is_zero() {
            return WithdrawalAttempt::NoLiquidity;
        }

        let withdrawal_amount = requested_amount.min(can_withdraw_now);

        //
        // Execute removal
        //

        let mut amount_to_remove = withdrawal_amount;

        self.position.borrow_asset_deposit.outgoing += withdrawal_amount;

        amount_to_remove = amount_to_remove.unwrap_sub(
            self.remove_incoming(withdrawal_amount),
            "Invariant violation: remove_incoming(amount) > amount",
        );

        if !amount_to_remove.is_zero() {
            self.remove_active(amount_to_remove);
        }

        // The only way to withdraw from a position is if it already has a deposit.
        // Adding a deposit guarantees started_at_block_timestamp_ms != None
        let Some(U64(started_at_block_timestamp_ms)) =
            self.0.position.started_at_block_timestamp_ms
        else {
            env::panic_str("Invariant violation: Position with deposit has no timestamp");
        };
        let supply_duration = block_timestamp_ms.saturating_sub(started_at_block_timestamp_ms);

        let amount_to_fees = self
            .market
            .configuration
            .supply_withdrawal_fee
            .of(withdrawal_amount, supply_duration)
            .unwrap_or_else(|| env::panic_str("Fee calculation overflow"))
            .min(withdrawal_amount);

        let amount_to_account = withdrawal_amount.saturating_sub(amount_to_fees);

        self.market.borrow_asset_withdrawal_in_flight += amount_to_account;

        let withdrawal = Withdrawal {
            account_id: self.account_id.clone(),
            amount_to_account,
            amount_to_fees,
        };

        if requested_amount > can_withdraw_now {
            WithdrawalAttempt::Partial {
                withdrawal,
                remaining: requested_amount.saturating_sub(can_withdraw_now),
            }
        } else {
            WithdrawalAttempt::Full(withdrawal)
        }
    }

    pub fn record_withdrawal_final(&mut self, withdrawal: &Withdrawal, success: bool) {
        let amount = withdrawal.amount_to_account + withdrawal.amount_to_fees;

        self.position.borrow_asset_deposit.outgoing -= amount;
        self.market.borrow_asset_withdrawal_in_flight -= withdrawal.amount_to_account;

        if success {
            self.market
                .record_borrow_asset_protocol_yield(withdrawal.amount_to_fees);

            MarketEvent::SupplyWithdrawn {
                account_id: self.account_id.clone(),
                borrow_asset_amount_to_account: withdrawal.amount_to_account,
                borrow_asset_amount_to_fees: withdrawal.amount_to_fees,
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

        if !amount.is_zero() {
            MarketEvent::SupplyDeposited {
                account_id: self.account_id.clone(),
                borrow_asset_amount: amount,
            }
            .emit();
        }
    }

    pub fn record_yield_withdrawal(&mut self, amount: BorrowAssetAmount) {
        self.0.position.borrow_asset_yield.remove(amount);
    }
}

pub enum WithdrawalAttempt {
    Full(Withdrawal),
    Partial {
        withdrawal: Withdrawal,
        remaining: BorrowAssetAmount,
    },
    EmptyPosition,
    NoLiquidity,
}
