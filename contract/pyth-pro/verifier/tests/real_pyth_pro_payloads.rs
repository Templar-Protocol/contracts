#![allow(clippy::unwrap_used)]

//! Regression tests over real Pyth Pro **solana**-format (ed25519) payloads captured offline from
//! the Lazer endpoint. The signer pubkey is carried in each envelope, so trust is established by
//! reading it out (no recovery).

use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use pyth_lazer_protocol::message::SolanaMessage;
use rstest::rstest;
use templar_primitives::Nanoseconds;
use templar_pyth_pro_verifier::{
    verify_solana_update, Crypto, TrustedSigner, VerifyError, VerifyParams,
};

const PAYLOAD_001: &str = "uQEagohEipEVyTiNYf6VaHJFux40+GmgzXaVUuzszi4nJMWpoMH4WZB0W3SMzUM41gQlkeJYJDydLouwjUDVBbksHwqA78H0gMVhWvP7Zz1CKH6ZPan7w1BrbkHfoylQggwubBwBddPHk0AKB5JsVAYAAwUHAAAABgD4hPUFAAAAAAUwKwAAAAAAAAT4/wqlkfUFAAAAAAsGNgAAAAAAAAwBQAoHkmxUBgAIAAAABgAvcfQFAAAAAAVeLAAAAAAAAAT4/wpWYvQFAAAAAAt1NwAAAAAAAAwBQAoHkmxUBgABAAAABgAqfglX/QUAAAV2rg9cAQAAAAT4/wqgFefA+wUAAAv8FrtEAQAAAAwBQAoHkmxUBgAbAAAABgC9d+INAAAAAAU27QEAAAAAAAT4/wqQi9sNAAAAAAu07wAAAAAAAAwBQAoHkmxUBgAXAAAABgDQwVgBAAAAAAWSGAAAAAAAAAT4/wqs8FYBAAAAAAvgGAAAAAAAAAwBQAoHkmxUBgA=";
const PAYLOAD_002: &str = "uQEagrbhXBQ0obffIhLeKsWy2+I5qMVP2dTi8kZYNjSc6yRRzcrMBXpvvZitAPKAe6Pao5xcNcg964VaW9rpiS61EgaA78H0gMVhWvP7Zz1CKH6ZPan7w1BrbkHfoylQggwubBwBddPHk4AXCpJsVAYAAwUHAAAABgDxhPUFAAAAAAU3KwAAAAAAAAT4/wqlkfUFAAAAAAsGNgAAAAAAAAwBgBcKkmxUBgAIAAAABgDXcfQFAAAAAAW2KwAAAAAAAAT4/wpaYvQFAAAAAAt1NwAAAAAAAAwBgBcKkmxUBgABAAAABgDz+QVX/QUAAAUn7ONkAQAAAAT4/wqAqevA+wUAAAukkbtEAQAAAAwBgBcKkmxUBgAbAAAABgC9d+INAAAAAAU27QEAAAAAAAT4/wqci9sNAAAAAAu27wAAAAAAAAwBgBcKkmxUBgAXAAAABgDQwVgBAAAAAAWSGAAAAAAAAAT4/wqz8FYBAAAAAAvgGAAAAAAAAAwBgBcKkmxUBgA=";
const PAYLOAD_003: &str = "uQEaggCqEFmq/DHcSpPiMlB1L1ebTDHQirAGoOF63nYAABPprL05D0YcyR34nzmaRh7/wRYQJe1IhEaxqGIy4rMm2wiA78H0gMVhWvP7Zz1CKH6ZPan7w1BrbkHfoylQggwubBwBddPHk8AkDZJsVAYAAwUHAAAABgDxhPUFAAAAAAU3KwAAAAAAAAT4/wqlkfUFAAAAAAsGNgAAAAAAAAwBwCQNkmxUBgAIAAAABgDXcfQFAAAAAAW2KwAAAAAAAAT4/wpbYvQFAAAAAAt0NwAAAAAAAAwBwCQNkmxUBgABAAAABgDy+QVX/QUAAAUO1cJhAQAAAAT4/wpgPfDA+wUAAAus/LtEAQAAAAwBwCQNkmxUBgAbAAAABgC9d+INAAAAAAXYFgIAAAAAAAT4/wqri9sNAAAAAAu47wAAAAAAAAwBwCQNkmxUBgAXAAAABgBlxlgBAAAAAAX9EwAAAAAAAAT4/wq78FYBAAAAAAvgGAAAAAAAAAwBwCQNkmxUBgA=";
const PAYLOAD_004: &str = "uQEagnoT9YVSg5gtkZHx0ejedhZoDwEePxhfEvQ5UbwjQxmgJmL9jgGRy6h+EaNjQwYHGCTKFqWZzrWd/ehgKSCytAuA78H0gMVhWvP7Zz1CKH6ZPan7w1BrbkHfoylQggwubBwBddPHkwAyEJJsVAYAAwUHAAAABgDxhPUFAAAAAAU3KwAAAAAAAAT4/wqmkfUFAAAAAAsFNgAAAAAAAAwBADIQkmxUBgAIAAAABgDXcfQFAAAAAAW2KwAAAAAAAAT4/wpdYvQFAAAAAAt0NwAAAAAAAAwBADIQkmxUBgABAAAABgDy+QVX/QUAAAUpCzhCAQAAAAT4/wrgV/bA+wUAAAu88btEAQAAAAwBADIQkmxUBgAbAAAABgC9d+INAAAAAAUH5gEAAAAAAAT4/wrEi9sNAAAAAAu67wAAAAAAAAwBADIQkmxUBgAXAAAABgB01FgBAAAAAAXjJgAAAAAAAAT4/wrA8FYBAAAAAAvgGAAAAAAAAAwBADIQkmxUBgA=";
const PAYLOAD_005: &str = "uQEags5APgOMCMAXqPRoS1putpV3DUQ5b2njS7XRh9JCkTvBJyr/El+UQkPVSjgfvB/l2mDY4O0BF/tNa/JWK/oHGg6A78H0gMVhWvP7Zz1CKH6ZPan7w1BrbkHfoylQggwubBwBddPHk0A/E5JsVAYAAwUHAAAABgDxhPUFAAAAAAU3KwAAAAAAAAT4/wqmkfUFAAAAAAsFNgAAAAAAAAwBQD8TkmxUBgAIAAAABgDacfQFAAAAAAWzKwAAAAAAAAT4/wpaYvQFAAAAAAt0NwAAAAAAAAwBQD8TkmxUBgABAAAABgCmfwpX/QUAAAVtZ99kAQAAAAT4/wogZfnA+wUAAAt0YbxEAQAAAAwBQD8TkmxUBgAbAAAABgC9d+INAAAAAAXYFgIAAAAAAAT4/wrXi9sNAAAAAAu87wAAAAAAAAwBQD8TkmxUBgAXAAAABgDv5FgBAAAAAAUfIwAAAAAAAAT4/wrE8FYBAAAAAAvgGAAAAAAAAAwBQD8TkmxUBgA=";

