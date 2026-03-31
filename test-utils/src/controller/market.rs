use std::{collections::HashMap, ops::Deref};

use near_sdk::{
    json_types::U128,
    serde_json::{self, json},
    AccountId, AccountIdRef, NearToken,
};
use near_workspaces::{
    network::Sandbox, result::ExecutionSuccess, types::SecretKey, Account, Contract, Worker,
};
use templar_common::{
    accumulator::Accumulator,
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus},
    market::{DepositMsg, HarvestYieldMode, LiquidateMsg, MarketConfiguration, RepayAccountMsg},
    number::Decimal,
    oracle::pyth::{self, OracleResponse},
    price::Convert,
    snapshot::Snapshot,
    supply::SupplyPosition,
    withdrawal_queue::{
        WithdrawalQueueExecutionResult, WithdrawalQueueStatus, WithdrawalRequestStatus,
    },
};
use tokio::sync::OnceCell;

use crate::{
    controller::storage_management::StorageManagementController, define, get_contract, to_price,
};

use super::{mock_oracle::MockOracleController, token::TokenController, ContractController};

#[derive(Clone)]
pub struct MarketController {
    pub(crate) contract: Contract,
}

impl ContractController for MarketController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl StorageManagementController for MarketController {}

impl MarketController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        WASM.get_or_init(|| get_contract("templar_market_contract", "contract/market"))
            .await
    }

    pub fn attach(worker: &Worker<Sandbox>, market_id: AccountId) -> Self {
        Self {
            contract: contract_with_dummy_sk(worker, market_id),
        }
    }

    pub async fn deploy(account: Account, configuration: &MarketConfiguration) -> Self {
        let wasm = Self::wasm().await;
        let contract = account.deploy(wasm).await.unwrap().unwrap();

        let init_call = contract
            .call("new")
            .args_json(json!({
                "configuration": configuration,
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();

        eprintln!("Init call logs");
        eprintln!("--------------");
        for log in init_call.logs() {
            eprintln!("\t{log}");
        }
        eprintln!("--------------");

        Self { contract }
    }

    define! {
        #[view] pub fn get_configuration() -> MarketConfiguration;
        #[view] pub fn get_finalized_snapshots_len() -> u32;
        #[view] pub fn list_finalized_snapshots(offset: Option<u32>, count: Option<u32>) -> Vec<Snapshot>;
        #[view] pub fn get_current_snapshot() -> Snapshot;
        #[view] pub fn list_supply_positions(offset: Option<u32>, count: Option<u32>) -> HashMap<AccountId, SupplyPosition>;
        #[view] pub fn get_supply_position(account_id: &AccountId) -> Option<SupplyPosition>;
        #[view] pub fn get_supply_position_pending_yield(account_id: &AccountId, snapshot_limit: Option<u32>) -> Option<BorrowAssetAmount>;
        #[view] pub fn list_borrow_positions(offset: Option<u32>, count: Option<u32>) -> HashMap<AccountId, BorrowPosition>;
        #[view] pub fn get_borrow_position(account_id: &AccountId) -> Option<BorrowPosition>;
        #[view] pub fn get_borrow_position_pending_interest(account_id: &AccountId, snapshot_limit: Option<u32>) -> Option<BorrowAssetAmount>;
        #[view] pub fn get_borrow_status(account_id: &AccountId, oracle_response: OracleResponse) -> Option<BorrowStatus>;
        #[view] pub fn get_static_yield(account_id: &AccountId) -> Option<Accumulator<BorrowAsset>>;
        #[view] pub fn get_supply_withdrawal_request_status(account_id: &AccountId) -> Option<WithdrawalRequestStatus>;
        #[view] pub fn get_supply_withdrawal_queue_status() -> WithdrawalQueueStatus;
        #[view] pub fn get_last_yield_rate() -> Decimal;

        #[call(exec, tgas(300))]
        pub fn borrow(amount: U128);
        #[call(exec, tgas(300))]
        pub fn apply_interest(account_id: Option<&AccountId>, snapshot_limit: Option<u32>);
        #[call(tgas(300))]
        pub fn harvest_yield(account_id: Option<&AccountId>, mode: Option<HarvestYieldMode>) -> BorrowAssetAmount;
        #[call(exec, tgas(300))]
        pub fn harvest_yield_exec["harvest_yield"](account_id: Option<&AccountId>, mode: Option<HarvestYieldMode>) -> BorrowAssetAmount;
        #[call(exec, tgas(300))]
        pub fn accumulate_static_yield(account_id: Option<AccountId>, snapshot_limit: Option<u32>);
        #[call(exec, tgas(20))]
        pub fn withdraw_static_yield(amount: Option<BorrowAssetAmount>);
        #[call(exec, tgas(42))]
        pub fn withdraw_collateral(amount: CollateralAssetAmount);
        #[call(exec)]
        pub fn create_supply_withdrawal_request(amount: BorrowAssetAmount);
        #[call(tgas(300))]
        pub fn execute_next_supply_withdrawal_request(batch_limit: Option<u32>) -> WithdrawalQueueExecutionResult;
        #[call(exec, tgas(300))]
        pub fn execute_next_supply_withdrawal_request_exec["execute_next_supply_withdrawal_request"](batch_limit: Option<u32>);
    }

    #[allow(unused)] // This is useful for debugging tests
    pub async fn print_snapshots(&self) {
        let snapshots = self.list_finalized_snapshots(None, None).await;

        eprintln!("Market snapshots:");
        for (i, snapshot) in snapshots.iter().enumerate() {
            eprintln!("\t{i}: {}", snapshot.time_chunk.0 .0);
            eprintln!("\t\tTimestamp:\t{}", snapshot.end_timestamp_ms.0);
            eprintln!(
                "\t\tDeposited (active):\t{}",
                snapshot.borrow_asset_deposited_active,
            );
            eprintln!("\t\tBorrowed:\t{}", snapshot.borrow_asset_borrowed);
            eprintln!("\t\tDistribution:\t{}", snapshot.yield_distribution);
        }
    }
}

#[derive(Clone)]
pub struct UnifiedMarketController {
    pub market: MarketController,
    pub configuration: MarketConfiguration,
    pub price_oracle: MockOracleController,
    pub borrow_asset: TokenController,
    pub collateral_asset: TokenController,
}

impl Deref for UnifiedMarketController {
    type Target = MarketController;

    fn deref(&self) -> &Self::Target {
        &self.market
    }
}

fn contract_with_dummy_sk(worker: &Worker<Sandbox>, account_id: AccountId) -> Contract {
    let dummy_key = SecretKey::from_seed(near_workspaces::types::KeyType::ED25519, "");

    Contract::from_secret_key(account_id, dummy_key.clone(), worker)
}

impl UnifiedMarketController {
    pub async fn attach(worker: &Worker<Sandbox>, market_id: AccountId) -> Self {
        let market = MarketController {
            contract: contract_with_dummy_sk(worker, market_id),
        };

        let configuration = market.get_configuration().await;

        let price_oracle = MockOracleController {
            contract: contract_with_dummy_sk(
                worker,
                configuration.price_oracle_configuration.account_id.clone(),
            ),
        };

        let borrow_asset =
            if let Some(account_id) = configuration.borrow_asset.clone().into_nep141() {
                TokenController::ft(contract_with_dummy_sk(worker, account_id))
            } else if let Some((account_id, token_id)) =
                configuration.borrow_asset.clone().into_nep245()
            {
                TokenController::mt(contract_with_dummy_sk(worker, account_id), token_id)
            } else {
                unreachable!()
            };

        let collateral_asset =
            if let Some(account_id) = configuration.collateral_asset.clone().into_nep141() {
                TokenController::ft(contract_with_dummy_sk(worker, account_id))
            } else if let Some((account_id, token_id)) =
                configuration.collateral_asset.clone().into_nep245()
            {
                TokenController::mt(contract_with_dummy_sk(worker, account_id), token_id)
            } else {
                unreachable!()
            };

        Self {
            market,
            configuration,
            price_oracle,
            borrow_asset,
            collateral_asset,
        }
    }

    pub fn new(
        market: MarketController,
        configuration: MarketConfiguration,
        price_oracle: MockOracleController,
        borrow_asset: TokenController,
        collateral_asset: TokenController,
    ) -> Self {
        Self {
            market,
            configuration,
            price_oracle,
            borrow_asset,
            collateral_asset,
        }
    }

    pub async fn init_account(&self, account: &Account) {
        const AMOUNT: U128 = U128(100_000_000);

        self.storage_deposits(account).await;
        self.collateral_asset.mint(account, AMOUNT).await;
        self.borrow_asset.mint(account, AMOUNT).await;
    }

    pub async fn storage_deposits(&self, account: &Account) {
        eprintln!("Performing storage deposits for {}...", account.id());
        let bounds = self.market.storage_balance_bounds().await;
        self.market.storage_deposit(account, bounds.min).await;
        if let TokenController::Ft { ref controller } = self.borrow_asset {
            controller
                .storage_deposit(account, NearToken::from_near(1).saturating_div(100))
                .await;
        }
        if let TokenController::Ft { ref controller } = self.collateral_asset {
            controller
                .storage_deposit(account, NearToken::from_near(1).saturating_div(100))
                .await;
        }
    }

    pub async fn set_collateral_asset_price(&self, price: f64) -> ExecutionSuccess {
        eprintln!("Setting collateral asset price...",);
        self.price_oracle
            .set_pyth_price(
                self.price_oracle.contract().as_account(),
                self.configuration
                    .price_oracle_configuration
                    .collateral_asset_price_id,
                Some(to_price(price)),
            )
            .await
    }

    pub async fn set_collateral_asset_price_exact(
        &self,
        price: Option<pyth::Price>,
    ) -> ExecutionSuccess {
        eprintln!("Setting collateral asset price...",);
        self.price_oracle
            .set_pyth_price(
                self.price_oracle.contract().as_account(),
                self.configuration
                    .price_oracle_configuration
                    .collateral_asset_price_id,
                price,
            )
            .await
    }

    pub async fn set_borrow_asset_price(&self, price: f64) -> ExecutionSuccess {
        eprintln!("Setting borrow asset price...",);
        self.price_oracle
            .set_pyth_price(
                self.price_oracle.contract().as_account(),
                self.configuration
                    .price_oracle_configuration
                    .borrow_asset_price_id,
                Some(to_price(price)),
            )
            .await
    }

    pub async fn set_borrow_asset_price_exact(
        &self,
        price: Option<pyth::Price>,
    ) -> ExecutionSuccess {
        eprintln!("Setting borrow asset price...",);
        self.price_oracle
            .set_pyth_price(
                self.price_oracle.contract().as_account(),
                self.configuration
                    .price_oracle_configuration
                    .borrow_asset_price_id,
                price,
            )
            .await
    }

    pub async fn get_prices(&self) -> OracleResponse {
        self.price_oracle
            .list_ema_prices_no_older_than(
                [
                    self.configuration
                        .price_oracle_configuration
                        .borrow_asset_price_id,
                    self.configuration
                        .price_oracle_configuration
                        .collateral_asset_price_id,
                ],
                self.configuration
                    .price_oracle_configuration
                    .price_maximum_age_s,
            )
            .await
    }

    pub async fn supply(&self, supply_user: &Account, amount: u128) -> ExecutionSuccess {
        eprintln!(
            "{} transferring {amount} tokens for supply...",
            supply_user.id()
        );
        self.borrow_asset
            .transfer_call(
                supply_user,
                self.market.contract().id(),
                amount,
                serde_json::to_string(&DepositMsg::Supply).unwrap(),
            )
            .await
    }

    pub async fn supply_and_harvest_until_activation(
        &self,
        supply_user: &Account,
        amount: u128,
    ) -> ExecutionSuccess {
        let e = self.supply(supply_user, amount).await;
        while !self
            .get_supply_position(supply_user.id())
            .await
            .unwrap()
            .get_deposit()
            .incoming
            .is_empty()
        {
            self.harvest_yield(supply_user, None, None).await;
        }
        e
    }

    pub async fn collateralize(&self, borrow_user: &Account, amount: u128) -> ExecutionSuccess {
        eprintln!(
            "{} transferring {amount} tokens for collateral...",
            borrow_user.id(),
        );
        self.collateral_asset
            .transfer_call(
                borrow_user,
                self.market.contract().id(),
                amount,
                serde_json::to_string(&DepositMsg::Collateralize).unwrap(),
            )
            .await
    }

    pub async fn repay(
        &self,
        borrow_user: &Account,
        account_id: Option<&AccountIdRef>,
        amount: u128,
    ) -> ExecutionSuccess {
        eprintln!("{} repaying {amount} tokens...", borrow_user.id());
        let msg = account_id.map_or(DepositMsg::Repay, |account_id| {
            DepositMsg::RepayAccount(RepayAccountMsg {
                account_id: account_id.to_owned(),
            })
        });
        self.borrow_asset
            .transfer_call(
                borrow_user,
                self.market.contract().id(),
                amount,
                serde_json::to_string(&msg).unwrap(),
            )
            .await
    }

    pub async fn liquidate(
        &self,
        liquidator_user: &Account,
        account_id: &AccountId,
        expect_receive_collateral: CollateralAssetAmount,
        borrow_asset_amount: BorrowAssetAmount,
    ) -> ExecutionSuccess {
        eprintln!(
            "{} executing liquidation against {} for {}...",
            liquidator_user.id(),
            account_id,
            borrow_asset_amount,
        );
        self.borrow_asset
            .transfer_call(
                liquidator_user,
                self.market.contract().id(),
                borrow_asset_amount,
                serde_json::to_string(&DepositMsg::Liquidate(LiquidateMsg {
                    account_id: account_id.clone(),
                    amount: Some(expect_receive_collateral),
                }))
                .unwrap(),
            )
            .await
    }

    pub async fn liquidatable_collateral_fmv(
        &self,
        account_id: &AccountId,
    ) -> (CollateralAssetAmount, BorrowAssetAmount) {
        let price_pair = self
            .configuration
            .price_oracle_configuration
            .create_price_pair(&self.get_prices().await)
            .unwrap();
        let borrow_position = self.get_borrow_position(account_id).await.unwrap();
        let liquidate_collateral = borrow_position.liquidatable_collateral(
            &price_pair,
            self.configuration.borrow_mcr_maintenance,
            self.configuration.liquidation_maximum_spread,
        );
        let pay_for_collateral = price_pair
            .convert(liquidate_collateral)
            .to_u128_ceil()
            .unwrap()
            .max(1)
            .into();
        (liquidate_collateral, pay_for_collateral)
    }

    pub async fn liquidatable_collateral_with_spread(
        &self,
        account_id: &AccountId,
    ) -> (CollateralAssetAmount, BorrowAssetAmount) {
        let price_pair = self
            .configuration
            .price_oracle_configuration
            .create_price_pair(&self.get_prices().await)
            .unwrap();
        let borrow_position = self.get_borrow_position(account_id).await.unwrap();
        let liquidate_collateral = borrow_position.liquidatable_collateral(
            &price_pair,
            self.configuration.borrow_mcr_maintenance,
            self.configuration.liquidation_maximum_spread,
        );
        let pay_for_collateral = (price_pair.convert(liquidate_collateral)
            * (Decimal::ONE - self.configuration.liquidation_maximum_spread))
            .to_u128_ceil()
            .unwrap()
            .max(1)
            .into();
        (liquidate_collateral, pay_for_collateral)
    }
}
