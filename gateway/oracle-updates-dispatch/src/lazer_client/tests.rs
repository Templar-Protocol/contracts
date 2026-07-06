use std::time::Duration;

use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use pyth_lazer_protocol::api::{
    ErrorResponse, InvalidFeedSubscriptionDetails, JsonBinaryData, JsonBinaryEncoding, JsonUpdate,
    StreamUpdatedResponse, SubscribedResponse, SubscribedWithInvalidFeedIdsIgnoredResponse,
    SubscriptionErrorResponse, UnsubscribedResponse, WsResponse,
};
use pyth_lazer_protocol::message::SolanaMessage;
use pyth_lazer_protocol::PriceFeedId;
use templar_gateway_core::OraclePayloadSource;
use templar_pyth_pro_verifier::{verify_solana_update, Crypto, TrustedSigner, VerifyParams};
use tokio::time::Instant;

use super::*;
use crate::lazer_wire::{decode_stream_message, DecodedLazerPayload, LazerStreamEvent};

const PAYLOAD_001: &str = "uQEagohEipEVyTiNYf6VaHJFux40+GmgzXaVUuzszi4nJMWpoMH4WZB0W3SMzUM41gQlkeJYJDydLouwjUDVBbksHwqA78H0gMVhWvP7Zz1CKH6ZPan7w1BrbkHfoylQggwubBwBddPHk0AKB5JsVAYAAwUHAAAABgD4hPUFAAAAAAUwKwAAAAAAAAT4/wqlkfUFAAAAAAsGNgAAAAAAAAwBQAoHkmxUBgAIAAAABgAvcfQFAAAAAAVeLAAAAAAAAAT4/wpWYvQFAAAAAAt1NwAAAAAAAAwBQAoHkmxUBgABAAAABgAqfglX/QUAAAV2rg9cAQAAAAT4/wqgFefA+wUAAAv8FrtEAQAAAAwBQAoHkmxUBgAbAAAABgC9d+INAAAAAAU27QEAAAAAAAT4/wqQi9sNAAAAAAu07wAAAAAAAAwBQAoHkmxUBgAXAAAABgDQwVgBAAAAAAWSGAAAAAAAAAT4/wqs8FYBAAAAAAvgGAAAAAAAAAwBQAoHkmxUBgA=";
const EXPECTED_FEEDS: [u32; 5] = [7, 8, 1, 27, 23];
const PAYLOAD_001_TIMESTAMP_US: u64 = 1_781_675_143_400_000;

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
            channel: None,
            max_payload_age,
        },
    )
    .expect("valid config")
}

/// Build a `streamUpdated` frame from the protocol's own response types, serialized by their serde
/// impls — so the wire tags (`type`/`streamUpdated`/`solana`/`encoding`) come from the protocol
/// shape rather than hand-written strings, and a protocol change surfaces here at compile time.
fn stream_message(solana: JsonBinaryData) -> String {
    let response = WsResponse::StreamUpdated(StreamUpdatedResponse {
        subscription_id: SubscriptionId(1),
        payload: JsonUpdate {
            parsed: None,
            evm: None,
            solana: Some(solana),
            le_ecdsa: None,
            le_unsigned: None,
        },
    });
    serde_json::to_string(&response).expect("protocol response serializes")
}

fn decoded_payload(message: &str) -> DecodedLazerPayload {
    match decode_stream_message(message).expect("message should decode") {
        LazerStreamEvent::Payload(payload) => payload,
        event => panic!("expected streamUpdated payload event, got {event:?}"),
    }
}

fn fixture_bytes() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(PAYLOAD_001)
        .expect("fixture should be base64")
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
    let hex_payload = decoded_payload(&stream_message(JsonBinaryData {
        encoding: JsonBinaryEncoding::Hex,
        data: hex::encode(&fixture),
    }));
    let base64_payload = decoded_payload(&stream_message(JsonBinaryData {
        encoding: JsonBinaryEncoding::Base64,
        data: PAYLOAD_001.to_owned(),
    }));

    assert_eq!(hex_payload.bytes, fixture);
    assert_eq!(
        base64_payload.feed_ids,
        EXPECTED_FEEDS.into_iter().collect()
    );
}

#[test]
fn oversized_payload_returns_error_before_decode() {
    let message = stream_message(JsonBinaryData {
        encoding: JsonBinaryEncoding::Hex,
        data: "00".repeat(1_048_577),
    });

    let error = decode_stream_message(&message).expect_err("oversized payload should fail");

    assert!(matches!(error, LazerClientError::PayloadTooLarge));
}

