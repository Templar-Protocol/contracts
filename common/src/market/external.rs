use near_sdk::{AccountId, Promise, PromiseOrValue};

use crate::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus},
    number::Decimal,
    oracle::pyth::OracleResponse,
    snapshot::Snapshot,
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};

use super::{BorrowAssetMetrics, MarketConfiguration};

#[near_sdk::ext_contract(ext_market)]
pub trait MarketExternalInterface {
    // ========================
    // MARKET GENERAL FUNCTIONS
    // ========================

    fn get_configuration(&self) -> MarketConfiguration;
    fn get_snapshots(&self, offset: Option<u32>, count: Option<u32>) -> Vec<&Snapshot>;
    /// Takes current balance as an argument so that it can be called as view.
    /// `borrow_asset_balance` should be retrieved from the borrow asset
    /// contract specified in the market configuration.
    fn get_borrow_asset_metrics(&self) -> BorrowAssetMetrics;

    // TODO: Decide how to work with remote balances:
    // Option 1:
    // Balance oracle calls a function directly.
    // Option 2: Balance oracle creates/maintains separate NEP-141-ish contracts that track remote
    // balances.

    fn list_borrows(&self, offset: Option<u32>, count: Option<u32>) -> Vec<AccountId>;
    fn list_supplys(&self, offset: Option<u32>, count: Option<u32>) -> Vec<AccountId>;

    // ==================
    // BORROW FUNCTIONS
    // ==================

    // ft_on_receive :: where msg = Collateralize
    fn collateralize_native(&mut self);
    // ft_on_receive :: where msg = Repay
    fn repay_native(&mut self) -> PromiseOrValue<()>;

    /// This function may report fees slightly inaccurately. This is because
    /// the function has to estimate what fees will be applied between the last
    /// market snapshot and the (present) time when the function was called.
    fn get_borrow_position(&self, account_id: AccountId) -> Option<BorrowPosition>;
    /// This is just a read-only function, so we don't care about validating
    /// the provided price data.
    fn get_borrow_status(
        &self,
        account_id: AccountId,
        oracle_response: OracleResponse,
    ) -> Option<BorrowStatus>;

    fn borrow(&mut self, amount: BorrowAssetAmount) -> Promise;
    fn withdraw_collateral(&mut self, amount: CollateralAssetAmount) -> Promise;

    /// Applies interest to the predecessor's borrow record.
    /// Not likely to be used in real life, since there it does not affect the
    /// final interest calculation, and rounds fractional interest UP.
    fn apply_interest(&mut self);

    fn get_last_interest_rate(&self) -> Decimal;

    // ================
    // SUPPLY FUNCTIONS
    // ================
    // We assume that all borrowed assets are NEAR-local. That is to say, we
    // don't yet support supplying of remote assets.

    // ft_on_receive :: where msg = Supply
    fn supply_native(&mut self);

    fn get_supply_position(&self, account_id: AccountId) -> Option<SupplyPosition>;

    fn create_supply_withdrawal_request(&mut self, amount: BorrowAssetAmount);
    fn cancel_supply_withdrawal_request(&mut self);
    fn execute_next_supply_withdrawal_request(&mut self) -> PromiseOrValue<()>;
    fn get_supply_withdrawal_request_status(
        &self,
        account_id: AccountId,
    ) -> Option<WithdrawalRequestStatus>;
    fn get_supply_withdrawal_queue_status(&self) -> WithdrawalQueueStatus;

    /// Claim any distributed yield to the supply record.
    /// If `compounding` is `true`, the all of the yield (including any
    /// harvested in previous, non-compounding `harvest_yield` calls) is
    /// deposited to the supply record, so it will contribute to future yield
    /// calculations.
    fn harvest_yield(&mut self, compounding: Option<bool>);

    /// This value is an *expected average over time*.
    /// Supply positions actually earn all of their yield the instant it is
    /// distributed.
    fn get_last_yield_rate(&self) -> Decimal;

    // =====================
    // LIQUIDATION FUNCTIONS
    // =====================

    // ft_on_receive :: where msg = Liquidate { account_id }
    fn liquidate_native(&mut self, account_id: AccountId) -> Promise;

    // =================
    // YIELD FUNCTIONS
    // =================
    fn get_static_yield(&self, account_id: AccountId) -> Option<StaticYieldRecord>;
    fn withdraw_supply_yield(&mut self, amount: Option<BorrowAssetAmount>) -> Promise;
    fn withdraw_static_yield(
        &mut self,
        borrow_asset_amount: Option<BorrowAssetAmount>,
        collateral_asset_amount: Option<CollateralAssetAmount>,
    ) -> Promise;
}
