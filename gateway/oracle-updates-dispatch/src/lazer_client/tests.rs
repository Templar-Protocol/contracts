use std::time::Duration;

use ed25519_dalek::{Signature, VerifyingKey};
use pyth_lazer_protocol::api::{
    ErrorResponse, InvalidFeedSubscriptionDetails, JsonBinaryData, JsonBinaryEncoding,
    SubscribedResponse, SubscribedWithInvalidFeedIdsIgnoredResponse, SubscriptionErrorResponse,
    UnsubscribedResponse, WsResponse,
};
use pyth_lazer_protocol::message::SolanaMessage;
use pyth_lazer_protocol::PriceFeedId;
use rstest::rstest;
use templar_gateway_core::OraclePayloadSource;
use templar_pyth_lazer_verifier::{verify_solana_update, Crypto, TrustedSigner, VerifyParams};
use tokio::time::Instant;

use super::*;
use crate::lazer_wire::{decode_stream_message, DecodedLazerPayload, LazerStreamEvent};

/// Read the live-network test environment — `PYTH_LAZER_API_KEY`,
/// `PYTH_LAZER_FEED_IDS`, and the optional `PYTH_LAZER_WS_URL`
/// — into a `LazerSourceConfig` and the requested feed ids. Shared by the two
/// `#[ignore]`d live tests below (`requires_network_fetches_production_lazer_payload`
/// and `capture_live_stream_updated_fixture`) so they read the same env and build the
/// same config and can't silently drift. Panics with a clear message on a missing or
/// malformed variable; only reached once those variables are set.
fn live_config_from_env() -> (LazerSourceConfig, Vec<u32>) {
    let token = std::env::var("PYTH_LAZER_API_KEY").expect("set PYTH_LAZER_API_KEY");
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
        RedactedString::from(token),
        LazerSubscriptionConfig {
            channel: "fixed_rate@200ms".to_owned(),
            max_payload_age: Duration::from_secs(5),
        },
    )
    .expect("valid Lazer config");
    (config, feed_ids)
}

use crate::lazer_client::fixtures::{
    fixture_bytes, stream_message, EXPECTED_FEEDS, PAYLOAD_001, PAYLOAD_001_TIMESTAMP_US,
};

struct TestCrypto;

impl Crypto for TestCrypto {
    fn ed25519_verify(&self, signature: &[u8; 64], message: &[u8], public_key: &[u8; 32]) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(public_key) else {
            return false;
        };
        key.verify_strict(message, &Signature::from_bytes(signature))
            .is_ok()
    }
}

fn test_config(max_payload_age: Duration) -> LazerSourceConfig {
    LazerSourceConfig::new(
        "wss://example.com/v1/stream".parse().expect("valid URL"),
        RedactedString::from("secret-token"),
        LazerSubscriptionConfig {
            channel: "fixed_rate@200ms".to_owned(),
            max_payload_age,
        },
    )
    .expect("valid config")
}

fn decoded_payload(message: &str) -> DecodedLazerPayload {
    match decode_stream_message(message).expect("message should decode") {
        LazerStreamEvent::Payload(payload) => payload,
        event => panic!("expected streamUpdated payload event, got {event:?}"),
    }
}

fn signer_of(raw: &[u8]) -> [u8; 32] {
    SolanaMessage::deserialize_slice(raw)
        .expect("fixture should be a Solana message")
        .public_key
}

fn verifier_params(trusted_signers: &[TrustedSigner]) -> VerifyParams<'_> {
    VerifyParams {
        trusted_signers,
        now_s: PAYLOAD_001_TIMESTAMP_US / 1_000_000,
        max_timestamp_delay_s: 60,
        max_timestamp_ahead_s: 60,
        allowed_channel_id: None,
    }
}

/// The exact `streamUpdated` envelope the production server sends, captured verbatim via
/// `capture_live_stream_updated_fixture`. The real frame carries `solana.encoding` explicitly, so
/// `decode_stream_message` parses it directly — proof the wire format needs no normalization.
#[test]
fn decodes_captured_stream_updated_fixture() {
    const FIXTURE: &str = include_str!("../../tests/fixtures/lazer_stream_updated.json");

    let payload = decoded_payload(FIXTURE);

    assert_eq!(payload.subscription_id, SubscriptionId(1));
    assert_eq!(payload.feed_ids, [7, 8].into_iter().collect());
}

