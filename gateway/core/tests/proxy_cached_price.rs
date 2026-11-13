use anyhow::{Context as _, Result};
use serde_json::json;
use templar_common::{
    oracle::pyth::{Price, PriceIdentifier},
    Nanoseconds,
};
use templar_gateway_methods_spec::{oracle, proxy_oracle};
use templar_gateway_testing::SandboxHarness;
use templar_gateway_types::ManagedAccountId;
use templar_proxy_oracle_kernel::proxy::{FreshnessFilter, Proxy};
use templar_proxy_oracle_near_common::request::OracleRequest;

#[tokio::test]
async fn cached_price_requires_refresh_and_respects_manual_trip() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let source = harness.deploy_mock_oracle("cache-source").await?;
    let proxy = harness.deploy_proxy_oracle().await?;
    let id = PriceIdentifier([0x42; 32]);
    let now = harness.chain_timestamp().await?.as_secs();
    let price: Price = serde_json::from_value(json!({
        "price":"100000000", "conf":"0", "expo":-8, "publish_time":now
    }))?;
    harness
        .set_mock_oracle_pyth_price(source.clone(), id, Some(price.clone()))
        .await?;
    harness
        .admin_set_proxy(
            proxy.clone(),
            id,
            Some(Proxy::median_low(
                [OracleRequest::pyth(source, id).into()],
                FreshnessFilter::new(Some(Nanoseconds::from_secs(3600)), None),
            )),
        )
        .await?;
    let client = harness.client()?;
    let cached = proxy_oracle::GetCachedPrice {
        oracle_id: proxy.clone(),
        id,
        age: 3600,
    };
    let live = oracle::GetPrice {
        oracle_id: proxy.clone(),
        price_id: id,
        age: 3600,
    };
    let resolved = client
        .read(live.clone())
        .await?
        .context("fixture source resolves")?;
    assert_eq!(resolved.price, price.price);
    assert!(client.read(cached.clone()).await?.is_none());
    harness.update_proxy_prices(proxy.clone(), vec![id]).await?;
    assert_eq!(client.read(cached.clone()).await?.as_ref(), Some(&price));
    for tripped in [true, false] {
        harness
            .call_function(
                &ManagedAccountId(proxy.clone()),
                &proxy,
                "admin_set_manual_trip",
                json!({
                    "id":id, "is_manually_tripped":tripped, "metadata":null
                }),
            )
            .await?;
        assert!(client.read(cached.clone()).await?.is_none());
        assert_eq!(client.read(live.clone()).await?.as_ref(), Some(&price));
    }
    // Clearing a trip does not make the cache usable until a fresh update succeeds.
    harness.update_proxy_prices(proxy, vec![id]).await?;
    assert_eq!(client.read(cached).await?, Some(price));
    Ok(())
}
