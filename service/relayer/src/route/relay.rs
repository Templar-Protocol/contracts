use axum::{extract::State, http::StatusCode, Json};
use near_primitives::views::FinalExecutionStatus;
use near_sdk::NearToken;
use tracing::error;

use crate::{
    app::App,
    message::{RelayRequest, RelayResponse},
};

pub async fn relay(
    State(app): State<App>,
    Json(relay_request): Json<RelayRequest>,
) -> (StatusCode, Json<RelayResponse>) {
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
                return RelayResponse::failure("Failed to estimate cost of gas");
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
                    return RelayResponse::failure("Database Error");
                }
            };

            if available_allowance < cost_of_gas {
                return RelayResponse::rejected("Insufficient allowance");
            }

            let signed_transaction = match app
                .near
                .construct_delegate_transaction(relay_request.signed_delegate_action)
                .await
            {
                Ok(tx) => tx,
                Err(e) => {
                    error!("Error constructing delegate transaction: {e}");
                    return RelayResponse::failure(e);
                }
            };

            let transaction_hash = signed_transaction.get_hash();

            if let Err(e) = app
                .database
                .set_pending_transaction(&account_id, cost_of_gas, transaction_hash)
                .await
            {
                return RelayResponse::rejected(e);
            }

            let tx_result = match app.near.send_transaction(signed_transaction).await {
                Ok(r) => r,
                Err(e) => {
                    error!("Send transaction failure: {e}");
                    return RelayResponse::failure(e);
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

            RelayResponse::success(tx_result)
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(RelayResponse::Rejected {
                reason: e.to_string(),
            }),
        ),
    }
}
