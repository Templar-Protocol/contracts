use near_sdk::{
    collections::{LookupMap, UnorderedMap},
    env, near, AccountId, BorshStorageKey, IntoStorageKey,
};

use crate::{
    accumulator::{AccumulationRecord, Accumulator},
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowPositionGuard, BorrowPositionRef},
    chunked_append_only_list::ChunkedAppendOnlyList,
    event::MarketEvent,
    incoming_deposit::IncomingDeposit,
    market::MarketConfiguration,
    number::Decimal,
    snapshot::Snapshot,
    supply::{SupplyPosition, SupplyPositionGuard, SupplyPositionRef},
    time_chunk::TimeChunk,
    withdrawal_queue::WithdrawalQueue,
    YEAR_PER_MS,
};

#[derive(Debug, Copy, Clone)]
pub struct SnapshotProof(());

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    SupplyPositions,
    BorrowPositions,
    FinalizedSnapshots,
    WithdrawalQueue,
    StaticYield,
}

#[near]
pub struct Market {
    prefix: Vec<u8>,
    pub configuration: MarketConfiguration,
    /// Total amount of borrow asset that the market knows it currently holds.
    ///
    /// This is different from `active - borrowed` because some of the active
    /// amount might be compounded yield (deposited into the protocol without
    /// actually being transferred in).
    pub borrow_asset_balance: BorrowAssetAmount,
    /// Total amount of borrow asset earning interest in the market.
    pub borrow_asset_deposited_active: BorrowAssetAmount,
    /// Upcoming snapshot indices with amounts of borrow asset that will be activated.
    pub borrow_asset_deposited_incoming: Vec<IncomingDeposit>,
    pub borrow_asset_withdrawal_in_flight: BorrowAssetAmount,
    /// Sending borrow asset out, because if somebody sends the contract borrow asset, it's ok for the
    /// contract to attempt to fulfill withdrawal request, even if the market thinks it doesn't have
    /// enough to fulfill.
    pub borrow_asset_borrowed_in_flight: BorrowAssetAmount,
    /// Amount of borrow asset that has been withdrawn (is in use by) by borrowers.
    ///
    /// `borrow_asset_deposited_active - borrow_asset_borrowed - borrow_asset_borrowed_in_flight >= 0` should always be true.
    pub borrow_asset_borrowed: BorrowAssetAmount,
    /// Market-wide collateral asset deposit tracking.
    pub collateral_asset_deposited: CollateralAssetAmount,
    pub(crate) supply_positions: UnorderedMap<AccountId, SupplyPosition>,
    pub(crate) borrow_positions: UnorderedMap<AccountId, BorrowPosition>,
    pub current_time_chunk: TimeChunk,
    pub current_yield_distribution: BorrowAssetAmount,
    pub finalized_snapshots: ChunkedAppendOnlyList<Snapshot, 32>,
    pub withdrawal_queue: WithdrawalQueue,
    pub static_yield: LookupMap<AccountId, Accumulator<BorrowAsset>>,
    single_snapshot_maximum_interest_precomputed: Decimal,
}

