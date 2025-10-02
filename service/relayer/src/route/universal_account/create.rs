use std::time::{Duration, SystemTime};

use axum::{extract::State, Json};
use near_primitives::hash::CryptoHash;
use near_sdk::serde::{Deserialize, Serialize};

use sha2::{Digest, Sha256};
use templar_universal_account::{
    authentication::{
        passkey::{self, Passkey},
        ExecutionContextProvider, Key,
    },
    Execute,
};

use crate::{app::App, route::SimpleResponse};

use super::pow::{Pow, PowTarget};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct CreatePasskeyAccount {
    pub key: Passkey,
    pub block_hash: CryptoHash,
}

impl PowTarget for CreatePasskeyAccount {
    fn pow_target(&self) -> String {
        format!("{},{}", &self.key.0, &self.block_hash)
    }
}

impl Execute for CreatePasskeyAccount {
    type Output = Self;

    fn execute(&self) -> Self::Output {
        self.clone()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub enum CreateRequest {
    Passkey(passkey::Message<Pow<CreatePasskeyAccount>>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub enum CreateResponse {}

pub async fn create(
    State(app): State<App>,
    Json(request): Json<CreateRequest>,
) -> SimpleResponse<CreateResponse> {
    let CreateRequest::Passkey(message) = request;

    let pow = message.payload();

    let actual_difficulty = pow.difficulty();
    let expected_difficulty = app.args.ua_create_pow_difficulty;
    if actual_difficulty < expected_difficulty {
        return SimpleResponse::Rejected { reason: format!("Difficulty requirement not reached: expected {expected_difficulty}, got {actual_difficulty}") };
    }

    let passkey = &pow.payload.key;
    let payload = match passkey.verify_signature(&message) {
        Ok(payload) => payload,
        Err(e) => {
            return SimpleResponse::Rejected {
                reason: format!("Invalid payload: {e}"),
            };
        }
    };

    // Check block timestamp (make sure signature is not too old)

    let block_hash = payload.block_hash;
    let Ok(block_timestamp_ms) = app.near.fetch_block_timestamp_ms(block_hash).await else {
        return SimpleResponse::Failure {
            error: "Failed to fetch block timestamp".to_string(),
        };
    };

    let Some(block_timestamp) =
        SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(block_timestamp_ms))
    else {
        return SimpleResponse::Failure {
            error: "Failed to calculate block age".to_string(),
        };
    };

    if !block_timestamp.elapsed().is_ok_and(|duration| {
        duration <= Duration::from_millis(app.args.ua_create_blockref_max_age_ms)
    }) {
        return SimpleResponse::Rejected {
            reason: "Block reference is too old".to_string(),
        };
    }

    // Check that account does not exist already

    let account_slug = hex::encode(&Sha256::digest(payload.key.0.to_sec1_bytes())[0..12]);

    todo!()
}
