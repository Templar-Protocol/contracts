use hex_literal::hex;

use super::{Config, SignerAddressBs};

pub const SIGNER_COUNT: usize = 5;

pub const MAX_TIMESTAMP_AHEAD_MS: u64 = 3 * 60 * 1_000;
pub const MAX_TIMESTAMP_DELAY_MS: u64 = 3 * 60 * 1_000;

pub const ALLOWED_SIGNERS: [SignerAddressBs; SIGNER_COUNT] = [
    hex!("8bb8f32df04c8b654987daaed53d6b6091e3b774"),
    hex!("deb22f54738d54976c4c0fe5ce6d408e40d88499"),
    hex!("51ce04be4b3e32572c4ec9135221d0691ba7d202"),
    hex!("dd682daec5a90dd295d14da4b0bec9281017b5be"),
    hex!("9c5ae89c4af6aa32ce58588dbaf90d18a855b6de"),
];

pub fn prod() -> Config {
    Config {
        signer_count_threshold: 3,
        signers: ALLOWED_SIGNERS.to_vec(),
        max_timestamp_ahead_ms: MAX_TIMESTAMP_AHEAD_MS,
        max_timestamp_delay_ms: MAX_TIMESTAMP_DELAY_MS,
        min_interval_between_updates_ms: 40_000,
    }
}
