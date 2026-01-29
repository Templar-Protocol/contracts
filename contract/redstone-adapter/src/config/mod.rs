use near_sdk::AccountId;
use redstone::{ConfigFactory, SignerAddress, TimestampMillis};

#[cfg(not(feature = "agnostic-tests"))]
mod config_prod;
#[cfg(feature = "agnostic-tests")]
mod config_test;

#[cfg(not(feature = "agnostic-tests"))]
use config_prod as config_values;
#[cfg(feature = "agnostic-tests")]
use config_test as config_values;
use config_values::{
    SignerAddressBs, MAX_TIMESTAMP_AHEAD_MS, MAX_TIMESTAMP_DELAY_MS,
    REDSTONE_PRIMARY_PROD_ALLOWED_SIGNERS, SIGNER_COUNT, TRUSTED_UPDATERS, UPDATER_COUNT,
};

pub struct Config {
    pub signer_count_threshold: u8,
    pub signers: [SignerAddressBs; SIGNER_COUNT],
    pub trusted_updaters: [&'static str; UPDATER_COUNT],
    pub max_timestamp_delay_ms: u64,
    pub max_timestamp_ahead_ms: u64,
    pub min_interval_between_updates_ms: u64,
}

pub const DATA_STALENESS: TimestampMillis = TimestampMillis::from_millis(30 * 60 * 60 * 1000);

pub const FEED_TTL_SECS: u32 = 2 * 24 * 60 * 60;
pub const FEED_TTL_THRESHOLD: u32 = FEED_TTL_SECS / 5;
pub const FEED_TTL_EXTEND_TO: u32 = FEED_TTL_SECS * 3 / 10;

pub const STELLAR_CONFIG: Config = Config {
    signer_count_threshold: 3,
    signers: REDSTONE_PRIMARY_PROD_ALLOWED_SIGNERS,
    trusted_updaters: TRUSTED_UPDATERS,
    max_timestamp_ahead_ms: MAX_TIMESTAMP_AHEAD_MS,
    max_timestamp_delay_ms: MAX_TIMESTAMP_DELAY_MS,
    min_interval_between_updates_ms: 40_000,
};

pub struct NearCrypto;

impl redstone::Crypto for NearCrypto {
    type KeccakOutput = [u8; 32];

    fn keccak256(&mut self, input: impl AsRef<[u8]>) -> Self::KeccakOutput {
        near_sdk::env::keccak256_array(input.as_ref())
    }

    fn recover_public_key(
        &mut self,
        recovery_byte: u8,
        signature_bytes: impl AsRef<[u8]>,
        message_hash: Self::KeccakOutput,
    ) -> Result<redstone::Bytes, redstone::CryptoError> {
        near_sdk::env::ecrecover(&message_hash, signature_bytes.as_ref(), recovery_byte, true)
            .ok_or(redstone::CryptoError::RecoverPreHash)
            .map(|e| e.to_vec().into())
    }
}

impl ConfigFactory<(), NearCrypto> for Config {
    fn signer_count_threshold(&self) -> u8 {
        self.signer_count_threshold
    }

    fn redstone_signers(&self) -> Vec<SignerAddress> {
        self.signers.iter().map(|s| s.to_vec().into()).collect()
    }

    fn max_timestamp_delay_ms(&self) -> u64 {
        self.max_timestamp_delay_ms
    }

    fn max_timestamp_ahead_ms(&self) -> u64 {
        self.max_timestamp_ahead_ms
    }

    fn make_crypto(_: ()) -> NearCrypto {
        NearCrypto
    }
}
