use axum::{extract::State, Json};
use near_sdk::{borsh, NearToken};
use templar_gateway_methods_spec::tx;
use templar_gateway_types::SignedDelegateActionInput;

use crate::{
    app::{App, SdaCheckError},
    client::database::AccountedStatus,
    route::SimpleResponse,
};

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
        Err(SdaCheckError::Rejected(reason)) => {
            tracing::info!(error = %reason, "Gas check rejected payload");
            return SimpleResponse::Rejected {
                reason: reason.to_string(),
            };
        }
        Err(error @ SdaCheckError::MarketConfigurationRead { .. }) => {
            tracing::warn!(%error, "Gas check failed while reading market configuration");
            return SimpleResponse::Failure {
                error: "Failed to load market configuration".to_string(),
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
    }

    if update_prices {
        // Best-effort: a failed oracle refresh must not block the user's already-paid
        // transaction. Shout loudly, then relay anyway so it lands on-chain and the
        // contract returns a precise result the user can reference, rather than an
        // opaque transient relayer error.
        if let Err(error) = app.update_market_prices(&market_ids).await {
            tracing::error!(
                %error,
                ?market_ids,
                "Failed to refresh oracle prices before relay; relaying anyway",
            );
        }
    }

    let Some(cost_of_gas) = app.estimate_cost_of_gas(gas).await else {
        tracing::error!("Failed to estimate cost of gas: {gas}");
        return SimpleResponse::Failure {
            error: "Failed to estimate cost of gas".to_string(),
        };
    };

    // Read-only pre-check; execute_and_account provisions a first-seen account.
    let available_allowance = match app
        .database
        .get_available_allowance(&account_id)
        .await
        .map(|a| a.unwrap_or(app.args.relay.starting_allowance_yocto))
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
    // transaction the relay account signs and pays for. Re-encode to borsh and
    // hand the gateway its validated `SignedDelegateActionInput` (the NEP-366
    // layout is signer-agnostic, bridging near_primitives -> near_api_types).
    let signed_delegate_action = match borsh::to_vec(&signed_delegate_action)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            SignedDelegateActionInput::from_borsh_bytes(&bytes).map_err(|error| error.to_string())
        }) {
        Ok(payload) => payload,
        Err(e) => {
            tracing::error!("Failed to encode signed delegate action: {e}");
            return SimpleResponse::Failure {
                error: "Failed to encode signed delegate action".to_string(),
            };
        }
    };

    let execution = match app
        .execute_and_account(
            account_id,
            app.args.relay.account_id.clone(),
            cost_of_gas,
            NearToken::from_near(0),
            tx::RelaySignedDelegateAction {
                signed_delegate_action,
            },
        )
        .await
    {
        Ok(execution) => execution,
        Err(e) => {
            tracing::error!("Relay submission failure: {e}");
            return SimpleResponse::Failure {
                error: e.to_string(),
            };
        }
    };

    // The relayer's job is to land the caller's signed transaction on chain; if
    // it then reverts, that is the caller's transaction to fix, not a relay
    // failure. Report success with the hash so the caller can inspect the outcome.
    if execution.status == AccountedStatus::Failed {
        tracing::warn!(transaction_hash = %execution.transaction_hash, "Relayed transaction reverted on chain");
    }

    RelayResponse {
        transaction_hash: execution.transaction_hash,
    }
    .into()
}