#[test]
fn decodes_explicit_hex_and_base64() {
    let fixture = fixture_bytes();
    let hex_payload = decoded_payload(&stream_message(
        SubscriptionId(1),
        JsonBinaryData {
            encoding: JsonBinaryEncoding::Hex,
            data: hex::encode(&fixture),
        },
    ));
    let base64_payload = decoded_payload(&stream_message(
        SubscriptionId(1),
        JsonBinaryData {
            encoding: JsonBinaryEncoding::Base64,
            data: PAYLOAD_001.to_owned(),
        },
    ));

    assert_eq!(hex_payload.bytes, fixture);
    assert_eq!(
        base64_payload.feed_ids,
        EXPECTED_FEEDS.into_iter().collect()
    );
}

#[test]
fn oversized_payload_returns_error_before_decode() {
    let message = stream_message(
        SubscriptionId(1),
        JsonBinaryData {
            encoding: JsonBinaryEncoding::Hex,
            data: "00".repeat(1_048_577),
        },
    );

    let error = decode_stream_message(&message).expect_err("oversized payload should fail");

    assert!(matches!(error, LazerClientError::PayloadTooLarge));
}

#[test]
fn decoded_fixture_is_accepted_by_pyth_lazer_verifier() {
    let raw = fixture_bytes();
    let message = stream_message(
        SubscriptionId(1),
        JsonBinaryData {
            encoding: JsonBinaryEncoding::Base64,
            data: PAYLOAD_001.to_owned(),
        },
    );
    let decoded = decoded_payload(&message);
    let trusted_signers = [TrustedSigner {
        public_key: signer_of(&raw),
        expires_at_s: PAYLOAD_001_TIMESTAMP_US / 1_000_000 + 3_600,
    }];

    let verified = verify_solana_update(
        &TestCrypto,
        &decoded.bytes,
        &verifier_params(&trusted_signers),
    )
    .expect("decoded payload should verify");

    assert_eq!(
        verified
            .feeds
            .iter()
            .map(|feed| feed.feed_id)
            .collect::<Vec<_>>(),
        EXPECTED_FEEDS
    );
}

#[rstest]
#[case::subscribed(
    WsResponse::Subscribed(SubscribedResponse { subscription_id: SubscriptionId(3) }),
    LazerStreamEvent::Subscribed { subscription_id: SubscriptionId(3) },
)]
#[case::unsubscribed(
    WsResponse::Unsubscribed(UnsubscribedResponse { subscription_id: SubscriptionId(4) }),
    LazerStreamEvent::Unsubscribed { subscription_id: SubscriptionId(4) },
)]
#[case::subscription_error(
    WsResponse::SubscriptionError(SubscriptionErrorResponse {
        subscription_id: SubscriptionId(5),
        error: "denied".to_owned(),
    }),
    LazerStreamEvent::SubscriptionError {
        subscription_id: SubscriptionId(5),
        error: "denied".to_owned(),
    },
)]
#[case::stream_error(
    WsResponse::Error(ErrorResponse { error: "bad request".to_owned() }),
    LazerStreamEvent::Error { error: "bad request".to_owned() },
)]
fn decodes_subscription_events_with_ids(
    #[case] response: WsResponse,
    #[case] expected: LazerStreamEvent,
) {
    let message = serde_json::to_string(&response).expect("protocol response serializes");

    let event = decode_stream_message(&message).expect("event should decode");

    assert_eq!(event, expected);
}

#[test]
fn decodes_partial_subscription_acknowledgement() {
    let response = WsResponse::SubscribedWithInvalidFeedIdsIgnored(
        SubscribedWithInvalidFeedIdsIgnoredResponse {
            subscription_id: SubscriptionId(6),
            subscribed_feed_ids: vec![PriceFeedId(7), PriceFeedId(8)],
            ignored_invalid_feed_ids: InvalidFeedSubscriptionDetails {
                unknown_ids: vec![PriceFeedId(9)],
                unknown_symbols: vec![],
                unsupported_channels: vec![],
                unstable: vec![],
                not_entitled: vec![],
            },
        },
    );
    let message = serde_json::to_string(&response).expect("protocol response serializes");

    let event = decode_stream_message(&message).expect("event should decode");

    assert_eq!(
        event,
        LazerStreamEvent::SubscribedWithInvalidFeedIdsIgnored {
            subscription_id: SubscriptionId(6),
            subscribed_feed_ids: [7, 8].into_iter().collect(),
        }
    );
}

/// Rejected before the source is consulted, so it needs no stream. Cache
/// freshness, feed coverage, and subscription failures are covered against the
/// mock server in `actor_tests`.
#[tokio::test]
async fn empty_request_returns_error() {
    let source = LazerPayloadSource::spawn(test_config(Duration::from_secs(5)));

    let error = source
        .fetch_payload(&[])
        .await
        .expect_err("empty request should fail");

    assert!(matches!(error, LazerClientError::EmptyRequest));
}

