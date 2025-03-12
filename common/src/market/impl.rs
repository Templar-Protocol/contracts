use near_sdk::{
    collections::{LookupMap, UnorderedMap},
    env, near,
    store::Vector,
    AccountId, BorshStorageKey, IntoStorageKey,
};

use crate::{
    accumulator::AccumulationRecord,
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
    chain_time::ChainTime,
    market::MarketConfiguration,
    number::Decimal,
    snapshot::Snapshot,
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::{error::WithdrawalQueueLockError, WithdrawalQueue},
};

use super::{OraclePriceProof, WithdrawalExecution};

pub const MS_IN_A_YEAR: u128 = 31_556_952_000; // 1000 * 60 * 60 * 24 * 365.2425

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    SupplyPositions,
    BorrowPositions,
    Snapshots,
    WithdrawalQueue,
    StaticYield,
}

#[near]
pub struct Market {
    prefix: Vec<u8>,
    pub configuration: MarketConfiguration,
    pub borrow_asset_deposited: BorrowAssetAmount,
    pub borrow_asset_in_flight: BorrowAssetAmount,
    pub borrow_asset_borrowed: BorrowAssetAmount,
    pub supply_positions: UnorderedMap<AccountId, SupplyPosition>,
    pub borrow_positions: UnorderedMap<AccountId, BorrowPosition>,
    pub snapshots: Vector<Snapshot>,
    pub withdrawal_queue: WithdrawalQueue,
    pub static_yield: LookupMap<AccountId, StaticYieldRecord>,
}

impl Market {
    pub fn new(prefix: impl IntoStorageKey, configuration: MarketConfiguration) -> Self {
        if let Err(e) = configuration.validate() {
            env::panic_str(&e.to_string());
        }

        let prefix = prefix.into_storage_key();
        macro_rules! key {
            ($key: ident) => {
                [
                    prefix.as_slice(),
                    StorageKey::$key.into_storage_key().as_slice(),
                ]
                .concat()
            };
        }
        let mut self_ = Self {
            prefix: prefix.clone(),
            configuration,
            borrow_asset_deposited: 0.into(),
            borrow_asset_in_flight: 0.into(),
            borrow_asset_borrowed: 0.into(),
            supply_positions: UnorderedMap::new(key!(SupplyPositions)),
            borrow_positions: UnorderedMap::new(key!(BorrowPositions)),
            snapshots: Vector::new(key!(Snapshots)),
            withdrawal_queue: WithdrawalQueue::new(key!(WithdrawalQueue)),
            static_yield: LookupMap::new(key!(StaticYield)),
        };

        // So we never have to worry about snapshots being empty.
        // This means that expressions like `self.snapshots.len() - 1` will never
        // underflow.
        self_.snapshot();

        self_
    }

    #[allow(clippy::unwrap_used, clippy::missing_panics_doc)]
    pub fn get_last_snapshot(&self) -> &Snapshot {
        self.snapshots.get(self.snapshots.len() - 1).unwrap()
    }

    pub fn snapshot(&mut self) -> u32 {
        self.snapshot_with_yield_distribution(BorrowAssetAmount::zero())
    }

    fn snapshot_with_yield_distribution(&mut self, yield_distribution: BorrowAssetAmount) -> u32 {
        let chain_time = ChainTime::now();

        if let Some((last_index, old_snapshot)) =
            self.snapshots.len().checked_sub(1).and_then(|last_index| {
                self.snapshots
                    .get(last_index)
                    .filter(|s| s.chain_time == chain_time)
                    .map(|s| (last_index, s))
            })
        {
            let new_snapshot = Snapshot {
                chain_time,
                timestamp_ms: old_snapshot.timestamp_ms,
                deposited: self.borrow_asset_deposited,
                borrowed: self.borrow_asset_borrowed,
                yield_distribution: {
                    let mut y = old_snapshot.yield_distribution;
                    y.join(yield_distribution);
                    y
                },
            };
            self.snapshots.replace(last_index, new_snapshot);
            last_index
        } else {
            let index = self.snapshots.len();
            let new_snapshot = Snapshot {
                chain_time,
                timestamp_ms: env::block_timestamp_ms().into(),
                deposited: self.borrow_asset_deposited,
                borrowed: self.borrow_asset_borrowed,
                yield_distribution,
            };
            self.snapshots.push(new_snapshot);
            index
        }
    }

