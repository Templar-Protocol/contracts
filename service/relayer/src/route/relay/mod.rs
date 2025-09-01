use axum::{extract::State, Json};
use near_primitives::views::TxExecutionStatus;
use tracing::error;

use crate::{app::App, client::near::STORAGE_DEPOSIT_GAS};

mod message;
use message::{RelayRequest, RelayResponse};

#[allow(clippy::too_many_lines)]
pub async fn relay(
    State(app): State<App>,
    Json(RelayRequest {
        signed_delegate_action,
        storage_deposit,
        wait_until,
    }): Json<RelayRequest>,
) -> RelayResponse {
    let (gas, contract_data) = match app.check_and_calculate_gas(&signed_delegate_action).await {
        Ok(x) => x,
        Err(e) => {
            return RelayResponse::Rejected {
                reason: e.to_string(),
            }
        }
    };

    let account_id = signed_delegate_action.delegate_action.sender_id.clone();

    // Deposit for storage before sending the meta transaction.
    if storage_deposit {
        let contract_id = signed_delegate_action.delegate_action.receiver_id.clone();

        let Some(storage_balance_bounds) = contract_data
            .storage_balance_bounds
            .as_ref()
            .filter(|b| !b.min.is_zero())
        else {
            return RelayResponse::Rejected {
                reason: "Contract has no storage requirements".to_string(),
            };
        };

        let storage_balance = match app
            .near
            .load_storage_balance_of(contract_id.clone(), &account_id)
            .await
        {
            Ok(storage_balance) => storage_balance,
            Err(e) => {
                return RelayResponse::Failure {
                    error: e.to_string(),
                };
            }
        };

        if storage_balance.is_some() {
            return RelayResponse::Rejected {
                reason: "Storage balance already exists".to_string(),
            };
        }

        let Some(cost_of_gas) = app
            .estimate_cost_of_gas(STORAGE_DEPOSIT_GAS)
            .await
            .map(|amount| amount.saturating_add(storage_balance_bounds.min))
        else {
            return RelayResponse::Failure {
                error: "Failed to estimate gas cost".to_string(),
            };
        };

        let signed_transaction = app
            .near
            .construct_storage_deposit_transaction(
                &app.cache,
                account_id.clone(),
                contract_id,
                storage_balance_bounds.min,
            )
            .await;

        let resolve_transaction = match app
            .send_and_resolve_transaction(
                account_id.clone(),
                cost_of_gas,
                signed_transaction,
                TxExecutionStatus::Final,
            )
            .await
        {
            Ok(future) => future,
            Err(e) => {
                error!("Send transaction failure: {e}");
                return RelayResponse::Failure {
                    error: e.to_string(),
                };
            }
        };

        // Resolve synchronously.
        if let Err(e) = resolve_transaction.await {
            error!("Resolve transaction failure: {e}");
        }
    } // end storage deposit

    let Some(cost_of_gas) = app.estimate_cost_of_gas(gas).await else {
        error!("Failed to estimate cost of gas: {gas}");
        return RelayResponse::Failure {
            error: "Failed to estimate cost of gas".to_string(),
        };
    };

    let available_allowance = match app
        .database
        .get_available_allowance_or_create(&account_id, app.args.starting_allowance_yocto)
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
        .construct_delegate_transaction(&app.cache, signed_delegate_action)
        .await;

    let transaction_hash = signed_transaction.get_hash();

    let resolve_transaction = match app
        .send_and_resolve_transaction(account_id, cost_of_gas, signed_transaction, wait_until)
        .await
    {
        Ok(future) => future,
        Err(e) => {
            error!("Send transaction failure: {e}");
            return RelayResponse::Failure {
                error: e.to_string(),
            };
        }
    };

    // Resolve asynchronously.
    tokio::spawn(async move {
        if let Err(e) = resolve_transaction.await {
            error!("Resolve transaction failure: {e}");
        }
    });

    RelayResponse::Success { transaction_hash }
}
