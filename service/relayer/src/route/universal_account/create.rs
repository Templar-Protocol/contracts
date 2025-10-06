use std::{
    str::FromStr,
    time::{Duration, SystemTime},
};

use axum::{extract::State, Json};
use near_jsonrpc_client::{
    errors::{JsonRpcError, JsonRpcServerError},
    methods::query::RpcQueryError,
};
use near_primitives::hash::CryptoHash;
use near_sdk::{
    serde::{Deserialize, Serialize},
    serde_json::json,
    AccountId,
};

use templar_universal_account::{
    authentication::{
        passkey::{self, Passkey},
        ExecutionContextProvider, Key,
    },
    Execute, KeyId,
};

use crate::{
    app::App,
    route::{universal_account::public_key_to_account_id_slug, SimpleResponse},
};

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
    let expected_difficulty = app.args.ua.pow_difficulty;
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
    let Ok(block_timestamp_ms) = app.ua_near.fetch_block_timestamp_ms(block_hash).await else {
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

    if !block_timestamp
        .elapsed()
        .is_ok_and(|duration| duration <= Duration::from_millis(app.args.ua.blockref_max_age_ms))
    {
        return SimpleResponse::Rejected {
            reason: "Block reference is too old".to_string(),
        };
    }

    // Check that account does not exist already

    let account_slug = public_key_to_account_id_slug(&payload.key.0.to_string());

    let registry_id = &app.args.ua.registry_id;
    let account_id = match AccountId::from_str(&format!("{account_slug}.{registry_id}")) {
        Ok(account_id) => account_id,
        Err(e) => {
            tracing::error!("Failed to construct account ID: {e}");
            return SimpleResponse::Failure {
                error: "Failed to construct account ID".to_string(),
            };
        }
    };

    match app.ua_near.fetch_near_balance(account_id.clone()).await {
        Err(JsonRpcError::ServerError(JsonRpcServerError::HandlerError(
            RpcQueryError::UnknownAccount { .. },
        ))) => { /* Account does not exist already: continue. */ }
        Ok(_) => {
            return SimpleResponse::Rejected {
                reason: "Account already exists".to_string(),
            };
        }
        Err(e) => {
            tracing::error!("Error detecting account existence: {e}");
            return SimpleResponse::Failure {
                error: "Failed to detect whether account exists".to_string(),
            };
        }
    }

    let signed_transaction = app
        .ua_near
        .construct_deploy_from_registry_transaction(
            &app.cache,
            app.args.ua.registry_id,
            account_slug,
            app.args.ua.version_key,
            json!({
                "key": KeyId::Passkey(payload.key),
            }),
            None,
        )
        .await;

    // app.ua_near.send_transaction(signed_transaction, TxExecutionStatus::Final).await;

    todo!()
}
