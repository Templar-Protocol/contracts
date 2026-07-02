use std::{collections::BTreeSet, io::Cursor};

use pyth_lazer_protocol::{
    api::{
        DeliveryFormat, Format, JsonBinaryData, JsonBinaryEncoding, SubscribeRequest,
        SubscriptionId, SubscriptionParams, SubscriptionParamsRepr, WsRequest, WsResponse,
    },
    message::SolanaMessage,
    payload::PayloadData,
    PriceFeedId, PriceFeedProperty,
};
use serde_json::Value;

use crate::{LazerClientError, LazerResult, LazerSourceConfig};

const JSON_WRAPPER_BYTES: usize = 4_096;
const MAX_SOLANA_PAYLOAD_BYTES: usize = 1_048_576;
const MAX_SOLANA_PAYLOAD_HEX_CHARS: usize = MAX_SOLANA_PAYLOAD_BYTES * 2;
const MAX_SOLANA_PAYLOAD_BASE64_CHARS: usize = MAX_SOLANA_PAYLOAD_BYTES.div_ceil(3) * 4;
pub(crate) const MAX_STREAM_JSON_MESSAGE_BYTES: usize =
    MAX_SOLANA_PAYLOAD_HEX_CHARS + JSON_WRAPPER_BYTES;

pub(crate) fn subscription_frame(config: &LazerSourceConfig) -> LazerResult<String> {
    let params = SubscriptionParams::new(SubscriptionParamsRepr {
        price_feed_ids: Some(
            config
                .price_feed_ids()
                .iter()
                .copied()
                .map(PriceFeedId)
                .collect(),
        ),
        symbols: None,
        properties: vec![
            PriceFeedProperty::Price,
            PriceFeedProperty::Confidence,
            PriceFeedProperty::Exponent,
            PriceFeedProperty::EmaPrice,
            PriceFeedProperty::EmaConfidence,
            PriceFeedProperty::FeedUpdateTimestamp,
        ],
        formats: vec![Format::Solana],
        delivery_format: DeliveryFormat::Json,
        json_binary_encoding: JsonBinaryEncoding::Base64,
        parsed: false,
        channel: config.channel(),
        ignore_invalid_feeds: true,
    })
    .map_err(|error| LazerClientError::Decode(error.to_owned()))?;
    let request = WsRequest::Subscribe(SubscribeRequest {
        subscription_id: SubscriptionId(1),
        params,
    });
    serde_json::to_string(&request).map_err(|error| LazerClientError::Decode(error.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedLazerPayload {
    pub(crate) bytes: Vec<u8>,
    pub(crate) feed_ids: BTreeSet<u32>,
}

pub(crate) fn decode_stream_message(text: &str) -> LazerResult<Option<DecodedLazerPayload>> {
    let mut value = serde_json::from_str::<Value>(text)
        .map_err(|error| LazerClientError::Decode(error.to_string()))?;
    normalize_solana_encoding(&mut value)?;
    match serde_json::from_value::<WsResponse>(value)
        .map_err(|error| LazerClientError::Decode(error.to_string()))?
    {
        WsResponse::StreamUpdated(message) => {
            let solana = message
                .payload
                .solana
                .ok_or(LazerClientError::MissingSolanaPayload)?;
            decode_solana_payload(&solana)
        }
        WsResponse::Error(_)
        | WsResponse::Subscribed(_)
        | WsResponse::SubscribedWithInvalidFeedIdsIgnored(_)
        | WsResponse::Unsubscribed(_)
        | WsResponse::SubscriptionError(_) => Ok(None),
    }
}

fn normalize_solana_encoding(value: &mut Value) -> LazerResult<()> {
    let Some(message) = value.as_object_mut() else {
        return Ok(());
    };
    if message.get("type").and_then(Value::as_str) != Some("streamUpdated") {
        return Ok(());
    }
    let Some(solana) = message.get_mut("solana").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    match solana.get("encoding").and_then(Value::as_str) {
        Some("hex" | "base64") => Ok(()),
        Some(encoding) => Err(LazerClientError::UnsupportedEncoding(encoding.to_owned())),
        None => {
            solana.insert("encoding".to_owned(), Value::String("hex".to_owned()));
            Ok(())
        }
    }
}

fn decode_solana_payload(payload: &JsonBinaryData) -> LazerResult<Option<DecodedLazerPayload>> {
    let bytes = decode_solana_payload_bytes(payload)?;
    let feed_ids = parse_solana_feed_ids(&bytes)?;
    Ok(Some(DecodedLazerPayload { bytes, feed_ids }))
}

fn decode_solana_payload_bytes(payload: &JsonBinaryData) -> LazerResult<Vec<u8>> {
    let bytes = match payload.encoding {
        JsonBinaryEncoding::Hex => {
            reject_oversized_encoded_payload(payload.data.len(), MAX_SOLANA_PAYLOAD_HEX_CHARS)?;
            hex::decode(&payload.data).map_err(|error| LazerClientError::Decode(error.to_string()))
        }
        JsonBinaryEncoding::Base64 => {
            use base64::Engine as _;

            reject_oversized_encoded_payload(payload.data.len(), MAX_SOLANA_PAYLOAD_BASE64_CHARS)?;
            base64::engine::general_purpose::STANDARD
                .decode(&payload.data)
                .map_err(|error| LazerClientError::Decode(error.to_string()))
        }
    }?;
    if bytes.len() > MAX_SOLANA_PAYLOAD_BYTES {
        return Err(LazerClientError::PayloadTooLarge);
    }
    Ok(bytes)
}

fn reject_oversized_encoded_payload(actual: usize, max: usize) -> LazerResult<()> {
    if actual > max {
        return Err(LazerClientError::PayloadTooLarge);
    }
    Ok(())
}

fn parse_solana_feed_ids(raw_message: &[u8]) -> LazerResult<BTreeSet<u32>> {
    let mut cursor = Cursor::new(raw_message);
    let message = SolanaMessage::deserialize(&mut cursor)
        .map_err(|error| LazerClientError::Decode(error.to_string()))?;
    if cursor.position() != raw_message.len() as u64 {
        return Err(LazerClientError::Decode(
            "trailing bytes in Solana payload".to_owned(),
        ));
    }

    let mut payload_cursor = Cursor::new(message.payload.as_slice());
    let payload = PayloadData::deserialize::<byteorder::LE>(&mut payload_cursor)
        .map_err(|error| LazerClientError::Decode(error.to_string()))?;
    if payload_cursor.position() != message.payload.len() as u64 {
        return Err(LazerClientError::Decode(
            "trailing bytes in Lazer payload".to_owned(),
        ));
    }

    Ok(payload
        .feeds
        .into_iter()
        .map(|feed| feed.feed_id.0)
        .collect())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pyth_lazer_protocol::{api::Channel, time::FixedRate};

    use super::*;
    use crate::LazerSubscriptionConfig;

    #[test]
    fn subscription_frame_uses_protocol_request_types() {
        let config = LazerSourceConfig::new(
            "wss://example.com/v1/stream".parse().expect("valid URL"),
            "secret-token".to_owned(),
            LazerSubscriptionConfig {
                price_feed_ids: vec![8, 7],
                channel: None,
                max_payload_age: Duration::from_secs(5),
            },
        )
        .expect("valid Lazer config");

        let frame = subscription_frame(&config).expect("frame should serialize");
        let request = serde_json::from_str::<WsRequest>(&frame).expect("protocol request");

        match request {
            WsRequest::Subscribe(request) => {
                assert_eq!(request.subscription_id, SubscriptionId(1));
                assert_eq!(
                    request.params.price_feed_ids,
                    Some(vec![PriceFeedId(7), PriceFeedId(8)])
                );
                assert_eq!(request.params.symbols, None);
                assert_eq!(request.params.formats, vec![Format::Solana]);
                assert_eq!(request.params.delivery_format, DeliveryFormat::Json);
                assert_eq!(
                    request.params.json_binary_encoding,
                    JsonBinaryEncoding::Base64
                );
                assert_eq!(
                    request.params.channel,
                    Channel::FixedRate(FixedRate::RATE_200_MS)
                );
                assert!(request.params.ignore_invalid_feeds);
            }
            WsRequest::Unsubscribe(_) => panic!("expected subscribe request"),
        }
    }
}
