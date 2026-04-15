use std::str::FromStr;

use blockchain_gateway_core::chain;
use near_api::{
    types::{CryptoHash, TxExecutionStatus},
    Account, Transaction,
};
use serde_json::json;

use crate::{
    client::NearClient,
    error::{GatewayError, GatewayResult},
};

#[derive(Clone, Copy)]
pub struct ChainClient<'a> {
    pub(crate) inner: &'a NearClient,
}

impl ChainClient<'_> {
    pub async fn view_account(
        &self,
        params: chain::ViewAccountParams,
    ) -> GatewayResult<chain::ViewAccountResult> {
        let account = Account(params.account_id)
            .view()
            .fetch_from(self.inner.network())
            .await
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?;

        Ok(chain::ViewAccountResult {
            value: serde_json::to_value(account)?,
        })
    }

    pub async fn view_function(
        &self,
        params: chain::ViewFunctionParams,
    ) -> GatewayResult<chain::ViewFunctionResult> {
        let result = self
            .inner
            .view_value(params.contract_id, &params.method_name.0, &params.args)
            .await?;

        Ok(chain::ViewFunctionResult { value: result.data })
    }

    pub async fn get_transaction(
        &self,
        params: chain::GetTransactionParams,
    ) -> GatewayResult<chain::GetTransactionResult> {
        let tx_hash = CryptoHash::from_str(&params.tx_hash)
            .map_err(|error| GatewayError::InvalidTransactionHash(error.to_string()))?;

        let result = Transaction::status_with_options(
            params.sender_account_id,
            tx_hash,
            TxExecutionStatus::Final,
        )
        .fetch_from(self.inner.network())
        .await
        .map_err(|error| GatewayError::NearQuery(error.to_string()))?;

        Ok(chain::GetTransactionResult {
            value: json!({
                "is_success": result.is_success(),
                "is_failure": result.is_failure(),
                "is_pending": result.is_pending(),
                "total_gas_burnt": result.total_gas_burnt,
                "debug": format!("{result:?}"),
            }),
        })
    }
}
