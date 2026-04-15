use blockchain_gateway_core::chain;
use near_api::{
    types::{account::ContractState, errors::ExecutionError, TxExecutionStatus},
    Account, Transaction,
};

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
        let (code_hash, global_contract_hash, global_contract_account_id) =
            match account.data.contract_state {
                ContractState::LocalHash(hash) => (hash.to_string(), None, None),
                ContractState::GlobalHash(hash) => (
                    near_api::types::CryptoHash::default().to_string(),
                    Some(hash.to_string()),
                    None,
                ),
                ContractState::GlobalAccountId(account_id) => (
                    near_api::types::CryptoHash::default().to_string(),
                    None,
                    Some(account_id),
                ),
                ContractState::None => (
                    near_api::types::CryptoHash::default().to_string(),
                    None,
                    None,
                ),
            };

        Ok(chain::ViewAccountResult {
            amount: account.data.amount,
            locked: account.data.locked,
            code_hash,
            storage_usage: account.data.storage_usage,
            global_contract_hash,
            global_contract_account_id,
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
        let result = Transaction::status_with_options(
            params.sender_account_id,
            params.tx_hash.into(),
            TxExecutionStatus::Final,
        )
        .fetch_from(self.inner.network())
        .await
        .map_err(|error| GatewayError::NearQuery(error.to_string()))?;

        let logs = result.logs().into_iter().map(str::to_owned).collect();
        let return_value = decode_return_value(result.clone(), params.encoding)
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?;
        let status = if result.is_pending() {
            chain::TransactionStatus::Pending
        } else if result.is_success() {
            chain::TransactionStatus::Succeeded
        } else {
            chain::TransactionStatus::Failed
        };

        Ok(chain::GetTransactionResult {
            status,
            total_gas_burnt: result.total_gas_burnt,
            logs,
            return_value,
        })
    }
}

fn decode_return_value(
    result: near_api::types::transaction::result::ExecutionFinalResult,
    encoding: chain::ValueEncoding,
) -> Result<Option<chain::TransactionReturnValue>, ExecutionError> {
    match encoding {
        chain::ValueEncoding::Json => match result.json::<serde_json::Value>() {
            Ok(value) => Ok(Some(chain::TransactionReturnValue::Json(value))),
            Err(ExecutionError::EofWhileParsingValue) => Ok(None),
            Err(error) => Err(error),
        },
        chain::ValueEncoding::Base64 => {
            let bytes = result.raw_bytes()?;
            if bytes.is_empty() {
                Ok(None)
            } else {
                Ok(Some(chain::TransactionReturnValue::Base64(
                    blockchain_gateway_core::Base64Bytes(bytes),
                )))
            }
        }
    }
}
