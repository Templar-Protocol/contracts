use anyhow::Result;
use serde_json::json;
use templar_common::{
    oracle::pyth::{Price, PriceIdentifier},
    Nanoseconds,
};
use templar_gateway_methods_spec::{oracle, proxy_oracle};
use templar_gateway_testing::SandboxHarness;
use templar_proxy_oracle_kernel::proxy::{FreshnessFilter, Proxy};
use templar_proxy_oracle_near_common::request::OracleRequest;

fn price(value: i64, published: u64) -> Result<Price> {
    Ok(serde_json::from_value(json!({
        "price": value.to_string(), "conf": "0", "expo": -8,
        "publish_time": published
    }))?)
}

#[tokio::test]
async fn readiness_requires_every_source_at_the_chain_clock() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let source = harness.deploy_mock_oracle("readiness-source").await?;
    let proxy = harness.deploy_proxy_oracle().await?;
    let id = PriceIdentifier([0x51; 32]);
    let second_id = PriceIdentifier([0x52; 32]);
    let client = harness.client()?;
    let ready = proxy_oracle::GetFreshSourcePrice {
        oracle_id: proxy.clone(),
        id,
        age: 3600,
    };
    let ordinary = oracle::GetPrice {
        oracle_id: proxy.clone(),
        price_id: id,
        age: 3600,
    };
    // Median can resolve from one healthy source; readiness must not.
    harness
        .admin_set_proxy(
            proxy.clone(),
            id,
            Some(Proxy::median_low(
                [
                    OracleRequest::pyth(source.clone(), id).into(),
                    OracleRequest::pyth(source.clone(), second_id).into(),
                ],
                FreshnessFilter::new(Some(Nanoseconds::from_secs(120)), None),
            )),
        )
        .await?;
    let now = harness.chain_timestamp().await?.as_secs();
    harness
        .set_mock_oracle_pyth_price(source.clone(), id, Some(price(100, now)?))
        .await?;
    harness
        .set_mock_oracle_pyth_price(source.clone(), second_id, Some(price(200, now)?))
        .await?;
    assert!(client.read(ready.clone()).await?.is_some());
    assert!(client
        .read(proxy_oracle::GetCachedPrice {
            oracle_id: proxy.clone(),
            id,
            age: 3600
        })
        .await?
        .is_none());

    for invalid in [
        Some(price(200, now + 86_400)?),
        Some(price(200, now.saturating_sub(600))?),
        Some(price(-200, now)?),
        None,
    ] {
        harness
            .set_mock_oracle_pyth_price(source.clone(), second_id, invalid)
            .await?;
        assert!(client.read(ready.clone()).await?.is_none());
        assert!(client.read(ordinary.clone()).await?.is_some());
    }

    // Explicit node time advancement also exercises requested-age rejection
    // independently of the deployed 120-second filter.
    harness.fast_forward(5).await?;
    let now = harness.chain_timestamp().await?.as_secs();
    for source_id in [id, second_id] {
        harness
            .set_mock_oracle_pyth_price(source.clone(), source_id, Some(price(100, now - 30)?))
            .await?;
    }
    assert!(client.read(ready.clone()).await?.is_some());
    assert!(client
        .read(proxy_oracle::GetFreshSourcePrice {
            age: 1,
            ..ready.clone()
        })
        .await?
        .is_none());
    let now = harness.chain_timestamp().await?.as_secs();
    for source_id in [id, second_id] {
        harness
            .set_mock_oracle_pyth_price(source.clone(), source_id, Some(price(100, now)?))
            .await?;
    }
    assert!(client.read(ready).await?.is_some());
    Ok(())
}
