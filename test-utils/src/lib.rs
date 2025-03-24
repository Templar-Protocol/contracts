use std::{path::Path, str::FromStr};

use near_sdk::{
    json_types::{I64, U128, U64},
    serde_json::{self, json},
    AccountId, NearToken,
};
use near_workspaces::{
    network::Sandbox, prelude::*, result::ExecutionSuccess, Account, Contract, DevNetwork, Worker,
};
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount, FungibleAsset},
    borrow::{BorrowPosition, BorrowStatus},
    dec,
    fee::{Fee, TimeBasedFee},
    interest_rate_strategy::InterestRateStrategy,
    market::{
        BalanceOracleConfiguration, LiquidateMsg, MarketConfiguration, Nep141MarketDepositMessage,
        YieldWeights,
    },
    number::Decimal,
    oracle::pyth::{self, OracleResponse, PriceIdentifier},
    snapshot::Snapshot,
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};
use tokio::sync::OnceCell;

pub fn to_price(price: f64) -> pyth::Price {
    pyth::Price {
        price: I64((price * 10000.0) as i64),
        conf: U64(0),
        expo: -4,
        publish_time: 0,
    }
}

pub struct TestController {
    pub worker: Worker<Sandbox>,
    pub contract: Contract,
    pub config: MarketConfiguration,
    pub balance_oracle: Contract,
    pub borrow_asset: Contract,
    pub collateral_asset: Contract,
}

