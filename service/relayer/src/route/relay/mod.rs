use axum::{extract::State, Json};
use near_sdk::{borsh, NearToken};
use templar_gateway_methods_spec::tx;
use templar_gateway_types::Base64Bytes;

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
        update_prices,
    }): Json<RelayRequest>,
) -> SimpleResponse<RelayResponse> {
    tracing::info!("Processing relay request");
    let relay_check = match app
        .sda_check_and_calculate_gas(&signed_delegate_action)
        .await
    {
        Ok(x) => {
            tracing::info!(gas = %x.gas, "Gas check passed");
            x
        }
        Err(e) => {
            tracing::info!(error = %e, "Gas check failed");
            return SimpleResponse::Rejected {
                reason: e.to_string(),
            };
        }
    };

    let gas = relay_check.gas;
    let contract_data = relay_check.contract_data;
    let market_ids = relay_check.market_ids;

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

    if update_prices {
        if let Err(error) = app.update_market_prices(&market_ids).await {
            return SimpleResponse::Failure {
                error: error.to_string(),
            };
        }
    }

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

    // NEP-366: the gateway wraps the user's signed delegate action in a
    // transaction the relay account signs and pays for. The gateway decodes the
    // borsh-encoded delegate action (the NEP-366 layout is signer-agnostic).
    let signed_delegate_action = match borsh::to_vec(&signed_delegate_action) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("Failed to encode signed delegate action: {e}");
            return SimpleResponse::Failure {
                error: "Failed to encode signed delegate action".to_string(),
            };
        }
    };

    let transaction_hash = match app
        .execute_and_account(
            account_id,
            app.args.relay.account_id.clone(),
            cost_of_gas,
            NearToken::from_near(0),
            tx::RelaySignedDelegateAction {
                signed_delegate_action: Base64Bytes(signed_delegate_action),
            },
        )
        .await
    {
        Ok(transaction_hash) => transaction_hash,
        Err(e) => {
            tracing::error!("Relay submission failure: {e}");
            return SimpleResponse::Failure {
                error: e.to_string(),
            };
        }
    };

    RelayResponse { transaction_hash }.into()
}
