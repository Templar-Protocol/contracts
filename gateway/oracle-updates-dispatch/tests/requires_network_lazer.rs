use std::time::Duration;

use templar_gateway_core::OraclePayloadSource;
use templar_gateway_oracle_updates_dispatch::{
    LazerPayloadSource, LazerSourceConfig, LazerSubscriptionConfig,
};
use tokio::time::Instant;

#[tokio::test]
#[ignore = "requires PYTH_LAZER_API_KEY/PYTH_PRO_API_KEY and PYTH_LAZER_FEED_IDS"]
async fn requires_network_fetches_production_lazer_payload() {
    let token = std::env::var("PYTH_LAZER_API_KEY")
        .or_else(|_| std::env::var("PYTH_PRO_API_KEY"))
        .expect("set PYTH_LAZER_API_KEY or PYTH_PRO_API_KEY");
    let feed_ids = std::env::var("PYTH_LAZER_FEED_IDS")
        .expect("set PYTH_LAZER_FEED_IDS to comma-separated u32 feed ids")
        .split(',')
        .map(str::trim)
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .expect("feed ids must be u32 values");
    let url = std::env::var("PYTH_LAZER_WS_URL")
        .unwrap_or_else(|_| "wss://pyth-lazer-0.dourolabs.app/v1/stream".to_owned())
        .parse()
        .expect("valid Pyth Lazer websocket URL");
    let config = LazerSourceConfig::new(
        url,
        token,
        LazerSubscriptionConfig {
            price_feed_ids: feed_ids.clone(),
            channel: None,
            max_payload_age: Duration::from_secs(5),
        },
    )
    .expect("valid Lazer config");
    let source = LazerPayloadSource::spawn(config);

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(payload) = source.fetch_payload(&feed_ids).await {
            assert!(!payload.is_empty());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for Pyth Lazer payload"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
