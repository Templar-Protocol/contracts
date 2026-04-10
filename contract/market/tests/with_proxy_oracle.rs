use near_sdk::json_types::{I64, U64};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use tokio::join;

use templar_common::{
    market::YieldWeights,
    oracle::{
        pyth::{self, PriceIdentifier, PythTimestamp},
        redstone::FeedData,
    },
    primitive_types::U256,
    time::Nanoseconds,
};
use test_utils::*;

#[allow(clippy::cast_possible_truncation)]
pub fn pyth_price(price: f64) -> pyth::Price {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    pyth::Price {
        price: I64((price * 10000.0) as i64),
        conf: U64(0),
        expo: -4,
        publish_time: PythTimestamp::from_ms(now_ms),
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn redstone_price(price: f64) -> FeedData {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let now_ms = Nanoseconds::from_ms(now_ms);
    FeedData {
        price: U256::from((price * 1e8) as u128).into(),
        package_timestamp: now_ms,
        write_timestamp: now_ms,
    }
}

#[rstest]
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn proxy_oracle(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(true, false)] proxy_borrow_pyth_first: bool,
    #[values(true, false)] proxy_collateral_pyth_first: bool,
) {
    use templar_common::asset::CollateralAssetAmount;

    const PYTH_BORROW_PRICE_ID: PriceIdentifier = PriceIdentifier([0xb7_u8; 32]);
    const PYTH_COLLATERAL_PRICE_ID: PriceIdentifier = PriceIdentifier([0xc7_u8; 32]);
    const REDSTONE_BORROW_FEED_ID: &str = "BORROW/USD";
    const REDSTONE_COLLATERAL_FEED_ID: &str = "COLLATERAL/USD";

    accounts!(
        worker,
        borrow_asset,
        collateral_asset,
        pyth_oracle,
        redstone_oracle,
        proxy_oracle,
        market,
        protocol_user,
        borrow_user,
        supply_user
    );

    let borrow_asset = FtController::deploy(borrow_asset, "Borrow Asset", "BORROW");
    let collateral_asset = FtController::deploy(collateral_asset, "Collateral Asset", "COLLATERAL");
    let pyth_oracle = MockOracleController::deploy(pyth_oracle);
    let redstone_oracle = MockOracleController::deploy(redstone_oracle);
    let proxy_oracle = ProxyOracleController::deploy(proxy_oracle);

    let (borrow_asset, collateral_asset, pyth_oracle, redstone_oracle, proxy_oracle) = join!(
        borrow_asset,
        collateral_asset,
        pyth_oracle,
        redstone_oracle,
        proxy_oracle
    );

    let set_pyth = |id: PriceIdentifier, price: Option<f64>| {
        let pyth_oracle = &pyth_oracle;
        async move {
            pyth_oracle
                .set_pyth_price(pyth_oracle.account(), id, price.map(pyth_price))
                .await;
        }
    };
    let set_redstone = |id: &'static str, price: Option<f64>| {
        let redstone_oracle = &redstone_oracle;
        async move {
            redstone_oracle
                .set_redstone_price(redstone_oracle.account(), id, price.map(redstone_price))
                .await;
        }
    };

    let mut oracle_requests_collateral: Vec<Source> = vec![
        OracleRequest::pyth(pyth_oracle.id().clone(), PYTH_COLLATERAL_PRICE_ID).into(),
        OracleRequest::redstone(redstone_oracle.id().clone(), REDSTONE_COLLATERAL_FEED_ID).into(),
    ];
    if !proxy_collateral_pyth_first {
        oracle_requests_collateral.reverse();
    }
    proxy_oracle
        .set_proxy(
            proxy_oracle.account(),
            DEFAULT_COLLATERAL_PRICE_ID,
            Some(Proxy::median_low(
                oracle_requests_collateral,
                FreshnessFilter::empty(),
            )),
        )
        .await;

    let mut oracle_requests_borrow: Vec<Source> = vec![
        OracleRequest::pyth(pyth_oracle.id().clone(), PYTH_BORROW_PRICE_ID).into(),
        OracleRequest::redstone(redstone_oracle.id().clone(), REDSTONE_BORROW_FEED_ID).into(),
    ];
    if !proxy_borrow_pyth_first {
        oracle_requests_borrow.reverse();
    }
    proxy_oracle
        .set_proxy(
            proxy_oracle.account(),
            DEFAULT_BORROW_PRICE_ID,
            Some(Proxy::median_low(
                oracle_requests_borrow,
                FreshnessFilter::empty(),
            )),
        )
        .await;

    let configuration = market_configuration(
        proxy_oracle.id().clone(),
        borrow_asset.id().clone(),
        collateral_asset.id().clone(),
        protocol_user.id().clone(),
        YieldWeights::new_with_supply_weight(4).with_static(protocol_user.id().clone(), 1),
    );

    let market = MarketController::deploy(market, &configuration).await;

    let c = UnifiedMarketController::attach(&worker, market.id().clone()).await;

    join!(
        c.init_account(&borrow_user),
        c.init_account(&supply_user),
        c.init_account(&protocol_user),
        c.storage_deposits(market.account()),
    );

    c.supply_and_harvest_until_activation(&supply_user, 100_000_000)
        .await;

    for (pyth_borrow, pyth_collateral, redstone_borrow, redstone_collateral) in
        itertools::iproduct!([false, true], [false, true], [false, true], [false, true])
    {
        join!(
            async {
                set_pyth(PYTH_BORROW_PRICE_ID, pyth_borrow.then_some(1.0)).await;
                set_pyth(PYTH_COLLATERAL_PRICE_ID, pyth_collateral.then_some(1.0)).await;
            },
            async {
                set_redstone(REDSTONE_BORROW_FEED_ID, redstone_borrow.then_some(1.0)).await;
                set_redstone(
                    REDSTONE_COLLATERAL_FEED_ID,
                    redstone_collateral.then_some(1.0),
                )
                .await;
            },
        );

        let collateral_before = c
            .get_borrow_position(borrow_user.id())
            .await
            .map_or(0.into(), |p| p.get_total_collateral_amount());

        c.collateralize(&borrow_user, 1_000_000).await;
        let expect_success =
            (pyth_borrow || redstone_borrow) && (pyth_collateral || redstone_collateral);

        let collateral_after = c
            .get_borrow_position(borrow_user.id())
            .await
            .map_or(0.into(), |p| p.get_total_collateral_amount());
        if expect_success {
            assert_eq!(
                collateral_before + CollateralAssetAmount::new(1_000_000),
                collateral_after
            );
        } else {
            assert_eq!(collateral_before, collateral_after);
        }
    }
}
