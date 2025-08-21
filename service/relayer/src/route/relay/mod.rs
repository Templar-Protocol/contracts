use axum::{extract::State, Json};
use near_primitives::views::FinalExecutionStatus;
use near_sdk::NearToken;
use tracing::error;

use crate::app::App;

mod message;
use message::{RelayRequest, RelayResponse};

pub async fn relay(
    State(app): State<App>,
    Json(relay_request): Json<RelayRequest>,
) -> RelayResponse {
    match app
        .check_and_calculate_gas(&relay_request.signed_delegate_action)
        .await
    {
        Ok(gas) => {
            let account_id = relay_request
                .signed_delegate_action
                .delegate_action
                .sender_id
                .clone();

            let Some(cost_of_gas) = app.estimate_cost_of_gas(gas).await else {
                error!("Failed to estimate cost of gas: {gas}");
                return RelayResponse::Failure {
                    error: "Failed to estimate cost of gas".to_string(),
                };
            };

            let available_allowance = match app
                .database
                .get_available_allowance_or_create(
                    &account_id,
                    app.configuration.starting_allowance_yocto,
                )
                .await
            {
                Ok(available) => available,
                Err(e) => {
                    error!("Database error trying to obtain available balance: {e}");
                    return RelayResponse::Failure {
                        error: "Database Error".to_string(),
                    };
                }
            };

            if available_allowance < cost_of_gas {
                return RelayResponse::Rejected {
                    reason: "Insufficient allowance".to_string(),
                };
            }

            let signed_transaction = app
                .near
                .construct_delegate_transaction(&app.cache, relay_request.signed_delegate_action)
                .await;

            let transaction_hash = signed_transaction.get_hash();

            if let Err(e) = app
                .database
                .set_pending_transaction(&account_id, cost_of_gas, transaction_hash)
                .await
            {
                return RelayResponse::Rejected {
                    reason: e.to_string(),
                };
            }

            let tx_result = match app.near.send_transaction(signed_transaction).await {
                Ok(r) => r,
                Err(e) => {
                    error!("Send transaction failure: {e}");
                    return RelayResponse::Failure {
                        error: e.to_string(),
                    };
                }
            };

            let succeeded = matches!(tx_result.status, FinalExecutionStatus::SuccessValue(_));

            if let Err(e) = app
                .database
                .record_transaction(
                    &account_id,
                    transaction_hash,
                    NearToken::from_yoctonear(tx_result.tokens_burnt()),
                    succeeded,
                )
                .await
            {
                error!("Error recording transaction after submitting to blockchain: {e}");
            }

            RelayResponse::Success {
                transaction_hash: tx_result.transaction.hash,
            }
        }
        Err(e) => RelayResponse::Rejected {
            reason: e.to_string(),
        },
    }
}
