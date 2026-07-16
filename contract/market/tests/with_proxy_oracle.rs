//! A market whose oracle is a **proxy oracle** that aggregates a pyth mock and a
//! redstone mock via `Proxy::median_low`, one proxy per asset price id. The two
//! `proxy_*_pyth_first` parameters flip the source order within each proxy (which
//! must not change availability). For every one of the 16 combinations of
//! (pyth_borrow, pyth_collateral, redstone_borrow, redstone_collateral)
//! availability we set the mock prices, refresh the proxy's cache, then assert a
//! `collateralize` succeeds **iff** both the borrow and collateral proxied prices
//! are available (and otherwise leaves the position untouched).
//!
//! A rejected collateralize is refunded via `ft_transfer_call`, so we drive it
//! through `try_collateralize` and assert the *effect* (collateral unchanged)
//! rather than the operation status. Unlike the mock-oracle tests, the proxy
//! caches prices and filters them by freshness, so prices are stamped with the
//! real current time (not the epoch-zero `to_price`).

use anyhow::{Context, Result};
use near_sdk::json_types::{I64, U64};
use rstest::rstest;
use templar_common::{
    oracle::{
        pyth::{self, PriceIdentifier, PythTimestamp},
        redstone::FeedData,
    },
    primitive_types::U256,
};
use templar_gateway_testing::{harness, SandboxHarness};
use templar_proxy_oracle_kernel::proxy::{FreshnessFilter, Proxy};
use templar_proxy_oracle_near_common::{input::Source, request::OracleRequest};
use test_utils::{DEFAULT_BORROW_PRICE_ID, DEFAULT_COLLATERAL_PRICE_ID};

/// Source price id of the pyth borrow feed (distinct from the proxy's own
/// `DEFAULT_BORROW_PRICE_ID`, which keys the aggregated result).
const PYTH_BORROW_PRICE_ID: PriceIdentifier = PriceIdentifier([0xb7_u8; 32]);
const PYTH_COLLATERAL_PRICE_ID: PriceIdentifier = PriceIdentifier([0xc7_u8; 32]);
const REDSTONE_BORROW_FEED_ID: &str = "BORROW/USD";
const REDSTONE_COLLATERAL_FEED_ID: &str = "COLLATERAL/USD";

/// Stamped on chain time, not the host clock — see
/// [`SandboxHarness::chain_timestamp`]. The proxy filters a price older than the
/// market's `price_maximum_age_s` against `block_timestamp`, so on a pooled node
/// that some earlier test has `fast_forward`ed, a host-stamped price is filtered
/// out as stale. This test derives its expectations from the same filtered view,
/// so it would not go red — it would pass while asserting nothing.
#[allow(clippy::cast_possible_truncation)]
async fn pyth_price(harness: &SandboxHarness, price: f64) -> Result<pyth::Price> {
    Ok(pyth::Price {
        price: I64((price * 10000.0) as i64),
        conf: U64(0),
        expo: -4,
        publish_time: PythTimestamp::try_from_time(harness.chain_timestamp().await?)
            .context("chain timestamp out of pyth range")?,
    })
}

/// Chain time, not the host clock — see [`pyth_price`].
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
async fn redstone_price(harness: &SandboxHarness, price: f64) -> Result<FeedData> {
    let now = harness.chain_timestamp().await?;
    Ok(FeedData {
        price: U256::from((price * 1e8) as u128).into(),
        package_timestamp: now,
        write_timestamp: now,
    })
}