impl Market {
    pub fn new(prefix: impl IntoStorageKey, configuration: MarketConfiguration) -> Self {
        if let Err(e) = configuration.validate() {
            crate::panic_with_message(&e.to_string());
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

        let first_snapshot = Snapshot::new(configuration.time_chunk_configuration.previous());
        let last_time_chunk = configuration.time_chunk_configuration.now();

        let single_snapshot_maximum_interest_precomputed =
            configuration.single_snapshot_maximum_interest();

        let mut self_ = Self {
            prefix: prefix.clone(),
            configuration,
            borrow_asset_balance: 0.into(),
            borrow_asset_deposited_active: 0.into(),
            borrow_asset_deposited_incoming: Vec::new(),
            borrow_asset_withdrawal_in_flight: 0.into(),
            borrow_asset_borrowed_in_flight: 0.into(),
            borrow_asset_borrowed: 0.into(),
            collateral_asset_deposited: 0.into(),
            supply_positions: UnorderedMap::new(key!(SupplyPositions)),
            borrow_positions: UnorderedMap::new(key!(BorrowPositions)),
            current_time_chunk: last_time_chunk,
            current_yield_distribution: 0.into(),
            finalized_snapshots: ChunkedAppendOnlyList::new(key!(FinalizedSnapshots)),
            withdrawal_queue: WithdrawalQueue::new(key!(WithdrawalQueue)),
            static_yield: LookupMap::new(key!(StaticYield)),
            single_snapshot_maximum_interest_precomputed,
        };

        self_.finalized_snapshots.push(first_snapshot);

        self_
    }

    pub fn borrowed(&self) -> BorrowAssetAmount {
        self.borrow_asset_borrowed + self.borrow_asset_borrowed_in_flight
    }

    pub fn total_incoming(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposited_incoming
            .iter()
            .fold(BorrowAssetAmount::zero(), |total_incoming, incoming| {
                total_incoming + incoming.amount
            })
    }

    pub fn incoming_at(&self, snapshot_index: u32) -> BorrowAssetAmount {
        self.borrow_asset_deposited_incoming
            .iter()
            .find_map(|incoming| {
                (incoming.activate_at_snapshot_index == snapshot_index).then_some(incoming.amount)
            })
            .unwrap_or(0.into())
    }

    pub fn get_last_finalized_snapshot(&self) -> &Snapshot {
        #[allow(clippy::unwrap_used, reason = "Snapshots are never empty")]
        self.finalized_snapshots
            .get(self.finalized_snapshots.len() - 1)
            .unwrap()
    }

    pub fn current_snapshot(&self) -> Snapshot {
        let current_snapshot_index = self.finalized_snapshots.len();
        let incoming = self.incoming_at(current_snapshot_index);

        let active = self.borrow_asset_deposited_active + incoming;

        let borrowed = self.borrowed();

        let interest_rate = self
            .configuration
            .borrow_interest_rate_strategy
            .at(usage_ratio(active, borrowed));

        Snapshot {
            time_chunk: self.current_time_chunk,
            end_timestamp_ms: env::block_timestamp_ms().into(),
            borrow_asset_deposited_active: active,
            borrow_asset_borrowed: borrowed,
            collateral_asset_deposited: self.collateral_asset_deposited,
            yield_distribution: self.current_yield_distribution,
            interest_rate,
        }
    }

    pub fn snapshot(&mut self) -> SnapshotProof {
        let now = self.configuration.time_chunk_configuration.now();

        // Do we need to finalize the current snapshot?
        if self.current_time_chunk == now {
            return SnapshotProof(());
        }

        let snapshot = self.current_snapshot();
        let current_snapshot_index = self.finalized_snapshots.len();

        // Emit event and push finalized snapshot
        MarketEvent::SnapshotFinalized {
            index: current_snapshot_index,
            snapshot: snapshot.clone(),
        }
        .emit();
        self.finalized_snapshots.push(snapshot);

        // We just pushed a snapshot
        let current_snapshot_index = current_snapshot_index + 1;

        // Activate incoming funds
        for i in 0..self.borrow_asset_deposited_incoming.len() {
            let incoming = &self.borrow_asset_deposited_incoming[i];
            if incoming.activate_at_snapshot_index == current_snapshot_index {
                self.borrow_asset_deposited_active += incoming.amount;
                self.borrow_asset_deposited_incoming.remove(i);
                break;
            }
        }

        // Reset for the new time chunk
        self.current_time_chunk = now;
        self.current_yield_distribution = 0.into();

        SnapshotProof(())
    }

    pub fn single_snapshot_fee(&self, amount: BorrowAssetAmount) -> Option<BorrowAssetAmount> {
        (u128::from(amount) * self.single_snapshot_maximum_interest_precomputed)
            .to_u128_ceil()
            .map(Into::into)
    }

    pub fn interest_rate(&self) -> Decimal {
        self.configuration
            .borrow_interest_rate_strategy
            .at(usage_ratio(
                self.borrow_asset_deposited_active,
                self.borrowed(),
            ))
    }

    pub fn get_borrow_asset_available_to_borrow(&self) -> BorrowAssetAmount {
        #[allow(
            clippy::unwrap_used,
            reason = "Factor is guaranteed to be <=1, so value must still fit in u128"
        )]
        let must_retain: BorrowAssetAmount = ((1u32
            - self.configuration.borrow_asset_maximum_usage_ratio)
            * Decimal::from(self.borrow_asset_deposited_active))
        .to_u128_ceil()
        .unwrap()
        .into();

