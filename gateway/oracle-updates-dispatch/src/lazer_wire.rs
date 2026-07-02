use std::{collections::BTreeSet, io::Cursor};

use pyth_lazer_protocol::{message::SolanaMessage, payload::PayloadData};
use serde::{Deserialize, Serialize};

use crate::{LazerClientError, LazerResult, LazerSourceConfig};

const MAX_SOLANA_PAYLOAD_BYTES: usize = 1_048_576;
const MAX_SOLANA_PAYLOAD_HEX_CHARS: usize = MAX_SOLANA_PAYLOAD_BYTES * 2;
const MAX_SOLANA_PAYLOAD_BASE64_CHARS: usize = ((MAX_SOLANA_PAYLOAD_BYTES + 2) / 3) * 4;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeFrame<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    subscription_id: u32,
    price_feed_ids: Vec<u32>,
    properties: [&'static str; 6],
    formats: [&'static str; 1],
    channel: &'a str,
    ignore_invalid_feeds: bool,
}

pub(crate) fn subscription_frame(config: &LazerSourceConfig) -> LazerResult<String> {
    let frame = SubscribeFrame {
        frame_type: "subscribe",
        subscription_id: 1,
        price_feed_ids: config.price_feed_ids().iter().copied().collect(),
        properties: [
            "price",
            "confidence",
            "exponent",
            "emaPrice",
            "emaConfidence",
            "feedUpdateTimestamp",
        ],
        formats: ["solana"],
        channel: config.channel(),
        ignore_invalid_feeds: true,
    };
    serde_json::to_string(&frame).map_err(|error| LazerClientError::Decode(error.to_string()))
}

#[derive(Deserialize)]
struct StreamMessage {
    #[serde(rename = "type")]
    message_type: String,
    solana: Option<SolanaPayload>,
}

#[derive(Deserialize)]
struct SolanaPayload {
    data: String,
    encoding: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedLazerPayload {
    pub(crate) bytes: Vec<u8>,
    pub(crate) feed_ids: BTreeSet<u32>,
}

pub(crate) fn decode_stream_message(text: &str) -> LazerResult<Option<DecodedLazerPayload>> {
    let message = serde_json::from_str::<StreamMessage>(text)
        .map_err(|error| LazerClientError::Decode(error.to_string()))?;
    if message.message_type != "streamUpdated" {
        return Ok(None);
    }
    let solana = message
        .solana
        .ok_or(LazerClientError::MissingSolanaPayload)?;
    decode_solana_payload(&solana)
}

fn decode_solana_payload(payload: &SolanaPayload) -> LazerResult<Option<DecodedLazerPayload>> {
    let bytes = decode_solana_payload_bytes(payload)?;
    let feed_ids = parse_solana_feed_ids(&bytes)?;
    Ok(Some(DecodedLazerPayload { bytes, feed_ids }))
}

fn decode_solana_payload_bytes(payload: &SolanaPayload) -> LazerResult<Vec<u8>> {
    let bytes = match payload.encoding.as_deref().unwrap_or("hex") {
        "hex" => {
            reject_oversized_encoded_payload(payload.data.len(), MAX_SOLANA_PAYLOAD_HEX_CHARS)?;
            hex::decode(&payload.data).map_err(|error| LazerClientError::Decode(error.to_string()))
        }
        "base64" => {
            use base64::Engine as _;

            reject_oversized_encoded_payload(payload.data.len(), MAX_SOLANA_PAYLOAD_BASE64_CHARS)?;
            base64::engine::general_purpose::STANDARD
                .decode(&payload.data)
                .map_err(|error| LazerClientError::Decode(error.to_string()))
        }
        encoding => Err(LazerClientError::UnsupportedEncoding(encoding.to_owned())),
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
