use controller::lst_oracle::LstOracleController;
use rstest::rstest;
use templar_common::{
    dec,
    fee::Fee,
    interest_rate_strategy::InterestRateStrategy,
    market::YieldWeights,
    oracle::{
        price_transformer::{self, PriceTransformer},
        pyth::PriceIdentifier,
    },
};
use test_utils::*;

#[rstest]
#[tokio::test]
async fn lst_oracle() {
    let worker = near_workspaces::sandbox().await.unwrap();

    let collateral_lst_id = PriceIdentifier(hex_literal::hex!(
        "cc11000000000000000000000000000000000000000000000000000000000000"
    ));

    setup_test_w!(
        worker
        extract(c, protocol_yield_user)
        accounts(supply_user, borrow_user, lst_oracle, lst_market)
    );

    let mut configuration = market_configuration(
        lst_oracle.id().clone(),
        c.borrow_asset.contract().id().clone(),
        c.collateral_asset.contract().id().clone(),
        protocol_yield_user.id().clone(),
        YieldWeights::new_with_supply_weight(1),
    );

    configuration
        .price_oracle_configuration
        .collateral_asset_price_id = collateral_lst_id;

    configuration.borrow_mcr = dec!("2");
    configuration.borrow_mcr_initial = dec!("2");

    configuration.borrow_origination_fee = Fee::zero();
    configuration.borrow_interest_rate_strategy = InterestRateStrategy::zero();

    let (lst_market, lst_oracle) = tokio::join!(
        async { MarketController::deploy(lst_market, &configuration).await },
        async {
            let lst_oracle =
                LstOracleController::deploy(lst_oracle, c.price_oracle.contract().id()).await;

            lst_oracle
                .create_transformer(
                    lst_oracle.contract().as_account(),
                    collateral_lst_id,
                    PriceTransformer::lst(
                        DEFAULT_COLLATERAL_PRICE_ID,
                        price_transformer::Call::new_simple(
                            c.collateral_asset.contract().id(),
                            "redemption_rate",
                        ),
                    ),
                )
                .await;

            lst_oracle
        },
    );

    let c = UnifiedMarketController {
        market: lst_market,
        configuration,
        ..c
    };

    let storage_bounds = c.market.storage_balance_bounds().await;
    c.market
        .storage_deposit(&supply_user, storage_bounds.min)
        .await;
    c.market
        .storage_deposit(&borrow_user, storage_bounds.min)
        .await;

    tokio::join!(
        // 2:1
        c.collateral_asset.set_redemption_rate(2 * 10u128.pow(24)),
        c.supply_and_harvest_until_activation(&supply_user, 10_000_000),
        c.collateralize(&borrow_user, 1_000_000),
    );

    let oracle_response = lst_oracle
        .list_ema_prices_no_older_than(
            lst_oracle.contract().as_account(),
            vec![DEFAULT_BORROW_PRICE_ID, collateral_lst_id],
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