        self.borrow_asset_deposited_active
            .saturating_sub(self.borrow_asset_borrowed)
            .saturating_sub(self.borrow_asset_borrowed_in_flight)
            .saturating_sub(must_retain)
    }

    pub fn iter_supply_positions(&self) -> impl Iterator<Item = (AccountId, SupplyPosition)> + '_ {
        self.supply_positions.iter()
    }

    pub fn supply_position_ref(&self, account_id: AccountId) -> Option<SupplyPositionRef<&Self>> {
        self.supply_positions
            .get(&account_id)
            .map(|position| SupplyPositionRef::new(self, account_id, position))
    }

    pub fn supply_position_guard(
        &mut self,
        _proof: SnapshotProof,
        account_id: AccountId,
    ) -> Option<SupplyPositionGuard> {
        self.supply_positions
            .get(&account_id)
            .map(|position| SupplyPositionGuard::new(self, account_id, position))
    }

    pub fn get_or_create_supply_position_guard(
        &mut self,
        _proof: SnapshotProof,
        account_id: AccountId,
    ) -> SupplyPositionGuard {
        let position = self
            .supply_positions
            .get(&account_id)
            .unwrap_or_else(|| SupplyPosition::new(self.finalized_snapshots.len()));

        SupplyPositionGuard::new(self, account_id, position)
    }

    pub fn cleanup_supply_position(&mut self, account_id: &AccountId) -> bool {
        self.supply_positions
            .get(account_id)
            .filter(SupplyPosition::can_be_removed)
            .and_then(|_| self.supply_positions.remove(account_id))
            .is_some()
    }

    pub fn iter_borrow_positions(&self) -> impl Iterator<Item = (AccountId, BorrowPosition)> + '_ {
        self.borrow_positions.iter()
    }

    pub fn borrow_position_ref(&self, account_id: AccountId) -> Option<BorrowPositionRef<&Self>> {
        self.borrow_positions
            .get(&account_id)
            .map(|position| BorrowPositionRef::new(self, account_id, position))
    }

    pub fn borrow_position_guard(
        &mut self,
        _proof: SnapshotProof,
        account_id: AccountId,
    ) -> Option<BorrowPositionGuard> {
        self.borrow_positions
            .get(&account_id)
            .map(|position| BorrowPositionGuard::new(self, account_id, position))
    }

    pub fn get_or_create_borrow_position_guard(
        &mut self,
        _proof: SnapshotProof,
        account_id: AccountId,
    ) -> BorrowPositionGuard {
        let position = self
            .borrow_positions
            .get(&account_id)
            .unwrap_or_else(|| BorrowPosition::new(self.finalized_snapshots.len()));

        BorrowPositionGuard::new(self, account_id, position)
    }

    pub fn cleanup_borrow_position(&mut self, account_id: &AccountId) -> bool {
        self.borrow_positions
            .get(account_id)
            .filter(|p| !p.exists())
            .and_then(|_| self.borrow_positions.remove(account_id))
            .is_some()
    }

    pub fn record_borrow_asset_protocol_yield(&mut self, amount: BorrowAssetAmount) {
        let mut yield_record = self
            .static_yield
            .get(&self.configuration.protocol_account_id)
            .unwrap_or_else(|| Accumulator::new(1));

        yield_record.add_once(amount);

        self.static_yield
            .insert(&self.configuration.protocol_account_id, &yield_record);
    }

    pub fn record_borrow_asset_yield_distribution(&mut self, amount: BorrowAssetAmount) {
        // Sanity.
        if amount.is_zero() {
            return;
        }

        self.current_yield_distribution += amount;
    }

    /// Accumulate static yield for an account.
    ///
    /// # Errors
    ///
    /// - When the account is not configured to earn static yield.
    pub fn accumulate_static_yield(
        &mut self,
        account_id: &AccountId,
        snapshot_limit: u32,
    ) -> Result<(), UnknownAccount> {
        let weight_numerator = *self
            .configuration
            .yield_weights
            .r#static
            .get(account_id)
            .ok_or(UnknownAccount)?;
        let weight_denominator = self.configuration.yield_weights.total_weight().get();
        let mut accumulator = self
            .static_yield
            .get(account_id)
            .unwrap_or_else(|| Accumulator::new(1));

        let mut next_snapshot_index = accumulator.get_next_snapshot_index();
        let mut accumulated = Decimal::ZERO;

        #[allow(clippy::unwrap_used, reason = "Guaranteed previous snapshot exists")]
        let mut prev_end_timestamp_ms = self
            .finalized_snapshots
            .get(next_snapshot_index.checked_sub(1).unwrap())
            .unwrap()
            .end_timestamp_ms
            .0;

        #[allow(
            clippy::cast_possible_truncation,
            reason = "Assume # of snapshots is never >u32::MAX"
        )]
        for (i, snapshot) in self
            .finalized_snapshots
            .iter()
            .enumerate()
            .skip(next_snapshot_index as usize)
            .take(snapshot_limit as usize)
        {
            let snapshot_duration_ms = snapshot.end_timestamp_ms.0 - prev_end_timestamp_ms;
            let interest_paid_by_borrowers = Decimal::from(snapshot.borrow_asset_borrowed)
                * snapshot.interest_rate
                * snapshot_duration_ms
                * YEAR_PER_MS;
            let other_yield = Decimal::from(snapshot.yield_distribution);
            accumulated +=
                (interest_paid_by_borrowers + other_yield) * weight_numerator / weight_denominator;

            next_snapshot_index = i as u32 + 1;
            prev_end_timestamp_ms = snapshot.end_timestamp_ms.0;
        }

        let accumulation_record = AccumulationRecord {
            // Accumulated amount is derived from real balances, so it should
            // never overflow underlying data type.
            #[allow(clippy::unwrap_used, reason = "Derived from real balances")]
            amount: accumulated.to_u128_floor().unwrap().into(),
            fraction_as_u128_dividend: accumulated.fractional_part_as_u128_dividend(),
            next_snapshot_index,
        };

        accumulator.accumulate(accumulation_record);

        self.static_yield.insert(account_id, &accumulator);

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("This account does not earn static yield")]
pub struct UnknownAccount;

