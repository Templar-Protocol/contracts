use std::time::Duration;

use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use pyth_lazer_protocol::message::SolanaMessage;
use templar_gateway_core::OraclePayloadSource;
use templar_pyth_pro_verifier::{verify_solana_update, Crypto, TrustedSigner, VerifyParams};
use tokio::time::Instant;

use super::*;
use crate::lazer_wire::decode_stream_message;

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
        "secret-token".to_owned(),
        LazerSubscriptionConfig {
            price_feed_ids: vec![7, 8],
            channel: None,
            max_payload_age,
        },
    )
    .expect("valid config")
}

fn stream_message(solana: &serde_json::Value) -> String {
    serde_json::json!({
        "type": "streamUpdated",
        "subscriptionId": 1,
        "solana": solana,
    })
    .to_string()
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

#[test]
fn decodes_hex_when_encoding_is_omitted() {
    let fixture = fixture_bytes();
    let message = stream_message(&serde_json::json!({ "data": hex::encode(&fixture) }));

    let payload = decode_stream_message(&message)
        .expect("message should decode")
        .expect("streamUpdated should produce payload");

    assert_eq!(payload.bytes, fixture);
    assert_eq!(payload.feed_ids, EXPECTED_FEEDS.into_iter().collect());
}

#[test]
fn decodes_explicit_hex_and_base64() {
    let fixture = fixture_bytes();
    let hex_message = stream_message(&serde_json::json!({
        "encoding": "hex",
        "data": hex::encode(&fixture),
    }));
    let base64_message = stream_message(&serde_json::json!({
        "encoding": "base64",
        "data": PAYLOAD_001,
    }));

    let hex_payload = decode_stream_message(&hex_message)
        .expect("hex should decode")
        .expect("streamUpdated should produce payload");
    let base64_payload = decode_stream_message(&base64_message)
        .expect("base64 should decode")
        .expect("streamUpdated should produce payload");

    assert_eq!(hex_payload.bytes, fixture);
    assert_eq!(
        base64_payload.feed_ids,
        EXPECTED_FEEDS.into_iter().collect()
    );
}

#[test]
fn unsupported_encoding_returns_error() {
    let message = stream_message(&serde_json::json!({
        "encoding": "base58",
        "data": "abc",
    }));

    let error = decode_stream_message(&message).expect_err("base58 should be rejected");

    assert!(
        matches!(error, LazerClientError::UnsupportedEncoding(encoding) if encoding == "base58")
    );
}

#[test]
fn oversized_payload_returns_error_before_decode() {
    let message = stream_message(&serde_json::json!({
        "encoding": "hex",
        "data": "00".repeat(1_048_577),
    }));

    let error = decode_stream_message(&message).expect_err("oversized payload should fail");

    assert!(matches!(error, LazerClientError::PayloadTooLarge));
}

#[test]
fn decoded_fixture_is_accepted_by_pyth_pro_verifier() {
    let raw = fixture_bytes();
    let message = stream_message(&serde_json::json!({
        "encoding": "base64",
        "data": PAYLOAD_001,
    }));
    let decoded = decode_stream_message(&message)
        .expect("message should decode")
        .expect("streamUpdated should produce payload");
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

#[tokio::test]
async fn cache_miss_returns_error() {
    let source = LazerPayloadSource::from_cached(test_config(Duration::from_secs(5)), None);

    let error = source
        .fetch_payload(&[7])
        .await
        .expect_err("empty cache should miss");

    assert!(matches!(error, LazerClientError::CacheMiss));
}

#[tokio::test]
async fn requested_feed_outside_subscription_returns_error() {
    let source = LazerPayloadSource::from_cached(
        test_config(Duration::from_secs(5)),
        Some(CachedPayload {
            payload: vec![1, 2, 3],
            feed_ids: [7, 8].into_iter().collect(),
            received_at: Instant::now(),
        }),
    );

    let error = source
        .fetch_payload(&[9])
        .await
        .expect_err("uncovered feed should fail");

    assert!(matches!(error, LazerClientError::FeedNotCovered(9)));
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
async fn cached_payload_must_cover_requested_feed() {
    let source = LazerPayloadSource::from_cached(
        test_config(Duration::from_secs(5)),
        Some(CachedPayload {
            payload: vec![1, 2, 3],
            feed_ids: [7].into_iter().collect(),
            received_at: Instant::now(),
        }),
    );

    let error = source
        .fetch_payload(&[7, 8])
        .await
        .expect_err("payload missing feed 8 should fail");

    assert!(matches!(error, LazerClientError::FeedNotCovered(8)));
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