#[test]
fn decoded_fixture_is_accepted_by_pyth_pro_verifier() {
    let raw = fixture_bytes();
    let message = stream_message(JsonBinaryData {
        encoding: JsonBinaryEncoding::Base64,
        data: PAYLOAD_001.to_owned(),
    });
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

#[test]
fn decodes_subscription_events_with_ids() {
    let cases = [
        (
            WsResponse::Subscribed(SubscribedResponse {
                subscription_id: SubscriptionId(3),
            }),
            LazerStreamEvent::Subscribed {
                subscription_id: SubscriptionId(3),
            },
        ),
        (
            WsResponse::Unsubscribed(UnsubscribedResponse {
                subscription_id: SubscriptionId(4),
            }),
            LazerStreamEvent::Unsubscribed {
                subscription_id: SubscriptionId(4),
            },
        ),
        (
            WsResponse::SubscriptionError(SubscriptionErrorResponse {
                subscription_id: SubscriptionId(5),
                error: "denied".to_owned(),
            }),
            LazerStreamEvent::SubscriptionError {
                subscription_id: SubscriptionId(5),
                error: "denied".to_owned(),
            },
        ),
        (
            WsResponse::Error(ErrorResponse {
                error: "bad request".to_owned(),
            }),
            LazerStreamEvent::Error {
                error: "bad request".to_owned(),
            },
        ),
    ];

    for (response, expected) in cases {
        let message = serde_json::to_string(&response).expect("protocol response serializes");
        let event = decode_stream_message(&message).expect("event should decode");

        assert_eq!(event, expected);
    }
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

#[tokio::test]
async fn cache_miss_returns_error() {
    let source = LazerPayloadSource::from_cached(test_config(Duration::from_secs(5)), None);

    let error = source
        .fetch_payload(&[7])
        .await
        .expect_err("empty cache should miss");

    // With dynamic subscriptions, when there's no background task running,
    // attempting to subscribe will fail with a request error
    assert!(matches!(error, LazerClientError::Request(_)));
}

#[tokio::test]
async fn stale_cache_returns_error() {
    let source = LazerPayloadSource::from_cached(
        test_config(Duration::from_secs(5)),
        Some(CachedPayload {
            payload: vec![1, 2, 3],
            feed_ids: [7, 8].into_iter().collect(),
            received_at: Instant::now() - Duration::from_secs(6),
        }),
    );

    let error = source
        .fetch_payload(&[7])
        .await
        .expect_err("stale payload should fail");

    assert!(matches!(error, LazerClientError::StalePayload));
}

#[tokio::test]
async fn fresh_cache_returns_payload_for_covered_feeds() {
    let source = LazerPayloadSource::from_cached(
        test_config(Duration::from_secs(5)),
        Some(CachedPayload {
            payload: vec![1, 2, 3],
            feed_ids: [7, 8].into_iter().collect(),
            received_at: Instant::now(),
        }),
    );

    let payload = source
        .fetch_payload(&[7, 8])
        .await
        .expect("fresh payload should be returned");

    assert_eq!(payload, vec![1, 2, 3]);
}

#[tokio::test]
async fn empty_request_returns_error() {
    let source = LazerPayloadSource::from_cached(
        test_config(Duration::from_secs(5)),
        Some(CachedPayload {
            payload: vec![1, 2, 3],
            feed_ids: [7, 8].into_iter().collect(),
            received_at: Instant::now(),
        }),
    );

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
#[ignore = "requires PYTH_LAZER_API_KEY/PYTH_PRO_API_KEY + PYTH_LAZER_FEED_IDS; captures a live fixture"]
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

    let token = std::env::var("PYTH_LAZER_API_KEY")
        .or_else(|_| std::env::var("PYTH_PRO_API_KEY"))
        .expect("set PYTH_LAZER_API_KEY or PYTH_PRO_API_KEY");
    let feed_ids = std::env::var("PYTH_LAZER_FEED_IDS")
        .expect("set PYTH_LAZER_FEED_IDS to comma-separated u32 feed ids")
        .split(',')
        .map(str::trim)
        .map(str::parse::<u32>)
        .collect::<Result<BTreeSet<_>, _>>()
        .expect("feed ids must be u32 values");
    let ws_url = std::env::var("PYTH_LAZER_WS_URL")
        .unwrap_or_else(|_| "wss://pyth-lazer-0.dourolabs.app/v1/stream".to_owned());

    let config = LazerSourceConfig::new(
        ws_url.parse().expect("valid Pyth Lazer websocket URL"),
        RedactedString::from(token),
        LazerSubscriptionConfig {
            channel: None,
            max_payload_age: Duration::from_secs(5),
        },
    )
    .expect("valid Lazer config");

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
