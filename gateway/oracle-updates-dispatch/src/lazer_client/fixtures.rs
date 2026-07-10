//! A captured Pyth Lazer `streamUpdated` payload, shared by the wire-decoding
//! tests and the connection-lifecycle tests.

use base64::Engine as _;
use pyth_lazer_protocol::api::{
    JsonBinaryData, JsonBinaryEncoding, JsonUpdate, StreamUpdatedResponse, SubscriptionId,
    WsResponse,
};

pub(crate) const PAYLOAD_001: &str = "uQEagohEipEVyTiNYf6VaHJFux40+GmgzXaVUuzszi4nJMWpoMH4WZB0W3SMzUM41gQlkeJYJDydLouwjUDVBbksHwqA78H0gMVhWvP7Zz1CKH6ZPan7w1BrbkHfoylQggwubBwBddPHk0AKB5JsVAYAAwUHAAAABgD4hPUFAAAAAAUwKwAAAAAAAAT4/wqlkfUFAAAAAAsGNgAAAAAAAAwBQAoHkmxUBgAIAAAABgAvcfQFAAAAAAVeLAAAAAAAAAT4/wpWYvQFAAAAAAt1NwAAAAAAAAwBQAoHkmxUBgABAAAABgAqfglX/QUAAAV2rg9cAQAAAAT4/wqgFefA+wUAAAv8FrtEAQAAAAwBQAoHkmxUBgAbAAAABgC9d+INAAAAAAU27QEAAAAAAAT4/wqQi9sNAAAAAAu07wAAAAAAAAwBQAoHkmxUBgAXAAAABgDQwVgBAAAAAAWSGAAAAAAAAAT4/wqs8FYBAAAAAAvgGAAAAAAAAAwBQAoHkmxUBgA=";
pub(crate) const EXPECTED_FEEDS: [u32; 5] = [7, 8, 1, 27, 23];
pub(crate) const PAYLOAD_001_TIMESTAMP_US: u64 = 1_781_675_143_400_000;

pub(crate) fn fixture_bytes() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(PAYLOAD_001)
        .expect("fixture should be base64")
}

/// Build a `streamUpdated` frame from the protocol's own response types, serialized by their serde
/// impls — so the wire tags (`type`/`streamUpdated`/`solana`/`encoding`) come from the protocol
/// shape rather than hand-written strings, and a protocol change surfaces here at compile time.
pub(crate) fn stream_message(subscription_id: SubscriptionId, solana: JsonBinaryData) -> String {
    serde_json::to_string(&stream_response(subscription_id, solana))
        .expect("protocol response serializes")
}

pub(crate) fn stream_response(
    subscription_id: SubscriptionId,
    solana: JsonBinaryData,
) -> WsResponse {
    WsResponse::StreamUpdated(StreamUpdatedResponse {
        subscription_id,
        payload: JsonUpdate {
            parsed: None,
            evm: None,
            solana: Some(solana),
            le_ecdsa: None,
            le_unsigned: None,
        },
    })
}

/// The captured payload as a `streamUpdated` response for `subscription_id`.
pub(crate) fn fixture_response(subscription_id: SubscriptionId) -> WsResponse {
    stream_response(
        subscription_id,
        JsonBinaryData {
            encoding: JsonBinaryEncoding::Base64,
            data: PAYLOAD_001.to_owned(),
        },
    )
}
