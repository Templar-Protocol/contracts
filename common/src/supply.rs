use std::ops::{Deref, DerefMut};

use near_sdk::{json_types::U64, near, require, AccountId};

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
    pub active_real: BorrowAssetAmount,
    pub active_virtual: BorrowAssetAmount,
    pub incoming: Vec<IncomingDeposit>,
    pub outgoing: BorrowAssetAmount,
}

impl Deposit {
    pub fn active(&self) -> BorrowAssetAmount {
        self.active_real + self.active_virtual
    }

    pub fn total(&self) -> BorrowAssetAmount {
        let mut total = self.active_real + self.active_virtual + self.outgoing;
        for incoming in &self.incoming {
            total += incoming.amount_real + incoming.amount_virtual;
        }
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

    pub fn total_incoming_real(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposit
            .incoming
            .iter()
            .fold(BorrowAssetAmount::zero(), |total_incoming, incoming| {
                total_incoming + incoming.amount_real
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

        let mut amount = u128::from(self.position.borrow_asset_deposit.active());
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
                amount += u128::from(incoming.amount_real + incoming.amount_virtual);
            }

            let snapshot_active_supply = snapshot.active_supply();
            if !snapshot_active_supply.is_zero() {
                let snapshot_duration_ms = snapshot.end_timestamp_ms.0 - prev_end_timestamp_ms;
                let interest_paid_by_borrowers = Decimal::from(snapshot.borrow_asset_borrowed)
                    * snapshot.interest_rate
                    * snapshot_duration_ms
                    * YEAR_PER_MS;
                let other_yield = Decimal::from(snapshot.yield_distribution);
                accumulated +=
                    (interest_paid_by_borrowers + other_yield) * amount * weight_numerator
                        / u128::from(snapshot_active_supply)
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
            self.position.borrow_asset_deposit.active_real += deposit.amount_real;
            self.position.borrow_asset_deposit.active_virtual += deposit.amount_virtual;
        }
        self.position.borrow_asset_deposit.incoming = incoming.collect();

        // Calling market.snapshot() performs the market accounting
    }

    fn remove_active(&mut self, amount: BorrowAssetAmount) {
        let amount_virtual = if amount >= self.position.borrow_asset_deposit.active_virtual {
            self.position.borrow_asset_deposit.active_virtual
        } else {
            amount
        };
        let amount_real = amount - amount_virtual;

        self.position.borrow_asset_deposit.active_virtual -= amount_virtual;
        self.market.borrow_asset_deposited_active_virtual -= amount_virtual;
        self.position.borrow_asset_deposit.active_real -= amount_real;
        self.market.borrow_asset_deposited_active_real -= amount_real;
    }

    fn add_incoming(
        &mut self,
        amount_real: BorrowAssetAmount,
        amount_virtual: BorrowAssetAmount,
        activate_at_snapshot_index: u32,
    ) {
        let incoming = &mut self.position.borrow_asset_deposit.incoming;
        if let Some(deposit) = incoming
            .last_mut()
            .filter(|i| i.activate_at_snapshot_index == activate_at_snapshot_index)
        {
            deposit.amount_real += amount_real;
            deposit.amount_virtual += amount_virtual;
        } else {
            const MAX_INCOMING: usize = 4;
            require!(
                incoming.len() < MAX_INCOMING,
                "Too many deposits without running accumulation",
            );
            incoming.push(IncomingDeposit {
                activate_at_snapshot_index,
                amount_real,
                amount_virtual,
            });
        }

        if let Some(incoming) = self
            .market
            .borrow_asset_deposited_incoming
            .iter_mut()
            .find(|incoming| incoming.activate_at_snapshot_index == activate_at_snapshot_index)
        {
            incoming.amount_real += amount_real;
            incoming.amount_virtual += amount_virtual;
        } else {
            self.market
                .borrow_asset_deposited_incoming
                .push(IncomingDeposit {
                    activate_at_snapshot_index,
                    amount_real,
                    amount_virtual,
                });
        }
    }

    /// Returns the amount successfully removed from incoming.
    fn remove_incoming_real(&mut self, amount: BorrowAssetAmount) -> BorrowAssetAmount {
        let mut total = BorrowAssetAmount::zero();
        let mut removals = vec![];
        for newest in self.position.borrow_asset_deposit.incoming.iter_mut().rev() {
            if total + newest.amount_real >= amount {
                let delta = amount - total;
                total = amount;
                newest.amount_real -= delta;
                removals.push((newest.activate_at_snapshot_index, delta));
                break;
            }
            total += newest.amount_real;
            removals.push((newest.activate_at_snapshot_index, newest.amount_real));
            newest.amount_real = 0.into();
        }

        for (snapshot_index, amount_removed) in removals {
            let Some(market_incoming) = self.market.incoming_at_mut(snapshot_index) else {
                crate::panic_with_message("Invariant violation: Market incoming entry should exist if position incoming entry exists");
            };

            market_incoming.amount_real -= amount_removed;
        }

        self.position
            .borrow_asset_deposit
            .incoming
            .retain(|i| !i.amount_real.is_zero() || !i.amount_virtual.is_zero());

        self.market
            .borrow_asset_deposited_incoming
            .retain(|i| !i.amount_real.is_zero() || !i.amount_virtual.is_zero());

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

        // Claim virtual to real
        let claimable = Ord::min(
            self.position.borrow_asset_deposit.active_virtual,
            self.market.borrow_asset_virtual_credit,
        );
        if !claimable.is_zero() {
            self.market.borrow_asset_virtual_credit -= claimable;

            self.position.borrow_asset_deposit.active_virtual -= claimable;
            self.market.borrow_asset_deposited_active_virtual -= claimable;

            self.position.borrow_asset_deposit.active_real += claimable;
            self.market.borrow_asset_deposited_active_real += claimable;
        }

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

        let my_incoming = self.position.total_incoming_real();
        let my_active = self.position.get_deposit().active();
        let entitled_to_withdraw = my_incoming + my_active;

        if entitled_to_withdraw.is_zero() {
            return WithdrawalAttempt::EmptyPosition;
        }

        let requested_amount = requested_amount.min(entitled_to_withdraw);
        let available_to_me = self.market.active_supply() + my_incoming;
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
            self.remove_incoming_real(withdrawal_amount),
            "Invariant violation: remove_incoming_real(amount) > amount",
        );

        if !amount_to_remove.is_zero() {
            self.remove_active(amount_to_remove);
        }

        // The only way to withdraw from a position is if it already has a deposit.
        // Adding a deposit guarantees started_at_block_timestamp_ms != None
        let Some(U64(started_at_block_timestamp_ms)) =
            self.0.position.started_at_block_timestamp_ms
        else {
            crate::panic_with_message(
                "Invariant violation: Position with deposit has no timestamp",
            );
        };
        let supply_duration = block_timestamp_ms.saturating_sub(started_at_block_timestamp_ms);

        let amount_to_fees = self
            .market
            .configuration
            .supply_withdrawal_fee
            .of(withdrawal_amount, supply_duration)
            .unwrap_or_else(|| crate::panic_with_message("Fee calculation overflow"))
            .min(withdrawal_amount);

        let amount_to_account = withdrawal_amount.saturating_sub(amount_to_fees);

        self.market.borrow_asset_balance -= amount_to_account;
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
            self.market.borrow_asset_balance += withdrawal.amount_to_account;
            // TODO: Is this correct? Do we need to separate real & virtual for failed withdrawals too?
            self.add_incoming(amount, 0.into(), self.market.finalized_snapshots.len() + 1);
        }
    }