    pub fn get_borrow_asset_available_to_borrow(
        &self,
        current_contract_balance: BorrowAssetAmount,
    ) -> BorrowAssetAmount {
        let must_retain = ((1u32 - self.configuration.maximum_borrow_asset_usage_ratio)
            * self.borrow_asset_deposited.as_u128())
        .to_u128_ceil()
        .unwrap();

        let known_available = current_contract_balance
            .as_u128()
            .saturating_sub(self.borrow_asset_in_flight.as_u128());

        known_available.saturating_sub(must_retain).into()
    }

    pub fn get_interest_rate_for_snapshot(&self, snapshot: &Snapshot) -> Decimal {
        let borrowed: Decimal = snapshot.borrowed.as_u128().into();
        let deposited: Decimal = snapshot.deposited.as_u128().into();
        let usage_ratio = if deposited.is_zero() {
            Decimal::ZERO
        } else {
            borrowed / deposited
        };
        self.configuration
            .borrow_interest_rate_strategy
            .at(usage_ratio)
    }

    /// # Errors
    /// - If the withdrawal queue is already locked.
    /// - If the withdrawal queue is empty.
    pub fn try_lock_next_withdrawal_request(
        &mut self,
    ) -> Result<Option<WithdrawalExecution>, WithdrawalQueueLockError> {
        let (account_id, requested_amount) = self.withdrawal_queue.try_lock()?;

        let Some((mut amount, mut supply_position)) = self
            .supply_positions
            .get(&account_id)
            .and_then(|supply_position| {
                // Cap withdrawal amount to deposit amount at most.
                let amount = supply_position
                    .get_borrow_asset_deposit()
                    .min(requested_amount);

                if amount.is_zero() {
                    None
                } else {
                    Some((amount, supply_position))
                }
            })
        else {
            // The amount that the entry is eligible to withdraw is zero, so skip it.
            self.withdrawal_queue
                .try_pop()
                .unwrap_or_else(|| env::panic_str("Inconsistent state")); // we just locked the queue
            return Ok(None);
        };

        self.record_supply_position_borrow_asset_withdrawal(&mut supply_position, amount);

        self.supply_positions.insert(&account_id, &supply_position);

        let amount_to_fees = self
            .configuration
            .supply_withdrawal_fee
            .of(
                amount,
                // Guaranteed to exist because position is nonzero (can withdraw).
                env::block_timestamp_ms()
                    - supply_position.started_at_block_timestamp_ms.unwrap().0,
            )
            .unwrap();

        amount.split(amount_to_fees).unwrap();

        Ok(Some(WithdrawalExecution {
            account_id,
            amount_to_account: amount,
            amount_to_fees,
        }))
    }

    pub fn record_borrow_asset_yield_distribution(&mut self, mut amount: BorrowAssetAmount) {
        // Sanity.
        if amount.is_zero() {
            return;
        }

        // First, static yield.

        let total_weight = u128::from(u16::from(self.configuration.yield_weights.total_weight()));
        let total_amount = amount.as_u128();
        if total_weight != 0 {
            for (account_id, share) in &self.configuration.yield_weights.r#static {
                #[allow(clippy::unwrap_used)]
                let portion = amount
                    .split(
                        // Safety:
                        // total_weight is guaranteed >0 and <=u16::MAX
                        // share is guaranteed <=u16::MAX
                        // Therefore, as long as total_amount <= u128::MAX / u16::MAX, this will never overflow.
                        // u128::MAX / u16::MAX == 5192376087906286159508272029171713 (0x10001000100010001000100010001)
                        // With 24 decimals, that's about 5,192,376,087 tokens.
                        // TODO: Fix.
                        total_amount
                            .checked_mul(u128::from(*share))
                            .unwrap() // TODO: This one might panic.
                        / total_weight, // This will never panic: is never div0
                    )
                    // Safety:
                    // Guaranteed share <= total_weight
                    // Guaranteed sum(shares) == total_weight
                    // Guaranteed sum(floor(total_amount * share / total_weight) for each share in shares) <= total_amount
                    // Therefore this should never panic.
                    .unwrap();

                let mut yield_record = self.static_yield.get(account_id).unwrap_or_default();
                // Assuming borrow_asset is implemented correctly:
                // this only panics if the circulating supply is somehow >u128::MAX
                // and we have somehow obtained >u128::MAX amount.
                // TODO: Include warning somewhere about tokens with >u128::MAX supply.
                //
                // Otherwise, borrow_asset is implemented incorrectly.
                // TODO: If that is the case, how to deal?
                #[allow(clippy::unwrap_used)]
                yield_record.borrow_asset.join(portion).unwrap();
                self.static_yield.insert(account_id, &yield_record);
            }
        }