fn usage_ratio(active: BorrowAssetAmount, borrowed: BorrowAssetAmount) -> Decimal {
    if active.is_zero() || borrowed.is_zero() {
        Decimal::ZERO
    } else if borrowed >= active {
        Decimal::ONE
    } else {
        Decimal::from(borrowed) / Decimal::from(active)
    }
}

#[allow(clippy::too_many_lines)]
#[cfg(test)]
mod tests {
    use near_sdk::{test_utils::*, testing_env};

    use crate::{
        asset::FungibleAsset,
        dec,
        fee::{Fee, TimeBasedFee},
        interest_rate_strategy::InterestRateStrategy,
        market::{PriceOracleConfiguration, YieldWeights},
        oracle::pyth::PriceIdentifier,
        price::PricePair,
        supply::WithdrawalAttempt,
        time_chunk::TimeChunkConfiguration,
    };

    use super::*;

    fn configuration() -> MarketConfiguration {
        MarketConfiguration {
            time_chunk_configuration: TimeChunkConfiguration::new(1),
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
            borrow_asset_maximum_usage_ratio: dec!("0.9"),
            borrow_origination_fee: Fee::Proportional(dec!("0.25")),
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

    fn price_pair(collateral: i64, borrow: i64) -> PricePair {
        PricePair::new(
            &crate::oracle::pyth::Price {
                price: collateral.into(),
                conf: 0.into(),
                expo: 24,
                publish_time: 10,
            },
            24,
            &crate::oracle::pyth::Price {
                price: borrow.into(),
                conf: 0.into(),
                expo: 24,
                publish_time: 10,
            },
            24,
        )
        .unwrap()
    }

    #[rstest::rstest]
    #[should_panic = "InsufficientBorrowAssetAvailable"]
    #[case(65_000_000)]
    #[case(63_500_000)]
    fn balance2(#[case] second_borrow_amount: u128) {
        let second_borrow_amount = BorrowAssetAmount::new(second_borrow_amount);

        let mut c = VMContextBuilder::new()
            .block_timestamp(1_000_000_000_000)
            .build();
        testing_env!(c.clone());

        let mut configuration = configuration();

        configuration.borrow_origination_fee = Fee::Flat(15_000_000.into());
        configuration.borrow_interest_rate_strategy = InterestRateStrategy::zero();
        configuration.borrow_asset_maximum_usage_ratio = Decimal::ONE;

        let supply_id: AccountId = "supply.near".parse().unwrap();
        let borrow_id: AccountId = "borrow.near".parse().unwrap();
        let borrow_2_id: AccountId = "borrow2.near".parse().unwrap();

        let mut market = Market::new(b"m", configuration.clone());

        let mut tick = |market: &mut Market| {
            c.block_timestamp += 1_000_000;
            testing_env!(c.clone());
            market.snapshot()
        };

        // Supply 100
        {
            let snapshot = tick(&mut market);
            let mut supply_position =
                market.get_or_create_supply_position_guard(snapshot, supply_id.clone());
            let yield_proof = supply_position.accumulate_yield();
            supply_position.record_deposit(
                yield_proof,
                100_000_000.into(),
                env::block_timestamp_ms(),
            );
        }
        assert_eq!(market.borrow_asset_balance, 100_000_000.into());
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            0.into() // still in incoming
        );

        // Collateralize 200
        {
            let snapshot = tick(&mut market);
            let mut borrow_position =
                market.get_or_create_borrow_position_guard(snapshot, borrow_id.clone());
            let proof = borrow_position.accumulate_interest();
            borrow_position.record_collateral_asset_deposit(proof, 200_000_000.into());
        }
        assert_eq!(market.borrow_asset_balance, 100_000_000.into());
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            100_000_000.into()
        );

        // Borrow 100 initial
        let initial = {
            let snapshot = tick(&mut market);
            let mut borrow_position = market
                .borrow_position_guard(snapshot, borrow_id.clone())
                .unwrap();
            let proof = borrow_position.accumulate_interest();
            borrow_position
                .record_borrow_initial(
                    snapshot,
                    proof,
                    100_000_000.into(),
                    &price_pair(1, 1),
                    env::block_timestamp_ms(),
                )
                .unwrap()
        };
        assert_eq!(market.borrow_asset_balance, 0.into());
        assert_eq!(market.get_borrow_asset_available_to_borrow(), 0.into());

        // Borrow final
        {
            let snapshot = tick(&mut market);
            let mut borrow_position = market
                .borrow_position_guard(snapshot, borrow_id.clone())
                .unwrap();
            let proof = borrow_position.accumulate_interest();
            borrow_position.record_borrow_final(
                snapshot,
                proof,
                &initial,
                true,
                env::block_timestamp_ms(),
            );
        }
        assert_eq!(market.borrow_asset_balance, 0.into());
        assert_eq!(market.get_borrow_asset_available_to_borrow(), 0.into());

        // Borrow repay 100% + fees
        let amount_repaid = {
            let snapshot = tick(&mut market);
            let mut borrow_position = market
                .borrow_position_guard(snapshot, borrow_id.clone())
                .unwrap();
            let proof = borrow_position.accumulate_interest();
            let liability = borrow_position.inner().get_total_borrow_asset_liability();
            assert_eq!(liability, 115_000_000.into());
            let remaining = borrow_position.record_repay(proof, liability);
            assert_eq!(remaining, 0.into());
            liability
        };
        assert_eq!(market.borrow_asset_balance, amount_repaid);
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            100_000_000.into()
        );

