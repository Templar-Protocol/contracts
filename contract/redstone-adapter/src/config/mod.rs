use near_sdk::near;
use redstone::{ConfigFactory, SignerAddress, TimestampMillis};

mod config_prod;
pub use config_prod::prod;
mod config_test;
pub use config_test::test;

pub type SignerAddressBs = [u8; 20];

#[derive(Debug, Clone)]
#[near(serializers = [borsh, json])]
pub struct Config {
    pub signer_count_threshold: u8,
    pub signers: Vec<SignerAddressBs>,
    pub max_timestamp_delay_ms: u64,
    pub max_timestamp_ahead_ms: u64,
    pub min_interval_between_updates_ms: u64,
}

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
        use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
        let signature_bytes = signature_bytes.as_ref();
        let signature = Signature::try_from(signature_bytes)
            .map_err(|_| redstone::CryptoError::Signature(signature_bytes.to_vec()))?;
        let r_id = RecoveryId::try_from(recovery_byte)
            .map_err(|_| redstone::CryptoError::RecoveryByte(recovery_byte))?;
        let result = VerifyingKey::recover_from_prehash(&message_hash, &signature, r_id)
            .map_err(|_| redstone::CryptoError::RecoverPreHash)?;

        Ok(result.to_encoded_point(false).to_bytes().to_vec().into())
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
