use std::{path::Path, str::FromStr};

use controller::{ft::FtController, market::MarketController, oracle::OracleController};
use near_sdk::{
    json_types::{I64, U64},
    AccountId,
};
use near_workspaces::{network::Sandbox, prelude::*, Account, DevNetwork, Worker};
use templar_common::{
    asset::FungibleAsset,
    dec,
    fee::{Fee, TimeBasedFee},
    interest_rate_strategy::InterestRateStrategy,
    market::{BalanceOracleConfiguration, MarketConfiguration, YieldWeights},
    number::Decimal,
    oracle::pyth::{self, PriceIdentifier},
};

pub mod controller;

pub fn to_price(price: f64) -> pyth::Price {
    pyth::Price {
        price: I64((price * 10000.0) as i64),
        conf: U64(0),
        expo: -4,
        publish_time: 0,
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

pub struct SetupEverything {
    pub worker: Worker<Sandbox>,
    pub c: MarketController,
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
        market,
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

    let (balance_oracle, borrow_asset, collateral_asset) = tokio::join!(
        OracleController::setup(balance_oracle),
        FtController::setup(borrow_asset, "Borrow Asset", "BORROW"),
        FtController::setup(collateral_asset, "Collateral Asset", "COLLATERAL"),
    );

    let c = MarketController::setup(
        market,
        config,
        balance_oracle,
        borrow_asset,
        collateral_asset,
    )
    .await;

    c.set_borrow_asset_price(1.0).await;
    c.set_collateral_asset_price(1.0).await;

    // Asset opt-ins.
    tokio::join!(
        c.storage_deposits(c.contract.as_account()),
        async {
            c.storage_deposits(&liquidator_user).await;
            c.borrow_asset
                .mint(&liquidator_user, 100_000_000.into())
                .await;
        },
        async {
            c.storage_deposits(&borrow_user).await;
            c.collateral_asset
                .mint(&borrow_user, 100_000_000.into())
                .await;
            c.borrow_asset.mint(&borrow_user, 100_000_000.into()).await;
        },
        async {
            c.storage_deposits(&borrow_user_2).await;
            c.collateral_asset
                .mint(&borrow_user_2, 100_000_000.into())
                .await;
            c.borrow_asset
                .mint(&borrow_user_2, 100_000_000.into())
                .await;
        },
        async {
            c.storage_deposits(&supply_user).await;
            c.borrow_asset.mint(&supply_user, 100_000_000.into()).await;
        },
        async {
            c.storage_deposits(&supply_user_2).await;
            c.borrow_asset
                .mint(&supply_user_2, 100_000_000.into())
                .await;
        },
        c.storage_deposits(&protocol_yield_user),
        c.storage_deposits(&insurance_yield_user),
    );

    SetupEverything {
        worker,
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
