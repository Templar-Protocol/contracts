use near_sdk::{
    json_types::U64,
    serde::{Deserialize, Serialize},
};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct Pow<T> {
    pow_nonce: U64,
    #[serde(flatten)]
    payload: T,
}

pub trait PowTarget {
    fn pow_target(&self) -> String;
}

fn leading_zeros(array: &[u8]) -> usize {
    let mut total = 0;
    for b in array {
        if *b == 0 {
            total += 8;
        } else {
            total += b.leading_zeros() as usize;
            break;
        }
    }
    total
}

#[derive(Debug, thiserror::Error)]
#[error("Proof-of-work does not satisfy target difficulty: actual {actual} < target {target}")]
pub struct FailsTargetDifficulty {
    pub actual: usize,
    pub target: usize,
}

impl<T: PowTarget> Pow<T> {
    fn target_hash(payload: &T) -> String {
        hex::encode(Sha256::digest(payload.pow_target()))
    }

    pub fn mine(payload: T, target_difficulty: usize, limit: u64) -> Option<Self> {
        let target_hash = Self::target_hash(&payload);
        let pow_nonce = (0u64..=limit).find(|nonce| {
            leading_zeros(&Sha256::digest(Sha256::digest(
                format!("{target_hash}{nonce}").as_bytes(),
            ))) >= target_difficulty
        })?;

        Some(Self {
            payload,
            pow_nonce: U64(pow_nonce),
        })
    }

    pub fn difficulty(&self) -> usize {
        let target_hash = Self::target_hash(&self.payload);
        let nonce = self.pow_nonce.0;
        leading_zeros(&Sha256::digest(Sha256::digest(
            format!("{target_hash}{nonce}").as_bytes(),
        )))
    }

    /// Verifies that the proof-of-work satisfies the target difficulty
    /// requirement.
    ///
    /// # Errors
    ///
    /// - If the payload does not satisfy the target difficulty.
    pub fn verify_pow(&self, target: usize) -> Result<&T, FailsTargetDifficulty> {
        let actual = self.difficulty();
        if actual >= target {
            Ok(&self.payload)
        } else {
            Err(FailsTargetDifficulty { actual, target })
        }
    }

    /// Gets the payload without checking the proof-of-work.
    pub fn payload_unchecked(&self) -> &T {
        &self.payload
    }
}
