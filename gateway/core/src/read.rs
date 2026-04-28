use async_trait::async_trait;
use near_api::types::{
    account::Account, transaction::result::ExecutionFinalResult, CryptoHash, TxExecutionStatus,
};
use near_api::{Account as NearAccountView, Contract, Transaction};
use serde::de::DeserializeOwned;

use crate::{GatewayError, GatewayResult, NearClient};

#[async_trait]
pub trait ReadNear: Send + Sync {
    async fn view_function<T>(
        &self,
        contract_id: near_account_id::AccountId,
        method_name: &str,
        args: Vec<u8>,
    ) -> GatewayResult<T>
    where
        T: DeserializeOwned + Send + Sync + 'static;

    async fn view_account(&self, account_id: near_account_id::AccountId) -> GatewayResult<Account>;

    async fn view_transaction_status(
        &self,
        sender_account_id: near_account_id::AccountId,
        tx_hash: CryptoHash,
        wait_until: TxExecutionStatus,
    ) -> GatewayResult<ExecutionFinalResult>;
}

#[async_trait]
impl ReadNear for NearClient {
    async fn view_function<T>(
        &self,
        contract_id: near_account_id::AccountId,
        method_name: &str,
        args: Vec<u8>,
    ) -> GatewayResult<T>
    where
        T: DeserializeOwned + Send + Sync + 'static,
    {
        Ok(Contract(contract_id)
            .call_function_raw(method_name, args)
            .read_only()
            .fetch_from(self.network())
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?
            .data)
    }

    async fn view_account(&self, account_id: near_account_id::AccountId) -> GatewayResult<Account> {
        let account = NearAccountView(account_id)
            .view()
            .fetch_from(self.network())
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?;
        Ok(account.data)
    }

    async fn view_transaction_status(
        &self,
        sender_account_id: near_account_id::AccountId,
        tx_hash: CryptoHash,
        wait_until: TxExecutionStatus,
    ) -> GatewayResult<ExecutionFinalResult> {
        Transaction::status_with_options(sender_account_id, tx_hash, wait_until)
            .fetch_from(self.network())
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))
    }
}