        // Next, dynamic (supply-based) yield.
        self.snapshot_with_yield_distribution(amount);
    }

    pub fn record_supply_position_borrow_asset_deposit(
        &mut self,
        supply_position: &mut SupplyPosition,
        amount: BorrowAssetAmount,
    ) {
        self.accumulate_supply_position_yield(supply_position);
        supply_position
            .increase_borrow_asset_deposit(amount, env::block_timestamp_ms())
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset overflow"));

        self.borrow_asset_deposited
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited overflow"));

        self.snapshot();
    }

    pub fn record_supply_position_borrow_asset_withdrawal(
        &mut self,
        supply_position: &mut SupplyPosition,
        amount: BorrowAssetAmount,
    ) -> BorrowAssetAmount {
        self.accumulate_supply_position_yield(supply_position);
        let withdrawn = supply_position
            .decrease_borrow_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset underflow"));

        self.borrow_asset_deposited
            .split(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited underflow"));

        self.snapshot();

        withdrawn
    }

    pub fn record_borrow_position_collateral_asset_deposit(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: CollateralAssetAmount,
    ) {
        self.accumulate_borrow_position_interest(borrow_position);
        borrow_position
            .increase_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset overflow"));
    }

    pub fn record_borrow_position_collateral_asset_withdrawal(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: CollateralAssetAmount,
    ) {
        self.accumulate_borrow_position_interest(borrow_position);
        borrow_position
            .decrease_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset underflow"));
    }

    pub fn record_borrow_position_borrow_asset_in_flight_start(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: BorrowAssetAmount,
        fees: BorrowAssetAmount,
    ) {
        self.accumulate_borrow_position_interest(borrow_position);

        self.borrow_asset_in_flight
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset in flight amount overflow"));
        borrow_position
            .temporary_lock
            .join(amount)
            .and_then(|()| borrow_position.temporary_lock.join(fees))
            .unwrap_or_else(|| env::panic_str("Borrow position in flight amount overflow"));
    }

    pub fn record_borrow_position_borrow_asset_in_flight_end(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: BorrowAssetAmount,
        fees: BorrowAssetAmount,
    ) {
        self.accumulate_borrow_position_interest(borrow_position);

        // This should never panic, because a given amount of in-flight borrow
        // asset should always be added before it is removed.
        self.borrow_asset_in_flight
            .split(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset in flight amount underflow"));
        borrow_position
            .temporary_lock
            .split(amount)
            .and_then(|_| borrow_position.temporary_lock.split(fees))
            .unwrap_or_else(|| env::panic_str("Borrow position in flight amount underflow"));
    }

    pub fn record_borrow_position_borrow_asset_withdrawal(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: BorrowAssetAmount,
        fee: BorrowAssetAmount,
    ) {
        self.accumulate_borrow_position_interest(borrow_position);

        borrow_position.borrow_asset_fees.add_once(fee);
        borrow_position
            .increase_borrow_asset_principal(amount, env::block_timestamp_ms())
            .unwrap_or_else(|| env::panic_str("Increase borrow asset principal overflow"));

        self.borrow_asset_borrowed
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset borrowed overflow"));
        self.snapshot();
    }

    pub fn record_borrow_position_borrow_asset_repay(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: BorrowAssetAmount,
    ) -> BorrowAssetAmount {
        self.accumulate_borrow_position_interest(borrow_position);

        let liability_reduction = borrow_position
            .reduce_borrow_asset_liability(amount)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));

        self.record_borrow_asset_yield_distribution(liability_reduction.amount_to_fees);

        // SAFETY: It should be impossible to panic here, since assets that
        // have not yet been borrowed cannot be repaid.
        self.borrow_asset_borrowed
            .split(liability_reduction.amount_to_principal)
            .unwrap_or_else(|| env::panic_str("Borrow asset borrowed underflow"));

        self.snapshot();

        liability_reduction.amount_remaining
    }

    pub fn accumulate_borrow_position_interest(&mut self, borrow_position: &mut BorrowPosition) {
        self.snapshot();

        let accumulation_record = self.calculate_borrow_position_interest(
            borrow_position.get_borrow_asset_principal(),
            borrow_position.borrow_asset_fees.next_snapshot_index,
            u32::MAX,
        );

        borrow_position
            .borrow_asset_fees
            .accumulate(accumulation_record);
    }

    #[must_use]
    pub fn calculate_borrow_position_instantaneous_pending_interest(
        &self,
        borrow_position: &BorrowPosition,
    ) -> BorrowAssetAmount {
        let mut amount = self
            .calculate_borrow_position_interest(
                borrow_position.get_borrow_asset_principal(),
                borrow_position.borrow_asset_fees.get_next_snapshot_index(),
                u32::MAX,
            )
            .get_amount();

        // Add the amount representing the "in-progress" snapshot.
        let last_snapshot_part =
            self.calculate_borrow_position_last_snapshot_interest(borrow_position);

        amount.join(last_snapshot_part);

        amount
    }

    pub(crate) fn calculate_borrow_position_last_snapshot_interest(
        &self,
        borrow_position: &BorrowPosition,
    ) -> BorrowAssetAmount {
        let last_snapshot = self.get_last_snapshot();
        let interest_rate = self.get_interest_rate_for_snapshot(last_snapshot);
        let duration_ms = Decimal::from(env::block_timestamp_ms() - last_snapshot.timestamp_ms.0);
        let ms_in_a_year = Decimal::from(MS_IN_A_YEAR);
        let interest_rate_part = interest_rate * duration_ms / ms_in_a_year;
        let interest = interest_rate_part
            * Decimal::from(borrow_position.get_borrow_asset_principal().as_u128());

        interest.to_u128_ceil().unwrap().into()
    }

    pub(crate) fn calculate_borrow_position_interest(
        &self,
        principal_in_span: BorrowAssetAmount,
        mut next_snapshot_index: u32,
        limit: u32,
    ) -> AccumulationRecord<BorrowAsset> {
        let principal = Decimal::from(principal_in_span.as_u128());

        let mut accumulated = Decimal::ZERO;

        let mut it = self
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

    /// In order for yield calculations to be accurate, this function MUST
    /// BE CALLED every time a supply position's deposit changes. This
    /// requirement is largely met by virtue of the fact that
    /// `SupplyPosition->borrow_asset_deposit` is a private field and can only
    /// be modified via `Self::record_supply_position_*` methods.
    pub fn accumulate_supply_position_yield(&mut self, supply_position: &mut SupplyPosition) {
        self.snapshot();

        let accumulation_record = self.calculate_supply_position_yield(
            supply_position.get_borrow_asset_deposit(),
            supply_position.borrow_asset_yield.next_snapshot_index,
        );

        supply_position
            .borrow_asset_yield
            .accumulate(accumulation_record);
    }

    /// This function must only be used to estimate interest for the purpose of account monitoring.
    #[must_use]
    pub fn calculate_supply_position_instantaneous_pending_yield(
        &self,
        supply_position: &SupplyPosition,
    ) -> BorrowAssetAmount {
        let mut amount = self
            .calculate_supply_position_yield(
                supply_position.get_borrow_asset_deposit(),
                supply_position.borrow_asset_yield.next_snapshot_index,
            )
            .get_amount();

        // Calculate the amount representing the "in-progress" snapshot.
        let current_snapshot_part =
            self.calculate_supply_position_last_snapshot_yield(supply_position);

        amount.join(current_snapshot_part);

        amount
    }

    pub fn calculate_supply_position_last_snapshot_yield(
        &self,
        supply_position: &SupplyPosition,
    ) -> BorrowAssetAmount {
        let deposit = Decimal::from(supply_position.get_borrow_asset_deposit().as_u128());
        if deposit.is_zero() {
            return BorrowAssetAmount::zero();
        }

        let last_snapshot = self.get_last_snapshot();
        let total_deposited = Decimal::from(last_snapshot.deposited.as_u128());
        if total_deposited.is_zero() {
            // divzero safety
            return BorrowAssetAmount::zero();
        }
        let supply_weight = Decimal::from(self.configuration.yield_weights.supply.get());
        // This is guaranteed to be nonzero, so no divzero issue.
        let total_weight = Decimal::from(self.configuration.yield_weights.total_weight().get());
        let total_yield_distribution = Decimal::from(last_snapshot.yield_distribution.as_u128());
        let estimate_current_snapshot =
            total_yield_distribution * deposit * supply_weight / total_deposited / total_weight;

        estimate_current_snapshot.to_u128_floor().unwrap().into()
    }

    #[allow(clippy::missing_panics_doc)]
    pub fn calculate_supply_position_yield(
        &self,
        amount_deposited_during_interval: BorrowAssetAmount,
        mut next_snapshot_index: u32,
    ) -> AccumulationRecord<BorrowAsset> {
        if self.snapshots.is_empty() {
            return AccumulationRecord::empty(next_snapshot_index);
        }

        let amount = Decimal::from(amount_deposited_during_interval.as_u128());

        let mut accumulated = Decimal::ZERO;

        let mut it = self.snapshots.iter();
        // Skip the last snapshot, which may be incomplete.
        it.next_back();

        for (i, snapshot) in it
            .enumerate()
            .skip(next_snapshot_index as usize)
            .map(|(i, s)| (i as u32, s))
        {
            let deposited = Decimal::from(snapshot.deposited.as_u128());
            let distributed = Decimal::from(snapshot.yield_distribution.as_u128());
            let share = amount * distributed / deposited;
            accumulated += share;

            next_snapshot_index = i + 1;
        }

        AccumulationRecord {
            amount: accumulated.to_u128_floor().unwrap().into(),
            next_snapshot_index,
        }
    }

    pub fn can_borrow_position_be_liquidated(
        &self,
        account_id: &AccountId,
        oracle_price_proof: &OraclePriceProof,
    ) -> bool {
        let Some(borrow_position) = self.borrow_positions.get(account_id) else {
            return false;
        };

        self.configuration
            .borrow_status(
                &borrow_position,
                oracle_price_proof,
                env::block_timestamp_ms(),
            )
            .is_liquidation()
    }

    pub fn record_liquidation_lock(&mut self, borrow_position: &mut BorrowPosition) {
        borrow_position.liquidation_lock = true;
    }

    pub fn record_liquidation_unlock(&mut self, borrow_position: &mut BorrowPosition) {
        borrow_position.liquidation_lock = false;
    }

    pub fn record_full_liquidation(
        &mut self,
        borrow_position: &mut BorrowPosition,
        mut recovered_amount: BorrowAssetAmount,
    ) {
        let principal = borrow_position.get_borrow_asset_principal();
        borrow_position.full_liquidation(self.snapshot());

        self.borrow_asset_borrowed.split(principal);

        // TODO: Is it correct to only care about the original principal here?
        if recovered_amount.split(principal).is_some() {
            // distribute yield
            // record_borrow_asset_yield_distribution will take snapshot, no need to do it.
            self.record_borrow_asset_yield_distribution(recovered_amount);
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