/// Live capture harness: connect to the real Pyth Lazer stream exactly as production does (same
/// subscribe frame, bearer auth, TLS), save the first raw `streamUpdated` text frame verbatim to a
/// durable fixture, and assert it decodes through `decode_stream_message`. Ignored by default
/// (needs credentials + network); run it to (re)capture the fixture the offline tests assert
/// against and to confirm empirically whether the server includes `solana.encoding`.
///
/// ```sh
/// PYTH_LAZER_API_KEY=… PYTH_LAZER_FEED_IDS=7,8 \
///   cargo test -p templar-gateway-oracle-updates-dispatch \
///   -- --ignored --nocapture capture_live_stream_updated_fixture
/// ```
#[tokio::test]
#[ignore = "requires PYTH_LAZER_API_KEY + PYTH_LAZER_FEED_IDS; captures a live fixture"]
async fn capture_live_stream_updated_fixture() {
    use std::collections::BTreeSet;

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::{
        connect_async_with_config,
        tungstenite::{
            client::IntoClientRequest,
            http::{header::AUTHORIZATION, HeaderValue},
            protocol::WebSocketConfig,
            Message,
        },
    };

    use crate::lazer_wire::{subscription_frame_for_feeds, MAX_STREAM_JSON_MESSAGE_BYTES};

    let (config, feed_ids) = live_config_from_env();
    let feed_ids: BTreeSet<u32> = feed_ids.into_iter().collect();

    let mut request = config
        .ws_url
        .as_str()
        .into_client_request()
        .expect("valid websocket request");
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", config.api_token.as_ref()))
            .expect("valid bearer header"),
    );
    let (mut stream, _) = connect_async_with_config(
        request,
        Some(
            WebSocketConfig::default()
                .max_message_size(Some(MAX_STREAM_JSON_MESSAGE_BYTES))
                .max_frame_size(Some(MAX_STREAM_JSON_MESSAGE_BYTES)),
        ),
        false,
    )
    .await
    .expect("connect to Pyth Lazer");

    let frame = subscription_frame_for_feeds(&config, SubscriptionId(1), feed_ids)
        .expect("subscription frame serializes");
    stream
        .send(Message::Text(frame.into()))
        .await
        .expect("send subscribe frame");

    let deadline = Instant::now() + Duration::from_secs(20);
    let raw = loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for a streamUpdated frame"
        );
        let Some(message) = tokio::time::timeout(Duration::from_secs(20), stream.next())
            .await
            .expect("stream did not yield within the timeout")
        else {
            panic!("stream closed before any streamUpdated frame");
        };
        let Message::Text(text) = message.expect("stream error") else {
            continue;
        };
        // Skip subscription acks (subscribed/…); capture the first data frame.
        let is_stream_updated = serde_json::from_str::<serde_json::Value>(text.as_str())
            .ok()
            .and_then(|value| value.get("type")?.as_str().map(str::to_owned))
            .as_deref()
            == Some("streamUpdated");
        if is_stream_updated {
            break text.as_str().to_owned();
        }
    };

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lazer_stream_updated.json");
    std::fs::create_dir_all(path.parent().expect("fixture path has a parent"))
        .expect("create fixtures directory");
    std::fs::write(&path, &raw).expect("write captured fixture");
    println!("captured streamUpdated fixture -> {}", path.display());
    println!("{raw}");

    // The captured wire frame must decode through our path as-is.
    match decode_stream_message(&raw).expect("captured frame should decode") {
        LazerStreamEvent::Payload(payload) => assert!(!payload.bytes.is_empty()),
        event => panic!("expected a streamUpdated payload event, got {event:?}"),
    }
}

/// Live end-to-end smoke test of the production `LazerPayloadSource`: spawn the
/// actor and confirm it fetches a non-empty payload for the requested feeds against
/// the real Pyth Lazer stream. Ignored by default (needs credentials + network).
///
/// The first fetch must succeed on its own: it connects, subscribes, and waits for
/// the payload. A retry loop here would only mask a source that connects late.
#[tokio::test]
#[ignore = "requires PYTH_LAZER_API_KEY and PYTH_LAZER_FEED_IDS"]
async fn requires_network_fetches_production_lazer_payload() {
    let (config, feed_ids) = live_config_from_env();
    let source = LazerPayloadSource::spawn(config);

    let payload = source
        .fetch_payload(&feed_ids)
        .await
        .expect("the first fetch should connect, subscribe, and return a payload");

    assert!(!payload.is_empty());
}
