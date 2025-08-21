use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use near_primitives::{hash::CryptoHash, views::FinalExecutionStatus};
use near_sdk::{
    serde::{Deserialize, Serialize},
    AccountId, NearToken,
};
use tracing::warn;

use crate::{app::App, client::near::STORAGE_DEPOSIT_GAS};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct StorageDepositRequest {
    pub account_id: AccountId,
    pub contract_id: AccountId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub enum StorageDepositResponse {
    Success { transaction_hash: CryptoHash },
    Failure { error: String },
    UnknownContractId,
    ContractHasNoStorageRequirements,
    StorageBalanceAlreadyExists,
}

impl IntoResponse for StorageDepositResponse {
    fn into_response(self) -> Response {
        let status_code = match self {
            Self::Success { .. } => StatusCode::OK,
            Self::Failure { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            Self::UnknownContractId
            | Self::StorageBalanceAlreadyExists
            | Self::ContractHasNoStorageRequirements => StatusCode::BAD_REQUEST,
        };
        (status_code, Json(self)).into_response()
    }
}

pub async fn storage_deposit(
    State(app): State<App>,
    Json(StorageDepositRequest {
        account_id,
        contract_id,
    }): Json<StorageDepositRequest>,
) -> StorageDepositResponse {
    let accounts = app.accounts.read().await;

    let Some(contract_data) = accounts.allowed_contract_data.get(&contract_id) else {
        return StorageDepositResponse::UnknownContractId;
    };

    let Some(storage_balance_bounds) = contract_data
        .storage_balance_bounds
        .as_ref()
        .filter(|b| !b.min.is_zero())
    else {
        return StorageDepositResponse::ContractHasNoStorageRequirements;
    };

    let storage_balance = match app
        .near
        .load_storage_balance_of(contract_id.clone(), &account_id)
        .await
    {
        Ok(storage_balance) => storage_balance,
        Err(e) => {
            return StorageDepositResponse::Failure {
                error: e.to_string(),
            };
        }
    };

    if storage_balance.is_some() {
        return StorageDepositResponse::StorageBalanceAlreadyExists;
    }

    let Some(mut allowance_lock_amount) = app.estimate_cost_of_gas(STORAGE_DEPOSIT_GAS).await
    else {
        return StorageDepositResponse::Failure {
            error: "Failed to estimate gas cost".to_string(),
        };
    };
    allowance_lock_amount = allowance_lock_amount.saturating_add(storage_balance_bounds.min);

    let signed_transaction = app
        .near
        .construct_storage_deposit_transaction(
            &app.cache,
            account_id.clone(),
            contract_id,
            storage_balance_bounds.min,
        )
        .await;

    if let Err(e) = app
        .database
        .set_pending_transaction(
            &account_id,
            allowance_lock_amount,
            signed_transaction.get_hash(),
        )
        .await
    {
        return StorageDepositResponse::Failure {
            error: e.to_string(),
        };
    };

    let execution = match app.near.send_transaction(signed_transaction).await {
        Ok(result) => result,
        Err(e) => {
            return StorageDepositResponse::Failure {
                error: e.to_string(),
            };
        }
    };

    let mut allowance_spent = NearToken::from_yoctonear(execution.tokens_burnt());

    let succeeded = matches!(execution.status, FinalExecutionStatus::SuccessValue(_));

    if succeeded {
        allowance_spent = allowance_spent.saturating_add(storage_balance_bounds.min);
    }

    if let Err(e) = app
        .database
        .record_transaction(
            &account_id,
            execution.transaction.hash,
            allowance_spent,
            succeeded,
        )
        .await
    {
        warn!("Failed to record transaction: {e}");
    }

    StorageDepositResponse::Success {
        transaction_hash: execution.transaction.hash,
    }
}
