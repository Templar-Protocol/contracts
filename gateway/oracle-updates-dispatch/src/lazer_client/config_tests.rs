use std::time::Duration;

use super::*;

#[test]
fn defaults_channel_when_unset() {
    let config = LazerSourceConfig::new(
        "wss://example.com/v1/stream".parse().expect("valid URL"),
        RedactedString::from("secret-token"),
        LazerSubscriptionConfig {
            channel: None,
            max_payload_age: Duration::from_secs(5),
        },
    )
    .expect("valid config should use the default channel");

    assert_eq!(config.channel(), DEFAULT_CHANNEL);
}

#[test]
fn rejects_insecure_websocket_url() {
    let error = LazerSourceConfig::new(
        "ws://example.com/v1/stream".parse().expect("valid URL"),
        RedactedString::from("secret-token"),
        LazerSubscriptionConfig {
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
        RedactedString::from("  "),
        LazerSubscriptionConfig {
            channel: None,
            max_payload_age: Duration::from_secs(5),
        },
    )
    .expect_err("blank token should be rejected");

    assert!(matches!(error, LazerClientError::EmptyApiToken));
}

#[test]
fn rejects_invalid_channel() {
    let error = LazerSourceConfig::new(
        "wss://example.com/v1/stream".parse().expect("valid URL"),
        RedactedString::from("secret-token"),
        LazerSubscriptionConfig {
            channel: Some("not-a-channel".to_owned()),
            max_payload_age: Duration::from_secs(5),
        },
    )
    .expect_err("invalid channel should be rejected");

    assert!(
        matches!(error, LazerClientError::InvalidChannel(channel) if channel == "not-a-channel")
    );
}

#[test]
fn rejects_zero_max_payload_age() {
    let error = LazerSourceConfig::new(
        "wss://example.com/v1/stream".parse().expect("valid URL"),
        RedactedString::from("secret-token"),
        LazerSubscriptionConfig {
            channel: None,
            max_payload_age: Duration::ZERO,
        },
    )
    .expect_err("zero max payload age should be rejected");

    assert!(matches!(error, LazerClientError::InvalidMaxPayloadAge));
}
