use axum::{extract::State, Json};
use near_primitives::{action::FunctionCallAction, hash::CryptoHash, views::TxExecutionStatus};
use near_sdk::{
    serde::{Deserialize, Serialize},
    AccountId, NearToken,
};
use templar_universal_account::{
    authentication::{ExecutionContextProvider, Key},
    transaction::Action,
    ExecuteArgs, KeyId,
};

use crate::{app::App, route::SimpleResponse};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct RelayRequest {
    pub account_id: AccountId,
    pub args: ExecuteArgs,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct RelayResponse {
    pub transaction_hash: CryptoHash,
}

#[allow(clippy::too_many_lines)]
pub async fn relay(
    State(app): State<App>,
    Json(RelayRequest { account_id, args }): Json<RelayRequest>,
) -> SimpleResponse<RelayResponse> {
    let ExecuteArgs::Passkey {
        ref key,
        ref message,
    } = args;

    let parameters = match app
        .ua_near
        .load_ua_key(account_id.clone(), KeyId::Passkey(key.clone()))
        .await
    {
        Ok(parameters) => parameters,
        Err(e) => {
            // Account might not exist, but we also might have connection issues.
            tracing::warn!("Failed to load execution parameters for key \"{}\" from universal account \"{account_id}\": {e}", &key.0);
            return SimpleResponse::Failure {
                error: "Failed to load execution parameters from universal account".to_string(),
            };
        }
    };

    let Some(parameters) = parameters else {
        tracing::info!(
            "Key \"{}\" does not exist on account \"{account_id}\"",
            key.0
        );
        return SimpleResponse::Rejected {
            reason: "Key does not exist on account".to_string(),
        };
    };

    let valid_signature = match key.verify(message.clone()) {
        Ok(p) => p,
        Err(e) => {
            tracing::info!("Signature verification failed: {e}");
            return SimpleResponse::Rejected {
                reason: "Signature verification failed".to_string(),
            };
        }
    };

    let payload = match valid_signature.verify(&account_id, &parameters.next(), |o| {
        app.args.ua.is_origin_allowed(o)
    }) {
        Ok(p) => p,
        Err(e) => {
            tracing::info!("Execution parameter verification failed: {e}");
            return SimpleResponse::Rejected {
                reason: "Execution parameter verification failed".to_string(),
            };
        }
    };

    let accounts = app.accounts.read().await;

    let mut gas = near_sdk::Gas::from_tgas(app.args.ua.execute_tgas).as_gas();
    for transaction in payload {
        let receiver_id = &transaction.receiver_id;
        if !accounts.allowed_contract_data.contains_key(receiver_id) {
            tracing::info!("Unknown receiver {receiver_id}");
            return SimpleResponse::Rejected {
                reason: "Unknown receiver".to_string(),
            };
        }
        let calls = match transaction
            .actions
            .iter()
            .map(|action| match action {
                Action::FunctionCall {
                    function_name,
                    arguments,
                    amount,
                    gas,
                }
                | Action::FunctionCallWeight {
                    function_name,
                    arguments,
                    amount,
                    gas,
                    ..
                } => Ok(FunctionCallAction {
                    method_name: function_name.clone(),
                    args: arguments.0.clone(),
                    gas: gas.as_gas(),
                    deposit: amount.as_yoctonear(),
                }),
                a => Err(a),
            })
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(calls) => calls,
            Err(e) => {
                tracing::info!("Unsupported action type: {e:?}");
                return SimpleResponse::Rejected {
                    reason: "Unsupported action type".to_string(),
                };
            }
        };
        if let Err(e) = app.actions_are_allowed(receiver_id, &accounts, calls.iter()) {
            tracing::info!("Disallowed action: {e}");
            return SimpleResponse::Rejected {
                reason: "Disallowed action".to_string(),
            };
        }
        gas += calls.iter().map(|f| f.gas).sum::<u64>();
    }

    let signed_transaction = app
        .relay_near
        .construct_ua_execute_transaction(&app.cache, account_id.clone(), args, gas)
        .await;
    let Some(cost_of_gas) = app.estimate_cost_of_gas(gas).await else {
        tracing::error!("Failed to estimate cost of gas");
        return SimpleResponse::Failure {
            error: "Failed to estimate cost of gas".to_string(),
        };
    };

    let transaction_hash = signed_transaction.get_hash();

    let resolve_transaction = match app
        .send_and_resolve_transaction(
            account_id,
            cost_of_gas,
            NearToken::from_near(0),
            signed_transaction,
            TxExecutionStatus::Included,
        )
        .await
    {
        Ok(future) => future,
        Err(e) => {
            tracing::error!("Send transaction failure: {e}");
            return SimpleResponse::Failure {
                error: format!("Send transaction failure: {e}"),
            };
        }
    };

    // Resolve asynchronously.
    tokio::spawn(async move {
        if let Err(e) = resolve_transaction.await {
            tracing::error!("Resolve transaction failure: {e}");
        }
    });

    RelayResponse { transaction_hash }.into()
}
