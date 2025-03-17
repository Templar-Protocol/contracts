use near_sdk::{
    collections::{LookupMap, UnorderedMap},
    env, near,
    store::Vector,
    AccountId, BorshStorageKey, IntoStorageKey,
};

use crate::{
    asset::BorrowAssetAmount,
    borrow::{BorrowPosition, LinkedBorrowPosition, LinkedBorrowPositionMut},
    chain_time::ChainTime,
    event::MarketEvent,
    market::MarketConfiguration,
    number::Decimal,
    snapshot::Snapshot,
    static_yield::StaticYieldRecord,
    supply::{LinkedSupplyPosition, LinkedSupplyPositionMut, SupplyPosition},
    withdrawal_queue::{error::WithdrawalQueueLockError, WithdrawalQueue},
};

use super::WithdrawalResolution;

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
    pub(crate) supply_positions: UnorderedMap<AccountId, SupplyPosition>,
    pub(crate) borrow_positions: UnorderedMap<AccountId, BorrowPosition>,
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
            if let Some(previous_snapshot_index) = index.checked_sub(1) {
                if let Some(previous_snapshot) = self.snapshots.get(previous_snapshot_index) {
                    MarketEvent::SnapshotFinalized {
                        index: previous_snapshot_index,
                        snapshot: previous_snapshot.clone(),
                    }
                    .emit();
                }
            }
            index
        }
    }

    #[allow(clippy::missing_panics_doc)]
    pub fn get_borrow_asset_available_to_borrow(
        &self,
        current_contract_balance: BorrowAssetAmount,
    ) -> BorrowAssetAmount {
        // Safe because factor is guaranteed to be <=1, so value must still fit in u128.
        #[allow(clippy::unwrap_used)]
        let must_retain = ((1u32 - self.configuration.borrow_asset_maximum_usage_ratio)
            * self.borrow_asset_deposited.to_decimal())
        .to_u128_ceil()
        .unwrap();

        let known_available = current_contract_balance
            .to_u128()
            .saturating_sub(self.borrow_asset_in_flight.to_u128());

        known_available.saturating_sub(must_retain).into()
    }

    pub fn get_interest_rate_for_snapshot(&self, snapshot: &Snapshot) -> Decimal {
        self.configuration
            .borrow_interest_rate_strategy
            .at(snapshot.usage_ratio())
    }

    pub fn iter_supply_account_ids(&self) -> impl Iterator<Item = AccountId> + '_ {
        self.supply_positions.keys()
    }

    pub fn get_linked_supply_position(
        &self,
        account_id: AccountId,
    ) -> Option<LinkedSupplyPosition<&Self>> {
        self.supply_positions
            .get(&account_id)
            .map(|position| LinkedSupplyPosition::new(self, account_id, position))
    }

    pub fn get_linked_supply_position_mut(
        &mut self,
        account_id: AccountId,
    ) -> Option<LinkedSupplyPositionMut<&mut Self>> {
        self.supply_positions
            .get(&account_id)
            .map(|position| LinkedSupplyPositionMut::new(self, account_id, position))
    }

    pub fn get_or_create_linked_supply_position_mut(
        &mut self,
        account_id: AccountId,
    ) -> LinkedSupplyPositionMut<&mut Self> {
        let position = self
            .supply_positions
            .get(&account_id)
            .unwrap_or_else(|| SupplyPosition::new(self.snapshot()));

        LinkedSupplyPositionMut::new(self, account_id, position)
    }

    pub fn iter_borrow_account_ids(&self) -> impl Iterator<Item = AccountId> + '_ {
        self.borrow_positions.keys()
    }

    pub fn get_linked_borrow_position(
        &self,
        account_id: AccountId,
    ) -> Option<LinkedBorrowPosition<&Self>> {
        self.borrow_positions
            .get(&account_id)
            .map(|position| LinkedBorrowPosition::new(self, account_id, position))
    }

    pub fn get_linked_borrow_position_mut(
        &mut self,
        account_id: AccountId,
    ) -> Option<LinkedBorrowPositionMut<&mut Self>> {
        self.borrow_positions
            .get(&account_id)
            .map(|position| LinkedBorrowPositionMut::new(self, account_id, position))
    }

    pub fn get_or_create_linked_borrow_position_mut(
        &mut self,
        account_id: AccountId,
    ) -> LinkedBorrowPositionMut<&mut Self> {
        let position = self
            .borrow_positions
            .get(&account_id)
            .unwrap_or_else(|| BorrowPosition::new(self.snapshot()));

        LinkedBorrowPositionMut::new(self, account_id, position)
    }

    /// # Errors
    /// - If the withdrawal queue is already locked.
    /// - If the withdrawal queue is empty.
    pub fn try_lock_next_withdrawal_request(
        &mut self,
    ) -> Result<Option<WithdrawalResolution>, WithdrawalQueueLockError> {
        let (account_id, requested_amount) = self.withdrawal_queue.try_lock()?;

        let Some((amount, mut supply_position)) = self
            .get_linked_supply_position_mut(account_id.clone())
            .and_then(|supply_position| {
                // Cap withdrawal amount to deposit amount at most.
                let amount = supply_position
                    .inner()
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

        let resolution = supply_position.record_withdrawal(amount, env::block_timestamp_ms());

        Ok(Some(resolution))
    }

    pub fn record_borrow_asset_protocol_yield(&mut self, amount: BorrowAssetAmount) {
        let mut yield_record = self
            .static_yield
            .get(&self.configuration.protocol_account_id)
            .unwrap_or_default();

        yield_record.borrow_asset.join(amount);

        self.static_yield
            .insert(&self.configuration.protocol_account_id, &yield_record);
    }

    pub fn record_borrow_asset_yield_distribution(&mut self, mut amount: BorrowAssetAmount) {
        // Sanity.
        if amount.is_zero() {
            return;
        }

        MarketEvent::GlobalYieldDistributed {
            borrow_asset_amount: amount,
        }
        .emit();

        // First, static yield.

        let total_weight = u128::from(u16::from(self.configuration.yield_weights.total_weight()));
        let total_amount = amount.to_u128();
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
}