impl TestController {
    pub async fn storage_deposits(&self, account: &Account) {
        eprintln!("Performing storage deposits for {}...", account.id());
        account
            .call(self.borrow_asset.id(), "storage_deposit")
            .args_json(json!({}))
            .deposit(NearToken::from_near(1))
            .transact()
            .await
            .unwrap()
            .unwrap();
        account
            .call(self.collateral_asset.id(), "storage_deposit")
            .args_json(json!({}))
            .deposit(NearToken::from_near(1))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn get_configuration(&self) -> MarketConfiguration {
        self.contract
            .view("get_configuration")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<MarketConfiguration>()
            .unwrap()
    }

    pub async fn set_collateral_asset_price(&self, price: f64) -> ExecutionSuccess {
        eprintln!("Setting collateral asset price...",);
        self.balance_oracle
            .call("set_price")
            .args_json(json!({
                "price_identifier": self.config.balance_oracle.collateral_asset_price_id,
                "price": to_price(price),
            }))
            .transact()
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn set_borrow_asset_price(&self, price: f64) -> ExecutionSuccess {
        eprintln!("Setting borrow asset price...",);
        self.balance_oracle
            .call("set_price")
            .args_json(json!({
                "price_identifier": self.config.balance_oracle.borrow_asset_price_id,
                "price": to_price(price),
            }))
            .transact()
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn get_prices(&self) -> OracleResponse {
        self.balance_oracle
            .view("list_ema_prices_no_older_than")
            .args_json(json!({ "price_ids": [
                self.config.balance_oracle.borrow_asset_price_id,
                self.config.balance_oracle.collateral_asset_price_id,
            ], "age": self.config.balance_oracle.price_maximum_age_s }))
            .await
            .unwrap()
            .json::<OracleResponse>()
            .unwrap()
    }

    pub async fn supply(&self, supply_user: &Account, amount: u128) -> ExecutionSuccess {
        eprintln!(
            "{} transferring {amount} tokens for supply...",
            supply_user.id()
        );
        self.borrow_asset_transfer_call(
            supply_user,
            self.contract.id(),
            amount,
            &serde_json::to_string(&Nep141MarketDepositMessage::Supply).unwrap(),
        )
        .await
    }

    pub async fn get_supply_position(&self, account_id: &AccountId) -> Option<SupplyPosition> {
        self.contract
            .view("get_supply_position")
            .args_json(json!({
                "account_id": account_id,
            }))
            .await
            .unwrap()
            .json::<Option<SupplyPosition>>()
            .unwrap()
    }

    pub async fn list_supplys(&self) -> Vec<AccountId> {
        self.contract
            .view("list_supplys")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<Vec<AccountId>>()
            .unwrap()
    }

    pub async fn collateralize(&self, borrow_user: &Account, amount: u128) {
        eprintln!(
            "{} transferring {amount} tokens for collateral...",
            borrow_user.id(),
        );
        self.collateral_asset_transfer_call(
            borrow_user,
            self.contract.id(),
            amount,
            &serde_json::to_string(&Nep141MarketDepositMessage::Collateralize).unwrap(),
        )
        .await;
    }

    pub async fn get_borrow_position(&self, account_id: &AccountId) -> Option<BorrowPosition> {
        self.contract
            .view("get_borrow_position")
            .args_json(json!({
                "account_id": account_id,
            }))
            .await
            .unwrap()
            .json::<Option<BorrowPosition>>()
            .unwrap()
    }

    pub async fn list_borrows(&self) -> Vec<AccountId> {
        self.contract
            .view("list_borrows")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<Vec<AccountId>>()
            .unwrap()
    }

    pub async fn get_borrow_status(
        &self,
        account_id: &AccountId,
        oracle_response: OracleResponse,
    ) -> Option<BorrowStatus> {
        self.contract
            .view("get_borrow_status")
            .args_json(json!({
                "account_id": account_id,
                "oracle_response": oracle_response,
            }))
            .await
            .unwrap()
            .json::<Option<BorrowStatus>>()
            .unwrap()
    }

    pub async fn borrow(&self, borrow_user: &Account, amount: u128) -> ExecutionSuccess {
        eprintln!("{} borrowing {amount} tokens...", borrow_user.id());
        borrow_user
            .call(self.contract.id(), "borrow")
            .args_json(json!({
                "amount": U128(amount),
            }))
            .max_gas()
            .transact()
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn collateral_asset_balance_of(&self, account_id: &AccountId) -> u128 {
        self.collateral_asset
            .view("ft_balance_of")
            .args_json(json!({
                "account_id": account_id,
            }))
            .await
            .unwrap()
            .json::<U128>()
            .unwrap()
            .0
    }

    pub async fn borrow_asset_balance_of(&self, account_id: &AccountId) -> u128 {
        self.borrow_asset
            .view("ft_balance_of")
            .args_json(json!({
                "account_id": account_id,
            }))
            .await
            .unwrap()
            .json::<U128>()
            .unwrap()
            .0
    }

    pub async fn asset_transfer(
        &self,
        asset_id: &AccountId,
        sender: &Account,
        receiver_id: &AccountId,
        amount: u128,
    ) {
        eprintln!(
            "{} sending {amount} tokens of {asset_id} to {receiver_id}...",
            sender.id(),
        );
        sender
            .call(asset_id, "ft_transfer")
            .args_json(json!({
                "receiver_id": receiver_id,
                "amount": U128(amount),
            }))
            .deposit(NearToken::from_yoctonear(1))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn asset_transfer_call(
        &self,
        asset_id: &AccountId,
        sender: &Account,
        receiver_id: &AccountId,
        amount: u128,
        msg: &str,
    ) -> ExecutionSuccess {
        eprintln!(
            "{} sending {amount} tokens of {asset_id} to {receiver_id} with msg {msg}...",
            sender.id(),
        );
        sender
            .call(asset_id, "ft_transfer_call")
            .args_json(json!({
                "receiver_id": receiver_id,
                "amount": U128(amount),
                "msg": msg,
            }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn borrow_asset_transfer(
        &self,
        sender: &Account,
        receiver_id: &AccountId,
        amount: u128,
    ) {
        self.asset_transfer(self.borrow_asset.id(), sender, receiver_id, amount)
            .await;
    }

    pub async fn borrow_asset_transfer_call(
        &self,
        sender: &Account,
        receiver_id: &AccountId,
        amount: u128,
        msg: &str,
    ) -> ExecutionSuccess {
        self.asset_transfer_call(self.borrow_asset.id(), sender, receiver_id, amount, msg)
            .await
    }

    pub async fn collateral_asset_transfer_call(
        &self,
        sender: &Account,
        receiver_id: &AccountId,
        amount: u128,
        msg: &str,
    ) -> ExecutionSuccess {
        self.asset_transfer_call(self.collateral_asset.id(), sender, receiver_id, amount, msg)
            .await
    }

    pub async fn repay(&self, borrow_user: &Account, amount: u128) -> ExecutionSuccess {
        eprintln!("{} repaying {amount} tokens...", borrow_user.id());
        self.borrow_asset_transfer_call(
            borrow_user,
            self.contract.id(),
            amount,
            &serde_json::to_string(&Nep141MarketDepositMessage::Repay).unwrap(),
        )
        .await
    }

    pub async fn apply_interest(&self, borrow_user: &Account) -> ExecutionSuccess {
        eprintln!("{} applying interest...", borrow_user.id());
        borrow_user
            .call(self.contract.id(), "apply_interest")
            .args_json(json!({}))
            .transact()
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn harvest_yield(
        &self,
        supply_user: &Account,
        compounding: bool,
    ) -> ExecutionSuccess {
        eprintln!("{} harvesting yield...", supply_user.id());
        supply_user
            .call(self.contract.id(), "harvest_yield")
            .args_json(json!({
                "compounding": compounding,
            }))
            .max_gas()
            .transact()
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn withdraw_static_yield(
        &self,
        account: &Account,
        borrow_asset_amount: Option<BorrowAssetAmount>,
        collateral_asset_amount: Option<CollateralAssetAmount>,
    ) {
        eprintln!("{} withdrawing static yield...", account.id());
        account
            .call(self.contract.id(), "withdraw_static_yield")
            .args_json(json!({
                "borrow_asset_amount": borrow_asset_amount,
                "collateral_asset_amount": collateral_asset_amount,
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn get_static_yield(&self, account_id: &AccountId) -> Option<StaticYieldRecord> {
        self.contract
            .view("get_static_yield")
            .args_json(json!({
                "account_id": account_id,
            }))
            .await
            .unwrap()
            .json::<Option<StaticYieldRecord>>()
            .unwrap()
    }

    pub async fn withdraw_collateral(
        &self,
        borrow_user: &Account,
        amount: u128,
    ) -> ExecutionSuccess {
        eprintln!("{} withdrawing {amount} collateral...", borrow_user.id());
        borrow_user
            .call(self.contract.id(), "withdraw_collateral")
            .args_json(json!({
                "amount": U128(amount),
            }))
            .transact()
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn create_supply_withdrawal_request(&self, supply_user: &Account, amount: u128) {
        eprintln!(
            "{} creating supply withdrawal request for {amount}...",
            supply_user.id()
        );
        supply_user
            .call(self.contract.id(), "create_supply_withdrawal_request")
            .args_json(json!({
                "amount": U128(amount),
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn get_supply_withdrawal_request_status(
        &self,
        account_id: &AccountId,
    ) -> Option<WithdrawalRequestStatus> {
        self.contract
            .view("get_supply_withdrawal_request_status")
            .args_json(json!({
                "account_id": account_id,
            }))
            .await
            .unwrap()
            .json::<Option<WithdrawalRequestStatus>>()
            .unwrap()
    }

    pub async fn get_supply_withdrawal_queue_status(&self) -> WithdrawalQueueStatus {
        self.contract
            .view("get_supply_withdrawal_queue_status")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<WithdrawalQueueStatus>()
            .unwrap()
    }

    pub async fn execute_next_supply_withdrawal_request(&self, account: &Account) {
        eprintln!(
            "{} executing next supply withdrawal request...",
            account.id(),
        );
        account
            .call(self.contract.id(), "execute_next_supply_withdrawal_request")
            .args_json(json!({}))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn liquidate(
        &self,
        liquidator_user: &Account,
        account_id: &AccountId,
        borrow_asset_amount: u128,
    ) {
        eprintln!(
            "{} executing liquidation against {} for {}...",
            liquidator_user.id(),
            account_id,
            borrow_asset_amount,
        );
        self.borrow_asset_transfer_call(
            liquidator_user,
            self.contract.id(),
            borrow_asset_amount,
            &serde_json::to_string(&Nep141MarketDepositMessage::Liquidate(LiquidateMsg {
                account_id: account_id.clone(),
            }))
            .unwrap(),
        )
        .await;
    }

    pub async fn mint_asset(&self, ft_id: &AccountId, receiver: &Account, amount: u128) {
        eprintln!("{} minting {amount} of {}...", receiver.id(), ft_id);
        receiver
            .call(ft_id, "mint")
            .args_json(json!({
                "amount": U128(amount),
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn mint_collateral_asset(&self, receiver: &Account, amount: u128) {
        self.mint_asset(self.collateral_asset.id(), receiver, amount)
            .await;
    }

    pub async fn mint_borrow_asset(&self, receiver: &Account, amount: u128) {
        self.mint_asset(self.borrow_asset.id(), receiver, amount)
            .await;
    }

    pub async fn get_last_interest_rate(&self) -> Decimal {
        self.contract
            .view("get_last_interest_rate")
            .args_json(json!({}))
            .await
            .unwrap()
            .json()
            .unwrap()
    }

    pub async fn get_last_yield_rate(&self) -> Decimal {
        self.contract
            .view("get_last_yield_rate")
            .args_json(json!({}))
            .await
            .unwrap()
            .json()
            .unwrap()
    }

    #[allow(unused)] // This is useful for debugging tests
    pub async fn print_snapshots(&self) {
        let snapshots = self
            .contract
            .view("get_snapshots")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<Vec<Snapshot>>()
            .unwrap();

        eprintln!("Market snapshots:");
        for (i, snapshot) in snapshots.iter().enumerate() {
            eprintln!("\t{i}: {}", snapshot.time_chunk.0 .0);
            eprintln!("\t\tTimestamp:\t{}", snapshot.timestamp_ms.0);
            eprintln!("\t\tDeposited:\t{}", snapshot.deposited);
            eprintln!("\t\tBorrowed:\t{}", snapshot.borrowed);
            eprintln!("\t\tDistribution:\t{}", snapshot.yield_distribution);
        }
    }
}

pub async fn create_prefixed_account<T: DevNetwork + TopLevelAccountCreator + 'static>(
    prefix: &str,
    worker: &near_workspaces::Worker<T>,
) -> Account {
    let (genid, sk) = worker.dev_generate().await;
    let new_id: AccountId = format!("{prefix}{}", &genid.as_str()[prefix.len()..])
        .parse()
        .unwrap();
    worker.create_tla(new_id, sk).await.unwrap().unwrap()
}

macro_rules! accounts {
    ($w: ident, $($n:ident),*) => {
        $(let $n = create_prefixed_account(stringify!($n), &$w).await;)*
    };
}

pub fn market_configuration(
    balance_oracle_id: AccountId,
    borrow_asset_id: AccountId,
    collateral_asset_id: AccountId,
    protocol_account_id: AccountId,
    yield_weights: YieldWeights,
) -> MarketConfiguration {
    MarketConfiguration {
        time_chunk_configuration: templar_common::time_chunk::TimeChunkConfiguration::BlockHeight {
            divisor: U64(1),
        },
        borrow_asset: FungibleAsset::nep141(borrow_asset_id),
        collateral_asset: FungibleAsset::nep141(collateral_asset_id),
        balance_oracle: BalanceOracleConfiguration {
            account_id: balance_oracle_id,
            collateral_asset_price_id: PriceIdentifier(hex_literal::hex!(
                "1fc18861232290221461220bd4e2acd1dcdfbc89c84092c93c18bdc7756c1588"
            )),
            collateral_asset_decimals: 24,
            borrow_asset_price_id: PriceIdentifier(hex_literal::hex!(
                "27e867f0f4f61076456d1a73b14c7edc1cf5cef4f4d6193a33424288f11bd0f4"
            )),
            borrow_asset_decimals: 24,
            price_maximum_age_s: 60,
        },
        borrow_mcr_initial: Decimal::from_str("1.25").unwrap(),
        borrow_mcr: Decimal::from_str("1.2").unwrap(),
        borrow_asset_maximum_usage_ratio: Decimal::from_str("0.99").unwrap(),
        borrow_origination_fee: Fee::Proportional(Decimal::from_str("0.1").unwrap()),
        borrow_interest_rate_strategy: InterestRateStrategy::piecewise(
            Decimal::ZERO,
            dec!("0.9"),
            dec!("0.04"),
            dec!("0.6"),
        )
        .unwrap(),
        borrow_maximum_duration_ms: None,
        borrow_minimum_amount: 1.into(),
        borrow_maximum_amount: u128::MAX.into(),
        liquidation_maximum_spread: Decimal::from_str("0.05").unwrap(),
        supply_withdrawal_fee: TimeBasedFee::zero(),
        supply_maximum_amount: None,
        yield_weights,
        protocol_account_id,
    }
}

async fn compile_contract(p: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_WORKSPACE_DIR")).join(p);
    near_workspaces::compile_project(path.to_str().unwrap())
        .await
        .unwrap()
}

async fn read_contract(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_WORKSPACE_DIR"))
        .join("target/near/")
        .join(name)
        .join(name.to_owned() + ".wasm");

    std::fs::read(path).unwrap()
}

async fn get_contract(name: &str, path: &str) -> Vec<u8> {
    if std::env::var("TEST_CONTRACTS_PREBUILT").is_ok() {
        read_contract(name).await
    } else {
        compile_contract(path).await
    }
}

pub static WASM_MARKET: OnceCell<Vec<u8>> = OnceCell::const_new();
pub static WASM_MOCK_FT: OnceCell<Vec<u8>> = OnceCell::const_new();
pub static WASM_MOCK_ORACLE: OnceCell<Vec<u8>> = OnceCell::const_new();

pub async fn setup_market(
    worker: &Worker<Sandbox>,
    configuration: &MarketConfiguration,
) -> Contract {
    let wasm = WASM_MARKET
        .get_or_init(|| get_contract("templar_market_contract", "contract/market"))
        .await;

    let contract = worker.dev_deploy(wasm).await.unwrap();
    contract
        .call("new")
        .args_json(json!({
            "configuration": configuration,
        }))
        .transact()
        .await
        .unwrap()
        .unwrap();

    contract
}

pub async fn deploy_oracle(account: Account) -> Contract {
    let wasm = WASM_MOCK_ORACLE
        .get_or_init(|| get_contract("mock_oracle", "mock/oracle"))
        .await;

    let contract = account.deploy(wasm).await.unwrap().unwrap();
    contract
        .call("new")
        .args_json(json!({}))
        .transact()
        .await
        .unwrap()
        .unwrap();

    contract
}

pub async fn deploy_ft(account: Account, name: &str, symbol: &str) -> Contract {
    let wasm = WASM_MOCK_FT
        .get_or_init(|| get_contract("mock_ft", "mock/ft"))
        .await;

    let contract = account.deploy(wasm).await.unwrap().unwrap();
    contract
        .call("new")
        .args_json(json!({
            "name": name,
            "symbol": symbol,
        }))
        .transact()
        .await
        .unwrap()
        .unwrap();

    contract
}

pub struct SetupEverything {
    pub c: TestController,
    pub liquidator_user: Account,
    pub supply_user: Account,
    pub supply_user_2: Account,
    pub borrow_user: Account,
    pub borrow_user_2: Account,
    pub protocol_yield_user: Account,
    pub insurance_yield_user: Account,
}

pub async fn setup_everything(
    customize_market_configuration: impl FnOnce(&mut MarketConfiguration),
) -> SetupEverything {
    let worker = near_workspaces::sandbox().await.unwrap();
    accounts!(
        worker,
        liquidator_user,
        supply_user,
        supply_user_2,
        borrow_user,
        borrow_user_2,
        protocol_yield_user,
        insurance_yield_user,
        collateral_asset,
        borrow_asset,
        balance_oracle
    );
    let mut config = market_configuration(
        balance_oracle.id().clone(),
        borrow_asset.id().clone(),
        collateral_asset.id().clone(),
        protocol_yield_user.id().clone(),
        YieldWeights::new_with_supply_weight(8)
            .with_static(protocol_yield_user.id().clone(), 1)
            .with_static(insurance_yield_user.id().clone(), 1),
    );
    customize_market_configuration(&mut config);

    let (contract, balance_oracle, borrow_asset, collateral_asset) = tokio::join!(
        setup_market(&worker, &config),
        deploy_oracle(balance_oracle),
        deploy_ft(borrow_asset, "Borrow Asset", "BORROW"),
        deploy_ft(collateral_asset, "Collateral Asset", "COLLATERAL"),
    );

    let c = TestController {
        worker,
        config,
        contract,
        balance_oracle,
        collateral_asset,
        borrow_asset,
    };

    c.set_borrow_asset_price(1.0).await;
    c.set_collateral_asset_price(1.0).await;

    // Asset opt-ins.
    tokio::join!(
        c.storage_deposits(c.contract.as_account()),
        async {
            c.storage_deposits(&liquidator_user).await;
            c.mint_borrow_asset(&liquidator_user, 100_000_000).await;
        },
        async {
            c.storage_deposits(&borrow_user).await;
            c.mint_collateral_asset(&borrow_user, 100_000_000).await;
            c.mint_borrow_asset(&borrow_user, 100_000_000).await;
        },
        async {
            c.storage_deposits(&borrow_user_2).await;
            c.mint_collateral_asset(&borrow_user_2, 100_000_000).await;
            c.mint_borrow_asset(&borrow_user_2, 100_000_000).await;
        },
        async {
            c.storage_deposits(&supply_user).await;
            c.mint_borrow_asset(&supply_user, 100_000_000).await;
        },
        async {
            c.storage_deposits(&supply_user_2).await;
            c.mint_borrow_asset(&supply_user_2, 100_000_000).await;
        },
        c.storage_deposits(&protocol_yield_user),
        c.storage_deposits(&insurance_yield_user),
    );

    SetupEverything {
        c,
        liquidator_user,
        supply_user,
        supply_user_2,
        borrow_user,
        borrow_user_2,
        protocol_yield_user,
        insurance_yield_user,
    }
}