const EXPECTED_FEEDS: [u32; 5] = [7, 8, 1, 27, 23];

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

fn decode_fixture(payload_base64: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(payload_base64)
        .unwrap()
}

/// The signer pubkey is carried in the solana envelope — read it directly (no recovery).
fn signer_of(raw: &[u8]) -> [u8; 32] {
    SolanaMessage::deserialize_slice(raw).unwrap().public_key
}

fn params_for(trusted_signers: &[TrustedSigner], now_s: u64) -> VerifyParams<'_> {
    VerifyParams {
        trusted_signers,
        now_s,
        max_timestamp_delay_s: 60,
        max_timestamp_ahead_s: 60,
        allowed_channel_id: None,
    }
}

#[rstest]
#[case::payload_001(PAYLOAD_001, 1_781_675_143_400_000)]
#[case::payload_002(PAYLOAD_002, 1_781_675_143_600_000)]
#[case::payload_003(PAYLOAD_003, 1_781_675_143_800_000)]
#[case::payload_004(PAYLOAD_004, 1_781_675_144_000_000)]
#[case::payload_005(PAYLOAD_005, 1_781_675_144_200_000)]
fn verifies_real_pyth_pro_solana_payloads(
    #[case] payload_base64: &str,
    #[case] expected_timestamp_us: u64,
) {
    let raw = decode_fixture(payload_base64);
    let now_s = expected_timestamp_us / 1_000_000;
    let trusted_signers = [TrustedSigner {
        public_key: signer_of(&raw),
        expires_at_s: now_s + 3_600,
    }];
    let update =
        verify_solana_update(&TestCrypto, &raw, &params_for(&trusted_signers, now_s)).unwrap();

    let expected_timestamp = Nanoseconds::from_micros(expected_timestamp_us);
    assert_eq!(update.timestamp, expected_timestamp);
    assert_eq!(update.feeds.len(), EXPECTED_FEEDS.len());
    assert_eq!(
        update
            .feeds
            .iter()
            .map(|feed| feed.feed_id)
            .collect::<Vec<_>>(),
        EXPECTED_FEEDS
    );
    for feed in &update.feeds {
        assert!(
            feed.price.is_some(),
            "missing price for feed {}",
            feed.feed_id
        );
        assert!(
            feed.confidence.is_some(),
            "missing confidence for feed {}",
            feed.feed_id
        );
        assert!(
            feed.ema_price.is_some(),
            "missing EMA price for feed {}",
            feed.feed_id
        );
        assert!(
            feed.ema_confidence.is_some(),
            "missing EMA confidence for feed {}",
            feed.feed_id
        );
        assert_eq!(feed.exponent, Some(-8));
        assert_eq!(feed.feed_update_timestamp, Some(expected_timestamp));
    }
}