#[rstest]
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn proxy_oracle(
    #[future(awt)] harness: SandboxHarness,
    #[values(true, false)] proxy_borrow_pyth_first: bool,
    #[values(true, false)] proxy_collateral_pyth_first: bool,
) -> Result<()> {
    // Proxy oracle plus the two mock oracles it aggregates.
    let proxy_id = harness.deploy_proxy_oracle().await?;
    let pyth_id = harness.deploy_mock_oracle("pyth").await?;
    let redstone_id = harness.deploy_mock_oracle("redstone").await?;

    // Each proxy aggregates its pyth + redstone source via median_low; the
    // `pyth_first` flags flip the source ordering (which must not matter).
    let mut collateral_sources: Vec<Source> = vec![
        OracleRequest::pyth(pyth_id.clone(), PYTH_COLLATERAL_PRICE_ID).into(),
        OracleRequest::redstone(redstone_id.clone(), REDSTONE_COLLATERAL_FEED_ID).into(),
    ];
    if !proxy_collateral_pyth_first {
        collateral_sources.reverse();
    }
    harness
        .admin_set_proxy(
            proxy_id.clone(),
            DEFAULT_COLLATERAL_PRICE_ID,
            Some(Proxy::median_low(
                collateral_sources,
                FreshnessFilter::empty(),
            )),
        )
        .await?;

    let mut borrow_sources: Vec<Source> = vec![
        OracleRequest::pyth(pyth_id.clone(), PYTH_BORROW_PRICE_ID).into(),
        OracleRequest::redstone(redstone_id.clone(), REDSTONE_BORROW_FEED_ID).into(),
    ];
    if !proxy_borrow_pyth_first {
        borrow_sources.reverse();
    }
    harness
        .admin_set_proxy(
            proxy_id.clone(),
            DEFAULT_BORROW_PRICE_ID,
            Some(Proxy::median_low(borrow_sources, FreshnessFilter::empty())),
        )
        .await?;

    // Market pointed at the proxy oracle.
    let market = harness
        .deploy_full_market_with_oracle(proxy_id.clone(), |_| {})
        .await?;

    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;
    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 100_000_000)
        .await?;

    for pyth_borrow in [false, true] {
        for pyth_collateral in [false, true] {
            for redstone_borrow in [false, true] {
                for redstone_collateral in [false, true] {
                    // Re-stamped every iteration: the four set-price transactions
                    // plus `update_proxy_prices` advance chain time, so a price
                    // stamped once outside the loop would age past the freshness
                    // window by the later iterations.
                    let pyth = pyth_price(&harness, 1.0).await?;
                    let redstone = redstone_price(&harness, 1.0).await?;

                    harness
                        .set_mock_oracle_pyth_price(
                            pyth_id.clone(),
                            PYTH_BORROW_PRICE_ID,
                            pyth_borrow.then(|| pyth.clone()),
                        )
                        .await?;
                    harness
                        .set_mock_oracle_pyth_price(
                            pyth_id.clone(),
                            PYTH_COLLATERAL_PRICE_ID,
                            pyth_collateral.then(|| pyth.clone()),
                        )
                        .await?;
                    harness
                        .set_mock_oracle_redstone_price(
                            redstone_id.clone(),
                            REDSTONE_BORROW_FEED_ID.into(),
                            redstone_borrow.then(|| redstone.clone()),
                        )
                        .await?;
                    harness
                        .set_mock_oracle_redstone_price(
                            redstone_id.clone(),
                            REDSTONE_COLLATERAL_FEED_ID.into(),
                            redstone_collateral.then(|| redstone.clone()),
                        )
                        .await?;

                    harness
                        .update_proxy_prices(
                            proxy_id.clone(),
                            vec![DEFAULT_BORROW_PRICE_ID, DEFAULT_COLLATERAL_PRICE_ID],
                        )
                        .await?;

                    let collateral_before =
                        collateral_amount(&harness, &market, &borrow_user).await?;

                    // The market accepts a collateralize only when both proxied
                    // prices are currently available — read them straight from the
                    // proxy to derive the expectation, exactly as the contract sees.
                    let available = harness.get_oracle_prices(&market).await?;
                    let expect_success = available
                        .get(&DEFAULT_BORROW_PRICE_ID)
                        .and_then(Option::as_ref)
                        .is_some()
                        && available
                            .get(&DEFAULT_COLLATERAL_PRICE_ID)
                            .and_then(Option::as_ref)
                            .is_some();

                    harness
                        .try_collateralize(&borrow_user, &market, 1_000_000)
                        .await?;

                    let collateral_after =
                        collateral_amount(&harness, &market, &borrow_user).await?;
                    if expect_success {
                        assert_eq!(
                            collateral_before + 1_000_000,
                            collateral_after,
                            "collateralize should succeed when both prices are available \
                             (pyth_borrow={pyth_borrow}, pyth_collateral={pyth_collateral}, \
                             redstone_borrow={redstone_borrow}, redstone_collateral={redstone_collateral})",
                        );
                    } else {
                        assert_eq!(
                            collateral_before, collateral_after,
                            "collateralize should be a no-op when a price is unavailable \
                             (pyth_borrow={pyth_borrow}, pyth_collateral={pyth_collateral}, \
                             redstone_borrow={redstone_borrow}, redstone_collateral={redstone_collateral})",
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

/// The borrow position's total collateral for `user`, or `0` if there is no
/// position yet.
async fn collateral_amount(
    harness: &SandboxHarness,
    market: &templar_gateway_testing::DeployedMarket,
    user: &templar_gateway_types::ManagedAccountId,
) -> Result<u128> {
    Ok(harness
        .get_borrow_position(market, &user.0)
        .await?
        .map_or(0, |position| {
            u128::from(position.get_total_collateral_amount())
        }))
}