    pub fn record_deposit(
        &mut self,
        _proof: YieldAccumulationProof,
        amount: BorrowAssetAmount,
        block_timestamp_ms: u64,
    ) {
        if self.position.started_at_block_timestamp_ms.is_none()
            || self.position.borrow_asset_deposit.total().is_zero()
        {
            self.position.started_at_block_timestamp_ms = Some(block_timestamp_ms.into());
        }

        self.market.borrow_asset_balance += amount;
        self.add_incoming(amount, 0.into(), self.market.finalized_snapshots.len() + 1);

        if !amount.is_zero() {
            MarketEvent::SupplyDeposited {
                account_id: self.account_id.clone(),
                borrow_asset_amount: amount,
            }
            .emit();
        }
    }

    /// Converts an amount of borrow asset from the yield record to the
    /// deposit record, allowing the account to earn compound interest on
    /// their yield without withdrawing it.
    ///
    /// # Panics
    ///
    /// If `amount` is greater the amount in the yield record. The caller
    /// should probably use the return value of
    /// [`SupplyPositionRef::total_yield`] as an upper bound for this argument.
    pub fn record_yield_compound(
        &mut self,
        _proof: YieldAccumulationProof,
        amount: BorrowAssetAmount,
    ) {
        self.0.position.borrow_asset_yield.remove(amount);
        self.add_incoming(0.into(), amount, self.market.finalized_snapshots.len() + 1);
    }
}

#[derive(Debug)]
pub enum WithdrawalAttempt {
    Full(Withdrawal),
    Partial {
        withdrawal: Withdrawal,
        remaining: BorrowAssetAmount,
    },
    EmptyPosition,
    NoLiquidity,
}