#[test]
fn rejects_real_payload_with_untrusted_signer() {
    let raw = decode_fixture(PAYLOAD_001);
    let now_s = 1_781_675_143_400_000 / 1_000_000;
    let trusted_signers = [TrustedSigner {
        public_key: [0x42; 32],
        expires_at_s: now_s + 3_600,
    }];

    assert_eq!(
        verify_solana_update(&TestCrypto, &raw, &params_for(&trusted_signers, now_s)),
        Err(VerifyError::UntrustedSigner)
    );
}

#[test]
fn rejects_real_payload_with_expired_signer() {
    let raw = decode_fixture(PAYLOAD_001);
    let now_s = 1_781_675_143_400_000 / 1_000_000;
    let trusted_signers = [TrustedSigner {
        public_key: signer_of(&raw),
        expires_at_s: now_s,
    }];

    assert_eq!(
        verify_solana_update(&TestCrypto, &raw, &params_for(&trusted_signers, now_s)),
        Err(VerifyError::UntrustedSigner)
    );
}

#[test]
fn rejects_real_payload_outside_timestamp_window() {
    let raw = decode_fixture(PAYLOAD_001);
    let timestamp_s = 1_781_675_143_400_000 / 1_000_000;
    let trusted_signers = [TrustedSigner {
        public_key: signer_of(&raw),
        expires_at_s: timestamp_s + 3_600,
    }];

    let old = verify_solana_update(
        &TestCrypto,
        &raw,
        &params_for(&trusted_signers, timestamp_s + 61),
    );
    assert_eq!(old, Err(VerifyError::TimestampTooOld));

    let ahead = verify_solana_update(
        &TestCrypto,
        &raw,
        &params_for(&trusted_signers, timestamp_s - 61),
    );
    assert_eq!(ahead, Err(VerifyError::TimestampTooFarAhead));
}

#[test]
fn rejects_real_payload_on_wrong_channel() {
    let raw = decode_fixture(PAYLOAD_001);
    let now_s = 1_781_675_143_400_000 / 1_000_000;
    let trusted_signers = [TrustedSigner {
        public_key: signer_of(&raw),
        expires_at_s: now_s + 3_600,
    }];
    let update =
        verify_solana_update(&TestCrypto, &raw, &params_for(&trusted_signers, now_s)).unwrap();
    let wrong_channel = update.channel_id ^ 1;
    let params = VerifyParams {
        trusted_signers: &trusted_signers,
        now_s,
        max_timestamp_delay_s: 60,
        max_timestamp_ahead_s: 60,
        allowed_channel_id: Some(wrong_channel),
    };

    assert_eq!(
        verify_solana_update(&TestCrypto, &raw, &params),
        Err(VerifyError::Channel {
            got: update.channel_id
        })
    );
}

#[test]
fn rejects_corrupted_real_payload_envelope() {
    let fixture = decode_fixture(PAYLOAD_001);
    let mut raw = fixture.clone();
    raw.truncate(raw.len() - 1);
    let now_s = 1_781_675_143_400_000 / 1_000_000;
    let trusted_signers = [TrustedSigner {
        public_key: signer_of(&fixture),
        expires_at_s: now_s + 3_600,
    }];

    assert!(verify_solana_update(&TestCrypto, &raw, &params_for(&trusted_signers, now_s)).is_err());
}

#[test]
fn rejects_tampered_real_payload_signature() {
    let raw = decode_fixture(PAYLOAD_001);
    let mut message = SolanaMessage::deserialize_slice(&raw).unwrap();
    // Flip a signature bit; the carried (trusted) pubkey is unchanged, so this fails the ed25519
    // check rather than the trust check.
    message.signature[0] ^= 0x01;
    let mut tampered = Vec::new();
    message.serialize(&mut tampered).unwrap();
    let now_s = 1_781_675_143_400_000 / 1_000_000;
    let trusted_signers = [TrustedSigner {
        public_key: signer_of(&raw),
        expires_at_s: now_s + 3_600,
    }];

    assert_eq!(
        verify_solana_update(&TestCrypto, &tampered, &params_for(&trusted_signers, now_s)),
        Err(VerifyError::Signature)
    );
}
