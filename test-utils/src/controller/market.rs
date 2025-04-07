use near_sdk::{
    json_types::U128,
    serde_json::{self, json},
    AccountId, Gas, NearToken,
};
use near_workspaces::{result::ExecutionSuccess, Account, Contract};
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus},
    market::{HarvestYieldMode, LiquidateMsg, MarketConfiguration, Nep141MarketDepositMessage},
    number::Decimal,
    oracle::pyth::OracleResponse,
    snapshot::Snapshot,
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};
use tokio::sync::OnceCell;

use crate::{
    controller::storage_management::StorageManagementController, define, get_contract, to_price,
};

use super::{ft::FtController, oracle::OracleController, ContractController};

pub struct MarketController {
    pub contract: Contract,
    pub configuration: MarketConfiguration,
    pub balance_oracle: OracleController,
    pub borrow_asset: FtController,
    pub collateral_asset: FtController,
}

impl ContractController for MarketController {
    fn contract(&self) -> &Contract {
        &self.contract
    }
}

impl StorageManagementController for MarketController {}

impl MarketController {
    pub async fn setup(
        account: Account,
        configuration: MarketConfiguration,
        balance_oracle: OracleController,
        borrow_asset: FtController,
        collateral_asset: FtController,
    ) -> Self {
        static WASM_MARKET: OnceCell<Vec<u8>> = OnceCell::const_new();

        let wasm = WASM_MARKET
            .get_or_init(|| get_contract("templar_market_contract", "contract/market"))
            .await;

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

        Self {
            contract,
            configuration,
            balance_oracle,
            borrow_asset,
            collateral_asset,
        }
    }

    pub async fn storage_deposits(&self, account: &Account) {
        eprintln!("Performing storage deposits for {}...", account.id());
        let bounds = self.storage_balance_bounds().await;
        self.storage_deposit(account, bounds.min).await;
        self.borrow_asset
            .storage_deposit(account, NearToken::from_near(1))
            .await;
        self.collateral_asset
            .storage_deposit(account, NearToken::from_near(1))
            .await;
    }

    define! {
        #[view] pub fn get_configuration() -> MarketConfiguration;
        #[view] pub fn get_finalized_snapshots_len() -> u32;
        #[view] pub fn list_finalized_snapshots(offset: Option<u32>, count: Option<u32>) -> Vec<Snapshot>;
        #[view] pub fn get_supply_position(account_id: &AccountId) -> Option<SupplyPosition>;
        #[view] pub fn get_borrow_position(account_id: &AccountId) -> Option<BorrowPosition>;
        #[view] pub fn get_borrow_status(account_id: &AccountId, oracle_response: OracleResponse) -> Option<BorrowStatus>;
        #[view] pub fn get_static_yield(account_id: &AccountId) -> Option<StaticYieldRecord>;
        #[view] pub fn get_supply_withdrawal_request_status(account_id: &AccountId) -> Option<WithdrawalRequestStatus>;
        #[view] pub fn get_supply_withdrawal_queue_status() -> WithdrawalQueueStatus;
        #[view] pub fn get_last_yield_rate() -> Decimal;

        #[call(tgas(300))]
        pub fn borrow(amount: U128);
        #[call(tgas(300))]
        pub fn apply_interest(snapshot_limit: Option<u32>);
        #[call(tgas(300))]
        pub fn harvest_yield(mode: Option<HarvestYieldMode>) -> BorrowAssetAmount;
        #[call(tgas(20))]
        pub fn withdraw_static_yield(borrow_asset_amount: Option<BorrowAssetAmount>, collateral_asset_amount: Option<CollateralAssetAmount>);
        #[call(tgas(20))]
        pub fn withdraw_collateral(amount: CollateralAssetAmount);
        #[call]
        pub fn create_supply_withdrawal_request(amount: BorrowAssetAmount);
        #[call(tgas(20))]
        pub fn execute_next_supply_withdrawal_request();
    }

