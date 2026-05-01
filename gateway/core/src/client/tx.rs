use near_api::{types::transaction::result::TransactionResult, Contract};
use templar_gateway_types::{common::ContractArgs, ContractMethodName, NearGas, NearToken};

use crate::{client::NearClient, GatewayError, GatewayResult};

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionCallRequest {
    pub receiver_id: near_account_id::AccountId,
    pub method_name: ContractMethodName,
    pub args: ContractArgs,
    pub gas: NearGas,
    pub deposit: NearToken,
}

#[derive(Clone)]
pub struct TxClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) signer_account_id: templar_gateway_types::ManagedAccountId,
    pub(crate) signer: std::sync::Arc<near_api::Signer>,
}

impl TxClient<'_> {
    pub async fn function_call(
        &self,
        body: FunctionCallRequest,
        wait_until: templar_gateway_types::common::TxExecutionStatus,
    ) -> GatewayResult<TransactionResult> {
        Contract(body.receiver_id)
            .call_function_raw(&body.method_name.0, body.args.try_into_bytes()?)
            .transaction()
            .gas(body.gas)
            .deposit(body.deposit)
            .with_signer(self.signer_account_id.0.clone(), self.signer.clone())
            .wait_until(wait_until.into())
            .send_to(self.inner.network())
            .await
            .map_err(|error| GatewayError::NearTransaction(error.to_string()))
    }
}
