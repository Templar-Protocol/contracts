use near_sdk::near;
use redstone::{ConfigFactory, SignerAddress, TimestampMillis};

mod config_prod;
pub use config_prod::prod;
mod config_test;
pub use config_test::test;

pub type SignerAddressBs = [u8; 20];

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct Config {
    pub signer_count_threshold: u8,
    pub signers: Vec<SignerAddressBs>,
    pub max_timestamp_delay_ms: u64,
    pub max_timestamp_ahead_ms: u64,
    pub min_interval_between_updates_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigValidationError {
    #[error("signer_count_threshold must be greater than zero")]
    ZeroSignerThreshold,
    #[error(transparent)]
    InvalidSigners(#[from] redstone::network::error::Error),
}

impl Config {
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        if self.signer_count_threshold == 0 {
            return Err(ConfigValidationError::ZeroSignerThreshold);
        }
        redstone::contract::verification::verify_signers_config(
            &self.redstone_signers(),
            self.signer_count_threshold,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

pub const DATA_STALENESS: TimestampMillis = TimestampMillis::from_millis(30 * 60 * 60 * 1000);

pub const FEED_TTL_SECS: u32 = 2 * 24 * 60 * 60;
pub const FEED_TTL_THRESHOLD: u32 = FEED_TTL_SECS / 5;
pub const FEED_TTL_EXTEND_TO: u32 = FEED_TTL_SECS * 3 / 10;

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
        let signature = signature_bytes.as_ref();
        // The host aborts the receipt on a signature that is not 64 bytes or a
        // recovery byte above 3. Report both as errors instead.
        let signature: &[u8; 64] = signature
            .try_into()
            .map_err(|_| redstone::CryptoError::InvalidSignatureLen(signature.len()))?;
        if recovery_byte > 3 {
            return Err(redstone::CryptoError::RecoveryByte(recovery_byte));
        }

        // Recovered by the host: the same curve arithmetic compiled into the
        // contract costs about 18 Tgas a signature, the host call 0.28 Tgas.
        // `true` rejects a high `s`, which `recover_address` refuses before it calls this.
        let key = near_sdk::env::ecrecover(&message_hash, signature, recovery_byte, true)
            .ok_or(redstone::CryptoError::RecoverPreHash)?;

        // The host returns the point without the SEC1 tag the SDK expects first.
        Ok([&[0x04][..], &key[..]].concat().into())
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

    fn make_crypto((): ()) -> NearCrypto {
        NearCrypto
    }
}
