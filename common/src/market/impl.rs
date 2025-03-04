use borsh::BorshDeserialize;
use near_sdk::{
    collections::{LookupMap, UnorderedMap, Vector},
    env, near, require, AccountId, BorshStorageKey, IntoStorageKey,
};

use crate::{
    asset::{
        AssetClass, BorrowAsset, BorrowAssetAmount, CollateralAssetAmount, FungibleAssetAmount,
    },
    balance_log::{search_balance_logs, BalanceLog, SearchResult},
    borrow::BorrowPosition,
    market::MarketConfiguration,
    number::Decimal,
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::{error::WithdrawalQueueLockError, WithdrawalQueue},
};

use super::OraclePriceProof;

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    SupplyPositions,
    BorrowPositions,
    TotalBorrowAssetDepositedLog,
    BorrowAssetYieldDistributionLog,
    WithdrawalQueue,
    StaticYield,
}

#[near]
pub struct Market {
    prefix: Vec<u8>,
    pub configuration: MarketConfiguration,
    pub borrow_asset_deposited: BorrowAssetAmount,
    pub borrow_asset_in_flight: BorrowAssetAmount,
    pub supply_positions: UnorderedMap<AccountId, SupplyPosition>,
    pub borrow_positions: UnorderedMap<AccountId, BorrowPosition>,
    pub total_borrow_asset_deposited_log: Vector<BalanceLog<BorrowAsset>>,
    pub borrow_asset_yield_distribution_log: Vector<BalanceLog<BorrowAsset>>,
    pub withdrawal_queue: WithdrawalQueue,
    pub static_yield: LookupMap<AccountId, StaticYieldRecord>,
}

impl Market {
    pub fn new(prefix: impl IntoStorageKey, configuration: MarketConfiguration) -> Self {
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
        Self {
            prefix: prefix.clone(),
            configuration,
            borrow_asset_deposited: 0.into(),
            borrow_asset_in_flight: 0.into(),
            supply_positions: UnorderedMap::new(key!(SupplyPositions)),
            borrow_positions: UnorderedMap::new(key!(BorrowPositions)),
            total_borrow_asset_deposited_log: Vector::new(key!(TotalBorrowAssetDepositedLog)),
            borrow_asset_yield_distribution_log: Vector::new(key!(BorrowAssetYieldDistributionLog)),
            withdrawal_queue: WithdrawalQueue::new(key!(WithdrawalQueue)),
            static_yield: LookupMap::new(key!(StaticYield)),
        }
    }

    pub fn get_borrow_asset_available_to_borrow(
        &self,
        current_contract_balance: BorrowAssetAmount,
    ) -> BorrowAssetAmount {
        let must_retain = ((1u32 - &self.configuration.maximum_borrow_asset_usage_ratio)
            * self.borrow_asset_deposited.as_u128())
        .to_u128_ceil()
        .unwrap();

        let known_available = current_contract_balance
            .as_u128()
            .saturating_sub(self.borrow_asset_in_flight.as_u128());

        known_available.saturating_sub(must_retain).into()
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

    fn log_total_borrow_asset_deposited(&mut self, amount: BorrowAssetAmount) {
        let current_epoch = env::epoch_height();
        let last_index = self.total_borrow_asset_deposited_log.len() - 1;
        let last = self
            .total_borrow_asset_deposited_log
            .get(last_index)
            .filter(|log| log.epoch_height.0 == current_epoch);

        if let Some(mut last) = last {
            last.amount = amount;
            self.total_borrow_asset_deposited_log
                .replace(last_index, &last);
        } else {
            self.total_borrow_asset_deposited_log
                .push(&BalanceLog::new(current_epoch, amount));
        }
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

        let current_epoch = env::epoch_height();
        let last_index = self.borrow_asset_yield_distribution_log.len() - 1;
        let log = self
            .borrow_asset_yield_distribution_log
            .get(last_index)
            .filter(|log| log.epoch_height.0 == current_epoch);
        if let Some(mut log) = log {
            log.amount.join(amount);
            self.borrow_asset_yield_distribution_log
                .replace(last_index, &log);
        } else {
            self.borrow_asset_yield_distribution_log
                .push(&BalanceLog::new(current_epoch, amount));
        }
    }

    pub fn record_supply_position_borrow_asset_deposit(
        &mut self,
        supply_position: &mut SupplyPosition,
        amount: BorrowAssetAmount,
    ) {
        self.accumulate_yield_on_supply_position(supply_position, env::epoch_height());
        supply_position
            .increase_borrow_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset overflow"));

        self.borrow_asset_deposited
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited overflow"));

        self.log_total_borrow_asset_deposited(self.borrow_asset_deposited);
    }

    pub fn record_supply_position_borrow_asset_withdrawal(
        &mut self,
        supply_position: &mut SupplyPosition,
        amount: BorrowAssetAmount,
    ) -> BorrowAssetAmount {
        self.accumulate_yield_on_supply_position(supply_position, env::epoch_height());
        let withdrawn = supply_position
            .decrease_borrow_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset underflow"));

        self.borrow_asset_deposited
            .split(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited underflow"));

        self.log_total_borrow_asset_deposited(self.borrow_asset_deposited);

        withdrawn
    }

    pub fn record_borrow_position_collateral_asset_deposit(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: CollateralAssetAmount,
    ) {
        borrow_position
            .increase_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset overflow"));
    }

    pub fn record_borrow_position_collateral_asset_withdrawal(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: CollateralAssetAmount,
    ) {
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
        fees: BorrowAssetAmount,
    ) {
        borrow_position
            .borrow_asset_fees
            .accumulate_fees(fees, env::epoch_height());
        borrow_position
            .increase_borrow_asset_principal(amount, env::block_timestamp_ms())
            .unwrap_or_else(|| env::panic_str("Increase borrow asset principal overflow"));
    }

    pub fn record_borrow_position_borrow_asset_repay(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: BorrowAssetAmount,
    ) {
        let liability_reduction = borrow_position
            .reduce_borrow_asset_liability(amount)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));

        require!(
            liability_reduction.amount_remaining.is_zero(),
            "Overpayment not supported",
        );

        self.record_borrow_asset_yield_distribution(liability_reduction.amount_to_fees);
    }

