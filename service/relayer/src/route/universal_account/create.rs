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
    AccountId, NearToken,
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
pub struct CreateResponse {
    pub account_id: AccountId,
    pub transaction_hash: CryptoHash,
}

#[allow(clippy::too_many_lines)]
pub async fn create(
    State(app): State<App>,
    Json(request): Json<CreateRequest>,
) -> SimpleResponse<CreateResponse> {
    let CreateRequest::Passkey(message) = request;

    // Verify PoW

    let payload = match message.payload().verify_pow(app.args.ua.pow_difficulty) {
        Ok(p) => p,
        Err(e) => {
            return SimpleResponse::Rejected {
                reason: e.to_string(),
            };
        }
    };

    // Verify signature

    let passkey = &payload.key;
    let payload = match passkey.verify_signature(&message) {
        Ok(p) => p,
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
        .is_ok_and(|duration| duration <= app.args.ua.blockref_max_age)
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

    // Check that account does not exist by fetching the balance and looking
    // for "unknown account" error.
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

    // Create transaction.
    let signed_transaction = app
        .ua_near
        .construct_deploy_from_registry_transaction(
            &app.cache,
            app.args.ua.registry_id.clone(),
            account_slug,
            app.args.ua.version_key.clone(),
            json!({
                "key": KeyId::Passkey(payload.key),
            }),
            None,
        )
        .await;

    // NOTE: This only counts gas from function calls, but this is OK, because
    // the deploy-from-registy transaction is a function call.
    let gas_estimate = signed_transaction
        .transaction
        .actions()
        .iter()
        .map(|a| a.get_prepaid_gas())
        .sum();

    let Some(gas_cost_estimate) = app.estimate_cost_of_gas(gas_estimate).await else {
        return SimpleResponse::Failure {
            error: "Gas cost estimation failure".to_string(),
        };
    };

    let transaction_hash = signed_transaction.get_hash();

    let resolve = match app
        .send_and_resolve_transaction(
            account_id.clone(),
            gas_cost_estimate,
            NearToken::from_near(0),
            signed_transaction,
            near_primitives::views::TxExecutionStatus::Included,
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to send account contract deployment transaction: {e}");
            return SimpleResponse::Failure {
                error: "Failed to send account contract deployment transaction".to_string(),
            };
        }
    };

    // Resolve the transaction in our DB asynchronously.
    tokio::spawn(async move {
        if let Err(e) = resolve.await {
            tracing::error!("Failed to resolve transaction: {e}");
        }
    });

    SimpleResponse::success(CreateResponse {
        account_id,
        transaction_hash,
    })
}
