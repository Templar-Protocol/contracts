use controller::lst_oracle::LstOracleController;
use near_workspaces::{network::Sandbox, Account, Worker};
use rstest::rstest;
use templar_common::{
    dec,
    fee::Fee,
    interest_rate_strategy::InterestRateStrategy,
    market::{MarketConfiguration, YieldWeights},
    oracle::{
        price_transformer::{Call, PriceTransformer},
        pyth::PriceIdentifier,
    },
};
use test_utils::*;

fn redemption_rate_call(account_id: &near_sdk::AccountIdRef) -> Call {
    Call {
        account_id: account_id.into(),
        method_name: "redemption_rate".to_string(),
        args: near_sdk::json_types::Base64VecU8(vec![]),
        gas: near_sdk::Gas::from_tgas(3).as_gas().into(),
    }
}

const COLLATERAL_LST_ID: PriceIdentifier = PriceIdentifier(hex_literal::hex!(
    "cc11000000000000000000000000000000000000000000000000000000000000"
));

async fn setup_lst_oracle(
    c: &UnifiedMarketController,
    lst_oracle: Account,
    lst_market: Account,
    storage_deposits: impl IntoIterator<Item = &Account>,
    config_fn: impl FnOnce(&mut MarketConfiguration),
) -> (UnifiedMarketController, LstOracleController) {
    let mut configuration = market_configuration(
        lst_oracle.id().clone(),
        c.borrow_asset.contract().id().clone(),
        c.collateral_asset.contract().id().clone(),
        c.configuration.protocol_account_id.clone(),
        YieldWeights::new_with_supply_weight(1),
    );

    configuration
        .price_oracle_configuration
        .collateral_asset_price_id = COLLATERAL_LST_ID;

    config_fn(&mut configuration);

    let (lst_market, lst_oracle) = tokio::join!(
        async { MarketController::deploy(lst_market, &configuration).await },
        async {
            let lst_oracle =
                LstOracleController::deploy(lst_oracle, c.price_oracle.id().clone()).await;

            lst_oracle
                .create_transformer(
                    lst_oracle.contract().as_account(),
                    COLLATERAL_LST_ID,
                    PriceTransformer::lst(
                        DEFAULT_COLLATERAL_PRICE_ID,
                        24,
                        redemption_rate_call(c.collateral_asset.contract().id()),
                    ),
                )
                .await;

            lst_oracle
        },
    );

    let c = UnifiedMarketController {
        market: lst_market,
        configuration,
        ..c.clone()
    };

    let storage_bounds = c.market.storage_balance_bounds().await;
    for account in storage_deposits {
        c.market.storage_deposit(account, storage_bounds.min).await;
    }

    (c, lst_oracle)
}

#[rstest]
#[tokio::test]
async fn lst_oracle(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(c, protocol_yield_user)
        accounts(supply_user, borrow_user, lst_oracle, lst_market)
    );

    let original_oracle_id = c
        .configuration
        .price_oracle_configuration
        .account_id
        .clone();

    let (c, lst_oracle) = setup_lst_oracle(
        &c,
        lst_oracle,
        lst_market,
        [&supply_user, &borrow_user],
        |configuration| {
            configuration.borrow_mcr_liquidation = dec!("2");
            configuration.borrow_mcr_maintenance = dec!("2");

            configuration.borrow_origination_fee = Fee::zero();
            configuration.borrow_interest_rate_strategy = InterestRateStrategy::zero();
        },
    )
    .await;

    tokio::join!(
        // 2:1
        c.collateral_asset.set_redemption_rate(2 * 10u128.pow(24)),
        c.supply_and_harvest_until_activation(&supply_user, 10_000_000),
        c.collateralize(&borrow_user, 1_000_000),
    );

    let underlying_oracle_actual = lst_oracle.oracle_id().await;
    assert_eq!(underlying_oracle_actual, original_oracle_id);

    let transformers = lst_oracle.list_transformers(None, None).await;
    assert_eq!(transformers, vec![COLLATERAL_LST_ID]);
    let transformer = lst_oracle.get_transformer(COLLATERAL_LST_ID).await.unwrap();
    assert_eq!(
        transformer,
        PriceTransformer::lst(
            DEFAULT_COLLATERAL_PRICE_ID,
            24,
            redemption_rate_call(c.collateral_asset.contract().id()),
        ),
    );

    for should_exist in [
        COLLATERAL_LST_ID,
        DEFAULT_COLLATERAL_PRICE_ID,
        DEFAULT_BORROW_PRICE_ID,
    ] {
        assert!(
            lst_oracle
                .price_feed_exists(lst_oracle.contract.as_account(), should_exist)
                .await,
            "Price ID {should_exist} should exist",
        );
    }

    assert!(
        !lst_oracle
            .price_feed_exists(
                lst_oracle.contract.as_account(),
                PriceIdentifier([0x88; 32]),
            )
            .await,
    );

    let oracle_response = lst_oracle
        .list_ema_prices_no_older_than(
            lst_oracle.contract().as_account(),
            vec![DEFAULT_BORROW_PRICE_ID, COLLATERAL_LST_ID],
            60u32,
        )
        .await;

    c.borrow(&borrow_user, 1_000_000).await;
    let status = c
        .get_borrow_status(borrow_user.id(), oracle_response)
        .await
        .unwrap();

    assert!(status.is_healthy());

    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();

    assert_eq!(borrow_position.collateral_asset_deposit, 1_000_000.into());
    assert_eq!(
        borrow_position.get_total_borrow_asset_liability(),
        1_000_000.into(),
    );
}
