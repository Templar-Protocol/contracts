use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use near_primitives::hash::CryptoHash;
use near_sdk::{
    serde::{Deserialize, Serialize},
    AccountId,
};

use crate::app::App;

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
            StorageDepositResponse::Success { .. } => StatusCode::OK,
            StorageDepositResponse::Failure { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            StorageDepositResponse::UnknownContractId
            | StorageDepositResponse::StorageBalanceAlreadyExists
            | StorageDepositResponse::ContractHasNoStorageRequirements => StatusCode::BAD_REQUEST,
        };
        (status_code, Json(self)).into_response()
    }
}

pub async fn storage_deposit(
    State(app): State<App>,
    Query(StorageDepositRequest {
        account_id,
        contract_id,
    }): Query<StorageDepositRequest>,
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

    let signed_transaction = app
        .near
        .construct_storage_deposit_transaction(
            &app.cache,
            account_id,
            contract_id,
            storage_balance_bounds.min,
        )
        .await;

    let execution = match app.near.send_transaction(signed_transaction).await {
        Ok(result) => result,
        Err(e) => {
            return StorageDepositResponse::Failure {
                error: e.to_string(),
            };
        }
    };

    StorageDepositResponse::Success {
        transaction_hash: execution.transaction.hash,
    }
}
