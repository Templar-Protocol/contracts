use std::io::Cursor;

use pyth_lazer_protocol::message::SolanaMessage;
use pyth_lazer_protocol::payload::{PayloadData, PayloadFeedData, PayloadPropertyValue};
use templar_primitives::Nanoseconds;

use crate::crypto::Crypto;
use crate::error::VerifyError;

/// A trusted publisher: the 32-byte ed25519 public key whose signatures are accepted, and the
/// unix-seconds instant after which that trust lapses (mirrors the Lazer contract's per-signer
/// `expiresAt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedSigner {
    pub public_key: [u8; 32],
    pub expires_at_s: u64,
}

/// The neutral, per-feed result of parsing a payload — preserves every Lazer property so callers
/// (e.g. the stateless `verify_update` view) get full data, not just the Pyth-compatible subset.
///
/// Each field is `None` when the property was absent (or, for prices, carried Lazer's zero
/// sentinel). NOTE: "not requested" and "requested but missing" both collapse to `None` here; the
/// official EVM contract distinguishes them via a tri-state map — a possible future enrichment.
/// Price-like values are raw `i64` mantissas (interpret with `exponent`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFeed {
    pub feed_id: u32,
    pub price: Option<i64>,
    pub best_bid_price: Option<i64>,
    pub best_ask_price: Option<i64>,
    pub publisher_count: Option<u16>,
    pub exponent: Option<i16>,
    pub confidence: Option<i64>,
    pub funding_rate: Option<i64>,
    pub funding_timestamp: Option<Nanoseconds>,
    pub funding_rate_interval: Option<Nanoseconds>,
    pub market_session: Option<i16>,
    pub ema_price: Option<i64>,
    pub ema_confidence: Option<i64>,
    pub feed_update_timestamp: Option<Nanoseconds>,
}

/// A successfully verified Lazer update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedUpdate {
    /// The trusted ed25519 signer public key that produced the signature.
    pub signer: [u8; 32],
    /// The channel the payload was published on.
    pub channel_id: u8,
    /// The payload-level timestamp.
    pub timestamp: Nanoseconds,
    /// One entry per feed present in the payload.
    pub feeds: Vec<ParsedFeed>,
}

/// Trust/freshness parameters for [`verify_solana_update`]. All durations are in whole seconds.
pub struct VerifyParams<'a> {
    pub trusted_signers: &'a [TrustedSigner],
    pub now_s: u64,
    pub max_timestamp_delay_s: u64,
    pub max_timestamp_ahead_s: u64,
    /// If `Some`, only payloads on this channel are accepted; `None` accepts any channel.
    pub allowed_channel_id: Option<u8>,
}

/// Verify and parse a Pyth Pro **solana**-format (ed25519) signed message. ("solana" is Pyth's
/// name for the format; the verification runs on NEAR, whose native scheme is ed25519.)
///
/// Checks, in order: envelope decode, trusted-and-unexpired signer (the pubkey is carried in the
/// envelope), ed25519 signature over the payload, payload decode (which also validates the inner
/// payload magic), channel filter, and freshness window.
pub fn verify_solana_update<C: Crypto>(
    crypto: &C,
    raw_message: &[u8],
    params: &VerifyParams<'_>,
) -> Result<VerifiedUpdate, VerifyError> {
    // Parse from a cursor and require it to reach the end — no trailing bytes (mirrors the official
    // Sui contract's `cursor.destroy_empty()`). Cheap: just the parse plus a position compare.
    let mut cursor = Cursor::new(raw_message);
    let message = SolanaMessage::deserialize(&mut cursor)
        .map_err(|e| VerifyError::Envelope(e.to_string()))?;
    if cursor.position() != raw_message.len() as u64 {
        return Err(VerifyError::TrailingBytes);
    }

    // The signer pubkey is carried in the envelope — trust it only if it's in the configured,
    // unexpired set, then confirm it actually produced the signature over the payload.
    let signer = message.public_key;
    let trusted = params
        .trusted_signers
        .iter()
        .any(|s| s.public_key == signer && s.expires_at_s > params.now_s);
    if !trusted {
        return Err(VerifyError::UntrustedSigner);
    }
    if !crypto.ed25519_verify(&message.signature, &message.payload, &signer) {
        return Err(VerifyError::Signature);
    }

    let mut payload_cursor = Cursor::new(message.payload.as_slice());
    let data = PayloadData::deserialize::<byteorder::LE>(&mut payload_cursor)
        .map_err(|e| VerifyError::Payload(e.to_string()))?;
    if payload_cursor.position() != message.payload.len() as u64 {
        return Err(VerifyError::TrailingBytes);
    }

    let channel_id = data.channel_id.0;
    if let Some(allowed) = params.allowed_channel_id {
        if channel_id != allowed {
            return Err(VerifyError::Channel { got: channel_id });
        }
    }

    // Freshness window is checked on the protocol type's whole seconds; the stored result carries
    // the unit in the type (`Nanoseconds`).
    let timestamp_secs = data.timestamp_us.as_secs();
    if timestamp_secs.saturating_add(params.max_timestamp_delay_s) < params.now_s {
        return Err(VerifyError::TimestampTooOld);
    }
    if timestamp_secs > params.now_s.saturating_add(params.max_timestamp_ahead_s) {
        return Err(VerifyError::TimestampTooFarAhead);
    }
    let timestamp = Nanoseconds::from_micros(data.timestamp_us.as_micros());

    let feeds = data.feeds.into_iter().map(parse_feed).collect();

    Ok(VerifiedUpdate {
        signer,
        channel_id,
        timestamp,
        feeds,
    })
}

