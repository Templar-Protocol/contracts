use std::time::Duration;

use super::*;

#[test]
fn accepts_a_supported_channel() {
    let config = LazerSourceConfig::new(
        "wss://example.com/v1/stream".parse().expect("valid URL"),
        RedactedString::from("secret-token"),
        LazerSubscriptionConfig {
            channel: "fixed_rate@200ms".to_owned(),
            max_payload_age: Duration::from_secs(5),
        },
    )
    .expect("a supported channel should build");

    assert_eq!(
        config.channel(),
        Channel::FixedRate(pyth_lazer_protocol::time::FixedRate::RATE_200_MS)
    );
}

#[test]
fn rejects_insecure_websocket_url() {
    let error = LazerSourceConfig::new(
        "ws://example.com/v1/stream".parse().expect("valid URL"),
        RedactedString::from("secret-token"),
        LazerSubscriptionConfig {
            channel: "fixed_rate@200ms".to_owned(),
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
            channel: "fixed_rate@200ms".to_owned(),
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
            channel: "not-a-channel".to_owned(),
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
            channel: "fixed_rate@200ms".to_owned(),
            max_payload_age: Duration::ZERO,
        },
    )
    .expect_err("zero max payload age should be rejected");

    assert!(matches!(error, LazerClientError::InvalidMaxPayloadAge));
}