    pub async fn set_collateral_asset_price(&self, price: f64) -> ExecutionSuccess {
        eprintln!("Setting collateral asset price...",);
        self.balance_oracle
            .set_price(
                self.balance_oracle.contract().as_account(),
                self.configuration.balance_oracle.collateral_asset_price_id,
                to_price(price),
            )
            .await
    }

    pub async fn set_borrow_asset_price(&self, price: f64) -> ExecutionSuccess {
        eprintln!("Setting borrow asset price...",);
        self.balance_oracle
            .set_price(
                self.balance_oracle.contract().as_account(),
                self.configuration.balance_oracle.borrow_asset_price_id,
                to_price(price),
            )
            .await
    }

    pub async fn get_prices(&self) -> OracleResponse {
        self.balance_oracle
            .list_ema_prices_no_older_than(
                &[
                    self.configuration.balance_oracle.borrow_asset_price_id,
                    self.configuration.balance_oracle.collateral_asset_price_id,
                ],
                self.configuration.balance_oracle.price_maximum_age_s,
            )
            .await
    }

    pub async fn supply(&self, supply_user: &Account, amount: u128) -> ExecutionSuccess {
        eprintln!(
            "{} transferring {amount} tokens for supply...",
            supply_user.id()
        );
        self.borrow_asset
            .ft_transfer_call(
                supply_user,
                self.contract.id(),
                amount.into(),
                &serde_json::to_string(&Nep141MarketDepositMessage::Supply).unwrap(),
            )
            .await
    }

    pub async fn collateralize(&self, borrow_user: &Account, amount: u128) {
        eprintln!(
            "{} transferring {amount} tokens for collateral...",
            borrow_user.id(),
        );
        self.collateral_asset
            .ft_transfer_call(
                borrow_user,
                self.contract.id(),
                amount.into(),
                &serde_json::to_string(&Nep141MarketDepositMessage::Collateralize).unwrap(),
            )
            .await;
    }

    pub async fn repay(&self, borrow_user: &Account, amount: u128) -> ExecutionSuccess {
        eprintln!("{} repaying {amount} tokens...", borrow_user.id());
        self.borrow_asset
            .ft_transfer_call(
                borrow_user,
                self.contract.id(),
                amount.into(),
                &serde_json::to_string(&Nep141MarketDepositMessage::Repay).unwrap(),
            )
            .await
    }

    pub async fn harvest_yield_execution(
        &self,
        supply_user: &Account,
        mode: Option<HarvestYieldMode>,
    ) -> ExecutionSuccess {
        eprintln!("{} harvesting yield...", supply_user.id());
        self.call_exec(
            supply_user,
            "harvest_yield",
            json!({ "mode": mode }),
            NearToken::from_near(0),
            Gas::from_tgas(300),
        )
        .await
    }

    pub async fn liquidate(
        &self,
        liquidator_user: &Account,
        account_id: &AccountId,
        borrow_asset_amount: u128,
    ) -> ExecutionSuccess {
        eprintln!(
            "{} executing liquidation against {} for {}...",
            liquidator_user.id(),
            account_id,
            borrow_asset_amount,
        );
        self.borrow_asset
            .ft_transfer_call(
                liquidator_user,
                self.contract.id(),
                borrow_asset_amount.into(),
                &serde_json::to_string(&Nep141MarketDepositMessage::Liquidate(LiquidateMsg {
                    account_id: account_id.clone(),
                }))
                .unwrap(),
            )
            .await
    }

    #[allow(unused)] // This is useful for debugging tests
    pub async fn print_snapshots(&self) {
        let snapshots = self.list_finalized_snapshots(None, None).await;

        eprintln!("Market snapshots:");
        for (i, snapshot) in snapshots.iter().enumerate() {
            eprintln!("\t{i}: {}", snapshot.time_chunk.0 .0);
            eprintln!("\t\tTimestamp:\t{}", snapshot.end_timestamp_ms.0);
            eprintln!("\t\tDeposited:\t{}", snapshot.deposited);
            eprintln!("\t\tBorrowed:\t{}", snapshot.borrowed);
            eprintln!("\t\tDistribution:\t{}", snapshot.yield_distribution);
        }
    }
}