        // Supplier withdraws 50: initial
        let (withdrawal, yield_amount) = {
            let snapshot = tick(&mut market);
            let mut supply_position = market
                .supply_position_guard(snapshot, supply_id.clone())
                .unwrap();
            let yield_proof = supply_position.accumulate_yield();
            let yield_amount = supply_position.total_yield();
            supply_position.record_yield_compound(yield_proof, yield_amount);
            let yield_proof = supply_position.accumulate_yield();
            (
                supply_position.record_withdrawal_initial(
                    yield_proof,
                    50_000_000.into(),
                    env::block_timestamp_ms(),
                ),
                yield_amount,
            )
        };
        let WithdrawalAttempt::Full(withdrawal) = withdrawal else {
            panic!("Expected full withdrawal");
        };
        assert_eq!(
            market.borrow_asset_balance,
            amount_repaid - BorrowAssetAmount::new(50_000_000),
        );
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            BorrowAssetAmount::new(50_000_000) + yield_amount
        );

        // Supply withdrawal final
        {
            let snapshot = tick(&mut market);
            let mut supply_position = market
                .supply_position_guard(snapshot, supply_id.clone())
                .unwrap();
            supply_position.record_withdrawal_final(&withdrawal, true);
        }
        assert_eq!(
            market.borrow_asset_balance,
            amount_repaid - BorrowAssetAmount::new(50_000_000),
        );
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            BorrowAssetAmount::new(50_000_000) + yield_amount
        );

        // Collateralize2 200
        {
            let snapshot = tick(&mut market);
            let mut borrow_position =
                market.get_or_create_borrow_position_guard(snapshot, borrow_2_id.clone());
            let proof = borrow_position.accumulate_interest();
            borrow_position.record_collateral_asset_deposit(proof, 200_000_000.into());
        }
        assert_eq!(
            market.borrow_asset_balance,
            amount_repaid - BorrowAssetAmount::new(50_000_000),
        );
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            BorrowAssetAmount::new(50_000_000) + yield_amount
        );

        eprintln!(
            "Available: {}",
            market.get_borrow_asset_available_to_borrow()
        );
        eprintln!("Borrow asset balance: {}", market.borrow_asset_balance);
        eprintln!("Borrow asset borrowed: {}", market.borrow_asset_borrowed);
        eprintln!(
            "Borrow asset deposited active: {}",
            market.borrow_asset_deposited_active
        );

        // Borrow2 initial
        let initial = {
            let snapshot = tick(&mut market);
            let mut borrow_position = market
                .borrow_position_guard(snapshot, borrow_2_id.clone())
                .unwrap();
            let proof = borrow_position.accumulate_interest();
            borrow_position
                .record_borrow_initial(
                    snapshot,
                    proof,
                    second_borrow_amount,
                    &price_pair(1, 1),
                    env::block_timestamp_ms(),
                )
                .unwrap()
        };
        assert_eq!(
            market.borrow_asset_balance,
            amount_repaid - BorrowAssetAmount::new(50_000_000) - second_borrow_amount,
        );
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            BorrowAssetAmount::new(50_000_000) + yield_amount - second_borrow_amount,
        );

        eprintln!("Borrow2 final");
        eprintln!("{initial:?}");
        eprintln!("{}", market.borrow_asset_borrowed_in_flight);

        // Borrow2 final
        {
            let snapshot = tick(&mut market);
            let mut borrow_position = market
                .borrow_position_guard(snapshot, borrow_2_id.clone())
                .unwrap();
            let proof = borrow_position.accumulate_interest();
            borrow_position.record_borrow_final(
                snapshot,
                proof,
                &initial,
                true,
                env::block_timestamp_ms(),
            );
        }
        assert_eq!(
            market.borrow_asset_balance,
            amount_repaid - BorrowAssetAmount::new(50_000_000) - second_borrow_amount,
        );

        eprintln!(
            "Available: {}",
            market.get_borrow_asset_available_to_borrow()
        );
        eprintln!("Borrow asset balance: {}", market.borrow_asset_balance);
        eprintln!("Borrow asset borrowed: {}", market.borrow_asset_borrowed);
        eprintln!(
            "Borrow asset deposited active: {}",
            market.borrow_asset_deposited_active
        );
    }

    #[test]
    fn balance() {
        let mut c = VMContextBuilder::new()
            .block_timestamp(1_000_000_000_000)
            .build();
        testing_env!(c.clone());

        let configuration = configuration();

        let supply_id: AccountId = "supply.near".parse().unwrap();
        let borrow_id: AccountId = "borrow.near".parse().unwrap();

        let mut market = Market::new(b"m", configuration.clone());

        let mut tick = |market: &mut Market| {
            c.block_timestamp += 1_000_000;
            testing_env!(c.clone());
            market.snapshot()
        };

        // Supply
        {
            let snapshot = tick(&mut market);
            let mut supply_position =
                market.get_or_create_supply_position_guard(snapshot, supply_id.clone());
            let yield_proof = supply_position.accumulate_yield();
            supply_position.record_deposit(
                yield_proof,
                10_000_000.into(),
                env::block_timestamp_ms(),
            );
        }
        assert_eq!(market.borrow_asset_balance, 10_000_000.into());
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            0.into(),
            "still incoming, not yet active",
        );

        // Collateralize
        {
            let snapshot = tick(&mut market);
            let mut borrow_position =
                market.get_or_create_borrow_position_guard(snapshot, borrow_id.clone());
            let interest_proof = borrow_position.accumulate_interest();
            borrow_position.record_collateral_asset_deposit(interest_proof, 4_000_000.into());
        }
        assert_eq!(market.borrow_asset_balance, 10_000_000.into());
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            9_000_000.into()
        );

        // Borrow: initial
        let initial = {
            let snapshot = tick(&mut market);
            let mut borrow_position = market
                .borrow_position_guard(snapshot, borrow_id.clone())
                .unwrap();
            let interest_proof = borrow_position.accumulate_interest();
            borrow_position
                .record_borrow_initial(
                    snapshot,
                    interest_proof,
                    2_000_000.into(),
                    &price_pair(1, 1),
                    env::block_timestamp_ms(),
                )
                .unwrap()
        };
        eprintln!("Borrowed: {}", market.borrow_asset_borrowed);
        assert_eq!(market.borrow_asset_balance, 8_000_000.into());
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            7_000_000.into()
        );

        // Borrow: final
        {
            let snapshot = tick(&mut market);
            let mut borrow_position = market
                .borrow_position_guard(snapshot, borrow_id.clone())
                .unwrap();
            let interest_proof = borrow_position.accumulate_interest();
            borrow_position.record_borrow_final(
                snapshot,
                interest_proof,
                &initial,
                true,
                env::block_timestamp_ms(),
            );
        }
        assert_eq!(market.borrow_asset_borrowed, 2_000_000.into());
        assert_eq!(market.borrow_asset_balance, 8_000_000.into());
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            7_000_000.into()
        );

        // Repay half
        {
            let snapshot = tick(&mut market);
            let mut borrow_position = market
                .borrow_position_guard(snapshot, borrow_id.clone())
                .unwrap();
            let interest_proof = borrow_position.accumulate_interest();
            borrow_position.record_repay(interest_proof, 1_500_000.into());
        }
        assert_eq!(market.borrow_asset_borrowed, 1_000_000.into());
        eprintln!("Borrowed: {}", market.borrow_asset_borrowed);
        assert_eq!(market.borrow_asset_balance, 9_500_000.into());
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            8_000_000.into()
        );

        // Withdraw half of collateral: initial
        {
            let snapshot = tick(&mut market);
            let mut borrow_position = market
                .borrow_position_guard(snapshot, borrow_id.clone())
                .unwrap();
            let interest_proof = borrow_position.accumulate_interest();
            borrow_position
                .record_collateral_asset_withdrawal_initial(interest_proof, 2_000_000.into());
        }
        assert_eq!(market.borrow_asset_balance, 9_500_000.into());
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            8_000_000.into()
        );

        // Withdraw half of collateral: final
        {
            let snapshot = tick(&mut market);
            let mut borrow_position = market
                .borrow_position_guard(snapshot, borrow_id.clone())
                .unwrap();
            let interest_proof = borrow_position.accumulate_interest();
            borrow_position.record_collateral_asset_withdrawal_final(
                interest_proof,
                2_000_000.into(),
                true,
            );
        }
        assert_eq!(market.borrow_asset_balance, 9_500_000.into());
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            8_000_000.into()
        );

        // Liquidate the position
        {
            let snapshot = tick(&mut market);
            let mut borrow_position = market
                .borrow_position_guard(snapshot, borrow_id.clone())
                .unwrap();
            let interest_proof = borrow_position.accumulate_interest();
            let liquidation = borrow_position
                .record_liquidation(
                    interest_proof,
                    "liquidator.near".parse().unwrap(),
                    1_000_000.into(),
                    None,
                    &price_pair(1, 2), // should cause 100% of the position to be liquidated
                    env::block_timestamp_ms(),
                )
                .unwrap();
            assert_eq!(u128::from(liquidation.liquidated), 2_000_000);
        }
        assert_eq!(market.borrow_asset_balance, 10_500_000.into());
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            9_000_000.into()
        );

        // Supply yield compounding
        let expected_yield_amount: BorrowAssetAmount = (500_000 * 9 / 10).into();
        {
            let snapshot = tick(&mut market);
            let mut supply_position = market
                .supply_position_guard(snapshot, supply_id.clone())
                .unwrap();
            let proof = supply_position.accumulate_yield();
            let yield_amount = supply_position.total_yield();
            assert_eq!(yield_amount, expected_yield_amount);
            supply_position.record_yield_compound(proof, yield_amount);
        }
        assert_eq!(
            market.borrow_asset_balance,
            10_500_000.into(),
            "Yield compounding does not affect the market's recorded balance",
        );
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            BorrowAssetAmount::new(9_000_000), // should still be in incoming
        );

        tick(&mut market); // move incoming to active
        assert_eq!(market.borrow_asset_balance, 10_500_000.into());
        assert_eq!(market.borrow_asset_deposited_active, 10_450_000.into());
        assert_eq!(
            market.get_borrow_asset_available_to_borrow(),
            BorrowAssetAmount::new(10_450_000 * 9 / 10),
        );

        // Withdraw supply: initial
        let initial = {
            let snapshot = tick(&mut market);
            let mut supply_position = market
                .supply_position_guard(snapshot, supply_id.clone())
                .unwrap();
            let proof = supply_position.accumulate_yield();
            supply_position.record_withdrawal_initial(
                proof,
                10_450_000.into(),
                env::block_timestamp_ms(),
            )
        };
        let initial = match initial {
            WithdrawalAttempt::Full(initial) => initial,
            a => {
                panic!("Should be full withdrawal: {a:?}");
            }
        };
        assert_eq!(market.borrow_asset_balance, 50_000.into());
        assert_eq!(market.get_borrow_asset_available_to_borrow(), 0.into());

        // Withdraw supply: final
        {
            let snapshot = tick(&mut market);
            let mut supply_position = market
                .supply_position_guard(snapshot, supply_id.clone())
                .unwrap();
            supply_position.record_withdrawal_final(&initial, true);
        }
        assert_eq!(market.borrow_asset_balance, 50_000.into());
        assert_eq!(market.get_borrow_asset_available_to_borrow(), 0.into());
    }
}