fn parse_feed(feed: PayloadFeedData) -> ParsedFeed {
    let mut parsed = ParsedFeed {
        feed_id: feed.feed_id.0,
        price: None,
        best_bid_price: None,
        best_ask_price: None,
        publisher_count: None,
        exponent: None,
        confidence: None,
        funding_rate: None,
        funding_timestamp: None,
        funding_rate_interval: None,
        market_session: None,
        ema_price: None,
        ema_confidence: None,
        feed_update_timestamp: None,
    };

    for property in feed.properties {
        match property {
            PayloadPropertyValue::Price(Some(p)) => parsed.price = Some(p.mantissa_i64()),
            PayloadPropertyValue::BestBidPrice(Some(p)) => {
                parsed.best_bid_price = Some(p.mantissa_i64());
            }
            PayloadPropertyValue::BestAskPrice(Some(p)) => {
                parsed.best_ask_price = Some(p.mantissa_i64());
            }
            PayloadPropertyValue::PublisherCount(n) => parsed.publisher_count = Some(n),
            PayloadPropertyValue::Exponent(e) => parsed.exponent = Some(e),
            PayloadPropertyValue::Confidence(Some(p)) => parsed.confidence = Some(p.mantissa_i64()),
            PayloadPropertyValue::FundingRate(Some(r)) => parsed.funding_rate = Some(r.mantissa()),
            PayloadPropertyValue::FundingTimestamp(Some(t)) => {
                parsed.funding_timestamp = Some(Nanoseconds::from_micros(t.as_micros()));
            }
            PayloadPropertyValue::FundingRateInterval(Some(d)) => {
                parsed.funding_rate_interval = Some(Nanoseconds::from_micros(d.as_micros()));
            }
            PayloadPropertyValue::MarketSession(ms) => {
                parsed.market_session = Some(i16::from(ms));
            }
            PayloadPropertyValue::EmaPrice(Some(p)) => parsed.ema_price = Some(p.mantissa_i64()),
            PayloadPropertyValue::EmaConfidence(Some(p)) => {
                parsed.ema_confidence = Some(p.mantissa_i64());
            }
            PayloadPropertyValue::FeedUpdateTimestamp(Some(t)) => {
                parsed.feed_update_timestamp = Some(Nanoseconds::from_micros(t.as_micros()));
            }
            // `None`-sentinel price-likes leave the corresponding field as `None`.
            _ => {}
        }
    }

    parsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
    use pyth_lazer_protocol::message::SolanaMessage;
    use pyth_lazer_protocol::payload::{PayloadData, PayloadFeedData, PayloadPropertyValue};
    use pyth_lazer_protocol::time::TimestampUs;
    use pyth_lazer_protocol::{ChannelId, Price, PriceFeedId};

    /// Test double for [`Crypto`] using `ed25519-dalek`.
    struct TestCrypto;

    impl Crypto for TestCrypto {
        fn ed25519_verify(
            &self,
            signature: &[u8; 64],
            message: &[u8],
            public_key: &[u8; 32],
        ) -> bool {
            let Ok(key) = VerifyingKey::from_bytes(public_key) else {
                return false;
            };
            key.verify_strict(message, &Signature::from_bytes(signature))
                .is_ok()
        }
    }

    fn sample_payload(timestamp_us: u64, channel: u8) -> Vec<u8> {
        let data = PayloadData {
            timestamp_us: TimestampUs::from_micros(timestamp_us),
            channel_id: ChannelId(channel),
            feeds: vec![PayloadFeedData {
                feed_id: PriceFeedId(2),
                properties: vec![
                    PayloadPropertyValue::Price(Some(Price::from_mantissa(123_456).unwrap())),
                    PayloadPropertyValue::Confidence(Some(Price::from_mantissa(50).unwrap())),
                    PayloadPropertyValue::Exponent(-8),
                    PayloadPropertyValue::EmaPrice(Some(Price::from_mantissa(123_000).unwrap())),
                    PayloadPropertyValue::FeedUpdateTimestamp(Some(TimestampUs::from_micros(
                        timestamp_us,
                    ))),
                ],
            }],
        };
        let mut payload = Vec::new();
        data.serialize::<byteorder::LE>(&mut payload).unwrap();
        payload
    }

    fn signed_message(signing_key: &SigningKey, payload: &[u8]) -> Vec<u8> {
        let message = SolanaMessage {
            payload: payload.to_vec(),
            signature: signing_key.sign(payload).to_bytes(),
            public_key: signing_key.verifying_key().to_bytes(),
        };
        let mut raw = Vec::new();
        message.serialize(&mut raw).unwrap();
        raw
    }

    fn signer_for(signing_key: &SigningKey, expires_at_s: u64) -> TrustedSigner {
        TrustedSigner {
            public_key: signing_key.verifying_key().to_bytes(),
            expires_at_s,
        }
    }

    #[test]
    fn verifies_and_parses_a_well_formed_update() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let now_s = 1_700_000_000;
        let payload = sample_payload(now_s * 1_000_000, ChannelId::REAL_TIME.0);
        let raw = signed_message(&key, &payload);

        let signers = [signer_for(&key, now_s + 1000)];
        let params = VerifyParams {
            trusted_signers: &signers,
            now_s,
            max_timestamp_delay_s: 60,
            max_timestamp_ahead_s: 60,
            allowed_channel_id: Some(ChannelId::REAL_TIME.0),
        };

        let update = verify_solana_update(&TestCrypto, &raw, &params).unwrap();
        assert_eq!(update.channel_id, ChannelId::REAL_TIME.0);
        assert_eq!(
            update.timestamp,
            Nanoseconds::from_micros(now_s * 1_000_000)
        );
        assert_eq!(update.feeds.len(), 1);
        let feed = &update.feeds[0];
        assert_eq!(feed.feed_id, 2);
        assert_eq!(feed.price, Some(123_456));
        assert_eq!(feed.confidence, Some(50));
        assert_eq!(feed.exponent, Some(-8));
        assert_eq!(feed.ema_price, Some(123_000));
        assert_eq!(feed.ema_confidence, None);
        assert_eq!(
            feed.feed_update_timestamp,
            Some(Nanoseconds::from_micros(now_s * 1_000_000))
        );
    }

    #[test]
    fn duplicate_property_last_wins() {
        // `parse_feed` has no duplicate-property policy; the last occurrence wins. Pin that.
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let now_s = 1_700_000_000;
        let data = PayloadData {
            timestamp_us: TimestampUs::from_micros(now_s * 1_000_000),
            channel_id: ChannelId::REAL_TIME,
            feeds: vec![PayloadFeedData {
                feed_id: PriceFeedId(2),
                properties: vec![
                    PayloadPropertyValue::Price(Some(Price::from_mantissa(100).unwrap())),
                    PayloadPropertyValue::Price(Some(Price::from_mantissa(200).unwrap())),
                    PayloadPropertyValue::Exponent(-8),
                ],
            }],
        };
        let mut payload = Vec::new();
        data.serialize::<byteorder::LE>(&mut payload).unwrap();
        let raw = signed_message(&key, &payload);

        let signers = [signer_for(&key, now_s + 1000)];
        let params = VerifyParams {
            trusted_signers: &signers,
            now_s,
            max_timestamp_delay_s: 60,
            max_timestamp_ahead_s: 60,
            allowed_channel_id: None,
        };

        let update = verify_solana_update(&TestCrypto, &raw, &params).unwrap();
        assert_eq!(update.feeds[0].price, Some(200));
    }

    #[test]
    fn rejects_untrusted_signer() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let now_s = 1_700_000_000;
        let payload = sample_payload(now_s * 1_000_000, ChannelId::REAL_TIME.0);
        let raw = signed_message(&key, &payload);

        let signers = [signer_for(&other, now_s + 1000)];
        let params = VerifyParams {
            trusted_signers: &signers,
            now_s,
            max_timestamp_delay_s: 60,
            max_timestamp_ahead_s: 60,
            allowed_channel_id: None,
        };

        assert_eq!(
            verify_solana_update(&TestCrypto, &raw, &params),
            Err(VerifyError::UntrustedSigner)
        );
    }

    #[test]
    fn rejects_expired_signer() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let now_s = 1_700_000_000;
        let payload = sample_payload(now_s * 1_000_000, ChannelId::REAL_TIME.0);
        let raw = signed_message(&key, &payload);

        let signers = [signer_for(&key, now_s - 1)];
        let params = VerifyParams {
            trusted_signers: &signers,
            now_s,
            max_timestamp_delay_s: 60,
            max_timestamp_ahead_s: 60,
            allowed_channel_id: None,
        };

        assert_eq!(
            verify_solana_update(&TestCrypto, &raw, &params),
            Err(VerifyError::UntrustedSigner)
        );
    }

    #[test]
    fn rejects_stale_and_wrong_channel() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let now_s = 1_700_000_000;

        let stale_payload = sample_payload((now_s - 600) * 1_000_000, ChannelId::REAL_TIME.0);
        let stale_raw = signed_message(&key, &stale_payload);
        let signers = [signer_for(&key, now_s + 1000)];
        let params = VerifyParams {
            trusted_signers: &signers,
            now_s,
            max_timestamp_delay_s: 60,
            max_timestamp_ahead_s: 60,
            allowed_channel_id: None,
        };
        assert_eq!(
            verify_solana_update(&TestCrypto, &stale_raw, &params),
            Err(VerifyError::TimestampTooOld)
        );

        let payload = sample_payload(now_s * 1_000_000, ChannelId::FIXED_RATE_200.0);
        let raw = signed_message(&key, &payload);
        let params = VerifyParams {
            trusted_signers: &signers,
            now_s,
            max_timestamp_delay_s: 60,
            max_timestamp_ahead_s: 60,
            allowed_channel_id: Some(ChannelId::REAL_TIME.0),
        };
        assert_eq!(
            verify_solana_update(&TestCrypto, &raw, &params),
            Err(VerifyError::Channel {
                got: ChannelId::FIXED_RATE_200.0
            })
        );
    }

    fn params(signers: &[TrustedSigner], now_s: u64) -> VerifyParams<'_> {
        VerifyParams {
            trusted_signers: signers,
            now_s,
            max_timestamp_delay_s: 60,
            max_timestamp_ahead_s: 60,
            allowed_channel_id: None,
        }
    }

    #[test]
    fn rejects_trailing_envelope_bytes() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let now_s = 1_700_000_000;
        let mut raw = signed_message(
            &key,
            &sample_payload(now_s * 1_000_000, ChannelId::REAL_TIME.0),
        );
        raw.push(0xFF); // junk after the canonical envelope
        let signers = [signer_for(&key, now_s + 1000)];
        assert_eq!(
            verify_solana_update(&TestCrypto, &raw, &params(&signers, now_s)),
            Err(VerifyError::TrailingBytes)
        );
    }

    #[test]
    fn rejects_trailing_signed_payload_bytes() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let now_s = 1_700_000_000;
        // Trailing junk *inside* the signed payload: the envelope is canonical, but the payload
        // parser leaves bytes unconsumed.
        let mut payload = sample_payload(now_s * 1_000_000, ChannelId::REAL_TIME.0);
        payload.push(0xFF);
        let raw = signed_message(&key, &payload);
        let signers = [signer_for(&key, now_s + 1000)];
        assert_eq!(
            verify_solana_update(&TestCrypto, &raw, &params(&signers, now_s)),
            Err(VerifyError::TrailingBytes)
        );
    }

    #[test]
    fn preserves_non_pyth_properties() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let now_s = 1_700_000_000;
        let data = PayloadData {
            timestamp_us: TimestampUs::from_micros(now_s * 1_000_000),
            channel_id: ChannelId::REAL_TIME,
            feeds: vec![PayloadFeedData {
                feed_id: PriceFeedId(2),
                properties: vec![
                    PayloadPropertyValue::Price(Some(Price::from_mantissa(100).unwrap())),
                    PayloadPropertyValue::Exponent(-8),
                    PayloadPropertyValue::PublisherCount(7),
                    PayloadPropertyValue::BestBidPrice(Some(Price::from_mantissa(99).unwrap())),
                    PayloadPropertyValue::BestAskPrice(Some(Price::from_mantissa(101).unwrap())),
                ],
            }],
        };
        let mut payload = Vec::new();
        data.serialize::<byteorder::LE>(&mut payload).unwrap();
        let raw = signed_message(&key, &payload);
        let signers = [signer_for(&key, now_s + 1000)];

        let feed = &verify_solana_update(&TestCrypto, &raw, &params(&signers, now_s))
            .unwrap()
            .feeds[0];
        // Properties beyond the Pyth subset are retained, not dropped.
        assert_eq!(feed.publisher_count, Some(7));
        assert_eq!(feed.best_bid_price, Some(99));
        assert_eq!(feed.best_ask_price, Some(101));
    }
}
