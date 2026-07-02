use std::time::Duration;

use super::*;

#[test]
fn rejects_insecure_websocket_url() {
    let error = LazerSourceConfig::new(
        "ws://example.com/v1/stream".parse().expect("valid URL"),
        "secret-token".to_owned(),
        LazerSubscriptionConfig {
            price_feed_ids: vec![7],
            channel: None,
            max_payload_age: Duration::from_secs(5),
        },
    )
    .expect_err("ws:// should be rejected");

    assert!(matches!(error, LazerClientError::InsecureWebSocketUrl));
}

#[test]
fn rejects_empty_api_token() {
    let error = LazerSourceConfig::new(
        "wss://example.com/v1/stream".parse().expect("valid URL"),
        "  ".to_owned(),
        LazerSubscriptionConfig {
            price_feed_ids: vec![7],
            channel: None,
            max_payload_age: Duration::from_secs(5),
        },
    )
    .expect_err("blank token should be rejected");

    assert!(matches!(error, LazerClientError::EmptyApiToken));
}

#[test]
fn rejects_empty_subscription() {
    let error = LazerSourceConfig::new(
        "wss://example.com/v1/stream".parse().expect("valid URL"),
        "secret-token".to_owned(),
        LazerSubscriptionConfig {
            price_feed_ids: vec![],
            channel: None,
            max_payload_age: Duration::from_secs(5),
        },
    )
    .expect_err("empty subscription should be rejected");

    assert!(matches!(error, LazerClientError::EmptySubscription));
}

#[test]
fn rejects_invalid_channel() {
    let error = LazerSourceConfig::new(
        "wss://example.com/v1/stream".parse().expect("valid URL"),
        "secret-token".to_owned(),
        LazerSubscriptionConfig {
            price_feed_ids: vec![7],
            channel: Some(String::new()),
            max_payload_age: Duration::from_secs(5),
        },
    )
    .expect_err("empty channel should be rejected");

    assert!(matches!(error, LazerClientError::InvalidChannel(channel) if channel.is_empty()));
}

#[test]
fn rejects_zero_max_payload_age() {
    let error = LazerSourceConfig::new(
        "wss://example.com/v1/stream".parse().expect("valid URL"),
        "secret-token".to_owned(),
        LazerSubscriptionConfig {
            price_feed_ids: vec![7],
            channel: None,
            max_payload_age: Duration::ZERO,
        },
    )
    .expect_err("zero max payload age should be rejected");

    assert!(matches!(error, LazerClientError::InvalidMaxPayloadAge));
}
