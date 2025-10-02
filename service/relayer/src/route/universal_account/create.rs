use axum::{extract::State, response::IntoResponse, Json};
use near_primitives::hash::CryptoHash;
use near_sdk::{
    json_types::U64,
    serde::{Deserialize, Serialize},
};
use sha2::{Digest, Sha256};

use templar_universal_account::authentication::passkey::{self, Passkey};

use crate::app::App;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct CreatePasskey {
    pub key: Passkey,
    pub block_hash: CryptoHash,
    pub pow_nonce: U64,
}

impl CreatePasskey {
    pub fn pow_hash(key: &Passkey, block_hash: &CryptoHash, pow_nonce: u64) -> [u8; 32] {
        Sha256::digest(Sha256::digest(
            format!("{},{block_hash},{pow_nonce}", &key.0).as_bytes(),
        ))
        .into()
    }

    pub fn difficulty(hash: &[u8]) -> usize {
        let mut d = 0;
        for b in hash {
            if *b == 0 {
                d += 8;
            } else {
                d += b.leading_zeros() as usize;
                break;
            }
        }
        d
    }

    pub fn pow(
        key: Passkey,
        block_hash: CryptoHash,
        target_difficulty: usize,
        limit: u64,
    ) -> Option<Self> {
        let prefix = format!("{},{block_hash},", &key.0);
        let pow_nonce = (0u64..=limit).find(|nonce| {
            Self::difficulty(&Sha256::digest(Sha256::digest(
                format!("{prefix}{nonce}").as_bytes(),
            ))) >= target_difficulty
        })?;

        Some(Self {
            key,
            block_hash,
            pow_nonce: U64(pow_nonce),
        })
    }

    pub fn verify_pow(&self, difficulty: usize) -> bool {
        let hash = Self::pow_hash(&self.key, &self.block_hash, self.pow_nonce.0);
        let submitted_difficulty = Self::difficulty(&hash);
        submitted_difficulty >= difficulty
    }
}

#[test]
fn mine() {
    let passkey = Passkey("p256:S8avjv5zYFYhViXo7giqwynnMdox3RAytXQ7FG9a2tj8WxZnU6KUr36MSuUvgrwk4uGNMdiXt6vwtL9yBvj6VAUL".parse().unwrap());
    let block_hash = CryptoHash::hash_borsh("test7");
    let result = CreatePasskey::pow(passkey, block_hash, 17, 1_000_000).unwrap();
    println!("{result:?}");
    let hash = CreatePasskey::pow_hash(&result.key, &result.block_hash, result.pow_nonce.0);
    println!("{hash:?}");
    let difficulty = CreatePasskey::difficulty(&hash);
    println!("{difficulty}");
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub enum CreateRequest {
    Passkey(passkey::Message<CreatePasskey>),
}

pub struct CreateResponse {}

impl IntoResponse for CreateResponse {
    fn into_response(self) -> axum::response::Response {
        todo!()
    }
}

pub async fn create(State(app): State<App>, Json(request): Json<CreateRequest>) -> CreateResponse {
    todo!()
}
