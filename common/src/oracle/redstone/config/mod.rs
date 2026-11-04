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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[near(serializers = [json])]
#[serde(remote = "Self", deny_unknown_fields)]
pub struct ConfigPatch {
    pub signer_count_threshold: Option<u8>,
    pub signers: Option<Vec<SignerAddressBs>>,
    pub max_timestamp_delay_ms: Option<u64>,
    pub max_timestamp_ahead_ms: Option<u64>,
    pub min_interval_between_updates_ms: Option<u64>,
}

// Serde's ordinary named-struct decoder also accepts positional arrays. Keep its
// field/null/unknown-field handling, but admit only a JSON object at this boundary.
impl<'de> near_sdk::serde::Deserialize<'de> for ConfigPatch {
    fn deserialize<D: near_sdk::serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> near_sdk::serde::de::Visitor<'de> for ObjectVisitor {
            type Value = ConfigPatch;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a configuration patch object")
            }

            fn visit_map<M: near_sdk::serde::de::MapAccess<'de>>(
                self,
                map: M,
            ) -> Result<Self::Value, M::Error> {
                ConfigPatch::deserialize(near_sdk::serde::de::value::MapAccessDeserializer::new(
                    map,
                ))
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

impl near_sdk::serde::Serialize for ConfigPatch {
    fn serialize<S: near_sdk::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Self::serialize(self, serializer)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigValidationError {
    #[error("configuration patch must supply at least one field")]
    EmptyPatch,
    #[error("signer_count_threshold must be greater than zero")]
    ZeroSignerThreshold,
    #[error(transparent)]
    InvalidSigners(#[from] redstone::network::error::Error),
}

impl Config {
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        validate_signer_policy(&self.signers, self.signer_count_threshold)
    }

    pub fn apply_patch(&mut self, patch: ConfigPatch) -> Result<(), ConfigValidationError> {
        if patch == ConfigPatch::default() {
            return Err(ConfigValidationError::EmptyPatch);
        }

        validate_signer_policy(
            patch.signers.as_deref().unwrap_or(&self.signers),
            patch
                .signer_count_threshold
                .unwrap_or(self.signer_count_threshold),
        )?;

        if let Some(value) = patch.signer_count_threshold {
            self.signer_count_threshold = value;
        }
        if let Some(value) = patch.signers {
            self.signers = value;
        }
        if let Some(value) = patch.max_timestamp_delay_ms {
            self.max_timestamp_delay_ms = value;
        }
        if let Some(value) = patch.max_timestamp_ahead_ms {
            self.max_timestamp_ahead_ms = value;
        }
        if let Some(value) = patch.min_interval_between_updates_ms {
            self.min_interval_between_updates_ms = value;
        }
        Ok(())
    }
}

fn validate_signer_policy(
    signers: &[SignerAddressBs],
    signer_count_threshold: u8,
) -> Result<(), ConfigValidationError> {
    if signer_count_threshold == 0 {
        return Err(ConfigValidationError::ZeroSignerThreshold);
    }
    let signers: Vec<SignerAddress> = signers.iter().map(|s| s.to_vec().into()).collect();
    redstone::contract::verification::verify_signers_config(&signers, signer_count_threshold)?;
    Ok(())
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