    /// In order for yield calculations to be accurate, this function MUST
    /// BE CALLED every time a supply position's deposit changes. This
    /// requirement is largely met by virtue of the fact that
    /// `SupplyPosition->borrow_asset_deposit` is a private field and can only
    /// be modified via `Self::record_supply_position_*` methods.
    pub fn accumulate_yield_on_supply_position(
        &self,
        supply_position: &mut SupplyPosition,
        until_epoch_height: u64,
    ) {
        let (accumulated, last_epoch_height) = self.calculate_supply_position_yield(
            &self.borrow_asset_yield_distribution_log,
            supply_position
                .borrow_asset_yield
                .last_updated_epoch_height
                .0,
            supply_position.get_borrow_asset_deposit(),
            until_epoch_height,
        );

        supply_position
            .borrow_asset_yield
            .accumulate_yield(accumulated, last_epoch_height);
    }

    #[allow(clippy::missing_panics_doc)]
    pub fn calculate_supply_position_yield<T: AssetClass + BorshDeserialize>(
        &self,
        yield_distribution_logs: &Vector<BalanceLog<T>>,
        last_updated_epoch_height: u64,
        borrow_asset_deposited_during_interval: BorrowAssetAmount,
        until_epoch_height: u64,
    ) -> (FungibleAssetAmount<T>, u64) {
        let (starting_index, starting_epoch_height) =
            match search_balance_logs(yield_distribution_logs, last_updated_epoch_height) {
                SearchResult::Found { index, log } => (index, log.epoch_height.0),
                SearchResult::NotFound { index_below } => {
                    let index = index_below + 1;
                    match yield_distribution_logs
                        .get(index)
                        .filter(|log| log.epoch_height.0 < until_epoch_height)
                    {
                        Some(log) => (index, log.epoch_height.0),
                        None => return (0.into(), last_updated_epoch_height),
                    }
                }
            };

        let mut accumulated_fees_in_span = FungibleAssetAmount::<T>::zero();

        let mut total_assets_deposited_at_distribution = match search_balance_logs(
            &self.total_borrow_asset_deposited_log,
            starting_epoch_height,
        ) {
            SearchResult::Found { index, log } => (index, log),
            SearchResult::NotFound { index_below } => (
                index_below,
                if let Some(log) = self.total_borrow_asset_deposited_log.get(index_below) {
                    log
                } else {
                    return (0.into(), last_updated_epoch_height);
                },
            ),
        };

        // This value is not necessary for correctness; it just reduces
        // duplicate reads.
        let mut next_total_assets_deposited_at_distribution = self
            .total_borrow_asset_deposited_log
            .get(total_assets_deposited_at_distribution.0 + 1);

        let mut last_epoch_height = last_updated_epoch_height;

        for i in starting_index..yield_distribution_logs.len() {
            let log = yield_distribution_logs.get(i).unwrap();
            if log.epoch_height.0 >= until_epoch_height {
                break;
            }

            // Now, we are looking for the latest total asset deposited amount
            // AT OR BEFORE the current yield distribution log.
            while let Some(next) = next_total_assets_deposited_at_distribution
                .clone()
                .filter(|l| l.epoch_height.0 <= log.epoch_height.0)
            {
                total_assets_deposited_at_distribution.0 += 1;
                total_assets_deposited_at_distribution.1 = next;

                next_total_assets_deposited_at_distribution = self
                    .total_borrow_asset_deposited_log
                    .get(total_assets_deposited_at_distribution.0 + 1);
            }

            let share_fraction = Decimal::from(borrow_asset_deposited_during_interval.as_u128())
                / total_assets_deposited_at_distribution.1.amount.as_u128();

            let share_amount = FungibleAssetAmount::new(
                (share_fraction * log.amount.as_u128())
                    .to_u128_floor()
                    .unwrap(),
            );

            accumulated_fees_in_span.join(share_amount);

            last_epoch_height = log.epoch_height.0;
        }

        (accumulated_fees_in_span, last_epoch_height)
    }

    pub fn can_borrow_position_be_liquidated(
        &self,
        account_id: &AccountId,
        oracle_price_proof: OraclePriceProof,
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
        borrow_position.full_liquidation(env::epoch_height());

        // TODO: Is it correct to only care about the original principal here?
        if recovered_amount.split(principal).is_some() {
            // distribute yield
            self.record_borrow_asset_yield_distribution(recovered_amount);
        } else {
            // we took a loss
            // TODO: some sort of recovery for suppliers
            todo!("Took a loss during liquidation");
        }
    }
}
