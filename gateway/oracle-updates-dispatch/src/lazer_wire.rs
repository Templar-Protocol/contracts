use std::{collections::BTreeSet, io::Cursor};

use pyth_lazer_protocol::{
    api::{
        DeliveryFormat, Format, JsonBinaryData, JsonBinaryEncoding, SubscribeRequest,
        SubscriptionId, SubscriptionParams, SubscriptionParamsRepr, UnsubscribeRequest, WsRequest,
        WsResponse,
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

pub(crate) fn subscription_frame_for_feeds(
    config: &LazerSourceConfig,
    subscription_id: SubscriptionId,
    price_feed_ids: BTreeSet<u32>,
) -> LazerResult<String> {
    let params = SubscriptionParams::new(SubscriptionParamsRepr {
        price_feed_ids: Some(price_feed_ids.into_iter().map(PriceFeedId).collect()),
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
        subscription_id,
        params,
    });
    serde_json::to_string(&request).map_err(|error| LazerClientError::Decode(error.to_string()))
}

pub(crate) fn unsubscription_frame(subscription_id: SubscriptionId) -> LazerResult<String> {
    let request = WsRequest::Unsubscribe(UnsubscribeRequest { subscription_id });
    serde_json::to_string(&request).map_err(|error| LazerClientError::Decode(error.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedLazerPayload {
    pub(crate) subscription_id: SubscriptionId,
    pub(crate) bytes: Vec<u8>,
    pub(crate) feed_ids: BTreeSet<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LazerStreamEvent {
    Payload(DecodedLazerPayload),
    Subscribed {
        subscription_id: SubscriptionId,
    },
    SubscribedWithInvalidFeedIdsIgnored {
        subscription_id: SubscriptionId,
        subscribed_feed_ids: BTreeSet<u32>,
    },
    Unsubscribed {
        subscription_id: SubscriptionId,
    },
    SubscriptionError {
        subscription_id: SubscriptionId,
        error: String,
    },
    Error {
        error: String,
    },
}

pub(crate) fn decode_stream_message(text: &str) -> LazerResult<LazerStreamEvent> {
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
            decode_solana_payload(message.subscription_id, &solana).map(LazerStreamEvent::Payload)
        }
        WsResponse::Error(error) => Ok(LazerStreamEvent::Error { error: error.error }),
        WsResponse::SubscriptionError(error) => Ok(LazerStreamEvent::SubscriptionError {
            subscription_id: error.subscription_id,
            error: error.error,
        }),
        WsResponse::SubscribedWithInvalidFeedIdsIgnored(message) => {
            Ok(LazerStreamEvent::SubscribedWithInvalidFeedIdsIgnored {
                subscription_id: message.subscription_id,
                subscribed_feed_ids: message
                    .subscribed_feed_ids
                    .into_iter()
                    .map(|feed| feed.0)
                    .collect(),
            })
        }
        WsResponse::Subscribed(message) => Ok(LazerStreamEvent::Subscribed {
            subscription_id: message.subscription_id,
        }),
        WsResponse::Unsubscribed(message) => Ok(LazerStreamEvent::Unsubscribed {
            subscription_id: message.subscription_id,
        }),
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

fn decode_solana_payload(
    subscription_id: SubscriptionId,
    payload: &JsonBinaryData,
) -> LazerResult<DecodedLazerPayload> {
    let bytes = decode_solana_payload_bytes(payload)?;
    let feed_ids = parse_solana_feed_ids(&bytes)?;
    Ok(DecodedLazerPayload {
        subscription_id,
        bytes,
        feed_ids,
    })
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
mod tests;
