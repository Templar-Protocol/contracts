use blockchain_gateway_core::tx;
use near_api::{types::errors::ExecutionError, Transaction};

use crate::{
    client::NearClient,
    error::{GatewayError, GatewayResult},
};

#[derive(Clone, Copy)]
pub struct ChainClient<'a> {
    pub(crate) inner: &'a NearClient,
}

impl ChainClient<'_> {
    pub async fn get_transaction(&self, params: tx::GetParams) -> GatewayResult<tx::GetResult> {
        let wait_until = params.wait_until.unwrap_or_default();
        let result = Transaction::status_with_options(
            params.sender_account_id,
            params.tx_hash.into(),
            wait_until.into(),
        )
        .fetch_from(self.inner.network())
        .await
        .map_err(|error| GatewayError::NearQuery(error.to_string()))?;

        let logs = result.logs().into_iter().map(str::to_owned).collect();
        let return_value = decode_return_value(result.clone(), params.encoding)
            .map_err(|error| GatewayError::NearQuery(error.to_string()))?;
        let status = if result.is_pending() {
            tx::Status::Pending
        } else if result.is_success() {
            tx::Status::Succeeded
        } else {
            tx::Status::Failed
        };

        Ok(tx::GetResult {
            status,
            total_gas_burnt: result.total_gas_burnt,
            logs,
            return_value,
        })
    }
}

fn decode_return_value(
    result: near_api::types::transaction::result::ExecutionFinalResult,
    encoding: tx::ValueEncoding,
) -> Result<Option<tx::ReturnValue>, ExecutionError> {
    match encoding {
        tx::ValueEncoding::Json => match result.json::<serde_json::Value>() {
            Ok(value) => Ok(Some(tx::ReturnValue::Json(value))),
            Err(ExecutionError::EofWhileParsingValue) => Ok(None),
            Err(error) => Err(error),
        },
        tx::ValueEncoding::Base64 => {
            let bytes = result.raw_bytes()?;
            if bytes.is_empty() {
                Ok(None)
            } else {
                Ok(Some(tx::ReturnValue::Base64(
                    blockchain_gateway_core::Base64Bytes(bytes),
                )))
            }
        }
    }
}
