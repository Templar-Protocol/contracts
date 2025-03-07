use near_sdk::{
    collections::{LookupMap, UnorderedMap},
    env, near, require,
    store::Vector,
    AccountId, BorshStorageKey, IntoStorageKey,
};

use crate::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
    chain_time::ChainTime,
    market::MarketConfiguration,
    market_log::MarketLog,
    number::Decimal,
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::{error::WithdrawalQueueLockError, WithdrawalQueue},
};

use super::OraclePriceProof;

pub const MS_IN_A_YEAR: u128 = 31_556_952_000; // 1000 * 60 * 60 * 24 * 365.2425

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    SupplyPositions,
    BorrowPositions,
    Logs,
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
    pub logs: Vector<MarketLog>,
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
            logs: Vector::new(key!(Logs)),
            withdrawal_queue: WithdrawalQueue::new(key!(WithdrawalQueue)),
            static_yield: LookupMap::new(key!(StaticYield)),
        };

        // So we never have to worry about logs being empty.
        // This means that expressions like `self.logs.len() - 1` will never
        // underflow.
        self_.log();

        self_
    }

    #[allow(clippy::unwrap_used, clippy::missing_panics_doc)]
    pub fn get_last_log(&self) -> &MarketLog {
        self.logs.get(self.logs.len() - 1).unwrap()
    }

    pub fn log(&mut self) -> u32 {
        self.log_yield_distribution(BorrowAssetAmount::zero())
    }

    fn log_yield_distribution(&mut self, yield_distribution: BorrowAssetAmount) -> u32 {
        let chain_time = ChainTime::now();

        if let Some((last_index, old_log)) = self.logs.len().checked_sub(1).and_then(|last_index| {
            self.logs
                .get(last_index)
                .filter(|log| log.chain_time == chain_time)
                .map(|log| (last_index, log))
        }) {
            let new_log = MarketLog {
                chain_time,
                timestamp_ms: old_log.timestamp_ms,
                deposited: self.borrow_asset_deposited,
                borrowed: self.borrow_asset_borrowed,
                yield_distribution: {
                    let mut y = old_log.yield_distribution;
                    y.join(yield_distribution);
                    y
                },
            };
            self.logs.replace(last_index, new_log);
            last_index
        } else {
            let index = self.logs.len();
            let new_log = MarketLog {
                chain_time,
                timestamp_ms: env::block_timestamp_ms().into(),
                deposited: self.borrow_asset_deposited,
                borrowed: self.borrow_asset_borrowed,
                yield_distribution,
            };
            self.logs.push(new_log);
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

    pub fn get_interest_rate_for_log(&self, log: &MarketLog) -> Decimal {
        let borrowed: Decimal = log.borrowed.as_u128().into();
        let deposited: Decimal = log.deposited.as_u128().into();
        let usage_ratio = borrowed / deposited;
        self.configuration
            .borrow_interest_rate_strategy
            .at(usage_ratio)
    }

    /// # Errors
    /// - If the withdrawal queue is already locked.
    /// - If the withdrawal queue is empty.
    pub fn try_lock_next_withdrawal_request(
        &mut self,
    ) -> Result<Option<(AccountId, BorrowAssetAmount)>, WithdrawalQueueLockError> {
        let (account_id, requested_amount) = self.withdrawal_queue.try_lock()?;

        let Some((amount, mut supply_position)) =
            self.supply_positions
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

        Ok(Some((account_id, amount)))
    }

    fn record_borrow_asset_yield_distribution(&mut self, mut amount: BorrowAssetAmount) {
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
        self.log_yield_distribution(amount);
    }

    pub fn record_supply_position_borrow_asset_deposit(
        &mut self,
        supply_position: &mut SupplyPosition,
        amount: BorrowAssetAmount,
    ) {
        self.accumulate_yield_on_supply_position(supply_position, ChainTime::now());
        supply_position
            .increase_borrow_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset overflow"));

        self.borrow_asset_deposited
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited overflow"));

        self.log();
    }

    pub fn record_supply_position_borrow_asset_withdrawal(
        &mut self,
        supply_position: &mut SupplyPosition,
        amount: BorrowAssetAmount,
    ) -> BorrowAssetAmount {
        self.accumulate_yield_on_supply_position(supply_position, ChainTime::now());
        let withdrawn = supply_position
            .decrease_borrow_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset underflow"));

        self.borrow_asset_deposited
            .split(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited underflow"));

        self.log();

        withdrawn
    }

    pub fn record_borrow_position_collateral_asset_deposit(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: CollateralAssetAmount,
    ) {
        self.accumulate_interest_on_borrow_position(borrow_position, ChainTime::now());
        borrow_position
            .increase_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset overflow"));
    }

    pub fn record_borrow_position_collateral_asset_withdrawal(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: CollateralAssetAmount,
    ) {
        self.accumulate_interest_on_borrow_position(borrow_position, ChainTime::now());
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
        self.accumulate_interest_on_borrow_position(borrow_position, ChainTime::now());

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
        self.accumulate_interest_on_borrow_position(borrow_position, ChainTime::now());

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
        self.accumulate_interest_on_borrow_position(borrow_position, ChainTime::now());

        borrow_position.borrow_asset_fees.add_one_time_fee(fee);
        borrow_position
            .increase_borrow_asset_principal(amount, env::block_timestamp_ms())
            .unwrap_or_else(|| env::panic_str("Increase borrow asset principal overflow"));

        self.borrow_asset_borrowed
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset borrowed overflow"));
        self.log();
    }

    pub fn record_borrow_position_borrow_asset_repay(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: BorrowAssetAmount,
    ) {
        self.accumulate_interest_on_borrow_position(borrow_position, ChainTime::now());

        let liability_reduction = borrow_position
            .reduce_borrow_asset_liability(amount)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));

        require!(
            liability_reduction.amount_remaining.is_zero(),
            "Overpayment not supported",
        );

        self.record_borrow_asset_yield_distribution(liability_reduction.amount_to_fees);

        // SAFETY: It should be impossible to panic here, since assets that
        // have not yet been borrowed cannot be repaid.
        self.borrow_asset_borrowed
            .split(liability_reduction.amount_to_principal)
            .unwrap_or_else(|| env::panic_str("Borrow asset borrowed underflow"));

        self.log();
    }

    pub fn accumulate_interest_on_borrow_position(
        &mut self,
        borrow_position: &mut BorrowPosition,
        until_chain_time: ChainTime,
    ) {
        self.log();

        let (accumulated, next_log_index) = self.calculate_borrow_position_interest(
            borrow_position.get_borrow_asset_principal(),
            borrow_position.borrow_asset_fees.next_log_index,
            |(_, log)| log.chain_time < until_chain_time,
        );

        borrow_position
            .borrow_asset_fees
            .accumulate_fees(accumulated, next_log_index);
    }

    /// This function must only be used to estimate interest for the purpose of account monitoring.
    /// The borrow position MUST NOT be written back to storage.
    pub fn estimate_instantaneous_borrow_position_interest(
        &self,
        borrow_position: &mut BorrowPosition,
    ) {
        let (calculated_from_logs, next_log_index) = self.calculate_borrow_position_interest(
            borrow_position.get_borrow_asset_principal(),
            borrow_position.borrow_asset_fees.get_next_log_index(),
            |_| true,
        );
        borrow_position
            .borrow_asset_fees
            .accumulate_fees(calculated_from_logs, next_log_index);

        // Add on the bit representing the "in-progress" log.
        let last_log = self.get_last_log();
        let interest_rate = self.get_interest_rate_for_log(last_log);
        let duration_ms = Decimal::from(env::block_timestamp_ms() - last_log.timestamp_ms.0);
        let ms_in_a_year = Decimal::from(MS_IN_A_YEAR);
        let interest_rate_part = interest_rate * duration_ms / ms_in_a_year;
        let interest = interest_rate_part
            * Decimal::from(borrow_position.get_borrow_asset_principal().as_u128());

        borrow_position
            .borrow_asset_fees
            .add_one_time_fee(interest.to_u128_ceil().unwrap().into());
    }

    pub(crate) fn calculate_borrow_position_interest(
        &self,
        principal_in_span: BorrowAssetAmount,
        mut next_log_index: u32,
        take_while: impl FnMut(&(u32, &MarketLog)) -> bool,
    ) -> (BorrowAssetAmount, u32) {
        let principal = Decimal::from(principal_in_span.as_u128());

        let mut accumulated = Decimal::ZERO;

        let mut it = self
            .logs
            .iter()
            .enumerate()
            .skip(next_log_index as usize)
            .map(|(i, log)| (i as u32, log))
            .take_while(take_while)
            .peekable();

        while let Some((i, log)) = it.next() {
            let Some(end_timestamp_ms) = it.peek().map(|(_, next_log)| next_log.timestamp_ms.0)
            else {
                // Cannot calculate duration.
                break;
            };

            let total_borrowed = Decimal::from(log.borrowed.as_u128());
            let total_deposited = Decimal::from(log.deposited.as_u128());
            let utilization_ratio = total_borrowed / total_deposited;
            let interest_rate_per_year = self
                .configuration
                .borrow_interest_rate_strategy
                .at(utilization_ratio);
            let ms_in_a_year = Decimal::from(MS_IN_A_YEAR);
            let duration_ms: Decimal = end_timestamp_ms
                .checked_sub(log.timestamp_ms.0)
                .unwrap_or_else(|| {
                    env::panic_str(&format!(
                        "Invariant violation: Log timestamp decrease at #{}.",
                        i + 1,
                    ))
                })
                .into();

            let interest = principal * interest_rate_per_year * duration_ms / ms_in_a_year;

            accumulated += interest;

            // TODO: This is definitely correct for this function.
            // Need to investigate to see if the same logic is necessary in
            // yield accumulation fns.
            next_log_index = i + 1;
        }

        (accumulated.to_u128_ceil().unwrap().into(), next_log_index)
    }

    /// In order for yield calculations to be accurate, this function MUST
    /// BE CALLED every time a supply position's deposit changes. This
    /// requirement is largely met by virtue of the fact that
    /// `SupplyPosition->borrow_asset_deposit` is a private field and can only
    /// be modified via `Self::record_supply_position_*` methods.
    pub fn accumulate_yield_on_supply_position(
        &self,
        supply_position: &mut SupplyPosition,
        until_chain_time: ChainTime,
    ) {
        let (accumulated, finished_at_log_index) = self.calculate_supply_position_yield(
            supply_position.get_borrow_asset_deposit(),
            supply_position.borrow_asset_yield.until_log_index,
            |(_, log)| log.chain_time < until_chain_time,
        );

        supply_position
            .borrow_asset_yield
            .accumulate_yield(accumulated, finished_at_log_index);
    }

    #[allow(clippy::missing_panics_doc)]
    pub fn calculate_supply_position_yield(
        &self,
        amount_deposited_during_interval: BorrowAssetAmount,
        from_log_index: u32,
        take_while: impl FnMut(&(u32, &MarketLog)) -> bool,
    ) -> (BorrowAssetAmount, u32) {
        if self.logs.is_empty() {
            return (0.into(), from_log_index);
        }

        let amount = Decimal::from(amount_deposited_during_interval.as_u128());

        let mut accumulated = Decimal::ZERO;
        let mut finished_at_log_index = from_log_index;

        for (i, log) in self
            .logs
            .iter()
            .enumerate()
            .skip(from_log_index as usize)
            .map(|(i, log)| (i as u32, log))
            .take_while(take_while)
        {
            let deposited = Decimal::from(log.deposited.as_u128());
            let distributed = Decimal::from(log.yield_distribution.as_u128());
            let share = amount * distributed / deposited;
            accumulated += share;

            finished_at_log_index = i;
        }

        (
            accumulated.to_u128_floor().unwrap().into(),
            finished_at_log_index,
        )
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
        borrow_position.full_liquidation(self.log());

        self.borrow_asset_borrowed.split(principal);

        // TODO: Is it correct to only care about the original principal here?
        if recovered_amount.split(principal).is_some() {
            // distribute yield
            // record_borrow_asset_yield_distribution will add logs, no need to do it:
            // self.add_or_update_log(None);
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
