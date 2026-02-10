use axum::{extract::State, Json};
use near_sdk::NearToken;

use crate::{app::App, route::SimpleResponse};

mod message;
pub use message::{RelayRequest, RelayResponse};

#[allow(clippy::too_many_lines)]
#[tracing::instrument(
    name = "relay_transaction",
    skip(app, signed_delegate_action),
    fields(
        sender_id = %signed_delegate_action.delegate_action.sender_id,
        receiver_id = %signed_delegate_action.delegate_action.receiver_id,
        storage_deposit = %storage_deposit,
    )
)]
pub async fn relay(
    State(app): State<App>,
    Json(RelayRequest {
        signed_delegate_action,
        storage_deposit,
        wait_until,
    }): Json<RelayRequest>,
) -> SimpleResponse<RelayResponse> {
    tracing::info!("Processing relay request");
    let (gas, contract_data) = match app
        .sda_check_and_calculate_gas(&signed_delegate_action)
        .await
    {
        Ok(x) => {
            tracing::info!(gas = %x.0, "Gas check passed");
            x
        }
        Err(e) => {
            tracing::info!(error = %e, "Gas check failed");
            return SimpleResponse::Rejected {
                reason: e.to_string(),
            };
        }
    };

    let account_id = signed_delegate_action.delegate_action.sender_id.clone();

    // Deposit for storage before sending the meta transaction.
    if storage_deposit {
        tracing::info!("Processing storage deposit request");
        let contract_id = signed_delegate_action.delegate_action.receiver_id.clone();

        if let Err(e) = app
            .storage_deposit_top_up(&contract_data, contract_id, account_id.clone())
            .await
        {
            tracing::warn!(error = %e, "Storage deposit error");
            return SimpleResponse::Failure {
                error: format!("Storage deposit error: {e}"),
            };
        }
    } // end storage deposit

    let Some(cost_of_gas) = app.estimate_cost_of_gas(gas).await else {
        tracing::error!("Failed to estimate cost of gas: {gas}");
        return SimpleResponse::Failure {
            error: "Failed to estimate cost of gas".to_string(),
        };
    };

    let available_allowance = match app
        .database
        .get_available_allowance_or_create(&account_id, app.args.relay.starting_allowance_yocto)
        .await
    {
        Ok(available) => available,
        Err(e) => {
            tracing::error!("Database error trying to obtain available balance: {e}");
            return SimpleResponse::Failure {
                error: "Database Error".to_string(),
            };
        }
    };

    if available_allowance < cost_of_gas {
        return SimpleResponse::Rejected {
            reason: "Insufficient allowance".to_string(),
        };
    }

    let signed_transaction = app
        .relay_near
        .construct_delegate_transaction(&app.cache, signed_delegate_action)
        .await;

    let transaction_hash = signed_transaction.get_hash();

    let resolve_transaction = match app
        .send_and_resolve_transaction(
            account_id,
            cost_of_gas,
            NearToken::ZERO,
            signed_transaction,
            wait_until,
        )
        .await
    {
        Ok(future) => future,
        Err(e) => {
            tracing::error!("Send transaction failure: {e}");
            return SimpleResponse::Failure {
                error: e.to_string(),
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
