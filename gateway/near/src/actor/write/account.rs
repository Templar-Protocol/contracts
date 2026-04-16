use blockchain_gateway_core::account;
use futures::future::BoxFuture;
use near_api::Account;

use crate::{GatewayError, GatewayResult, NearClient};

use super::{operation_outcome_from_transaction_result, WriteRpcRequest};

impl WriteRpcRequest for account::Delete {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: std::sync::Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = Account(request.signer_account_id.0.clone())
                .delete_account_with_beneficiary(request.body.beneficiary_id)
                .with_signer(signer)
                .wait_until(request.wait_until.into())
                .send_to(client.network())
                .await
                .map_err(|error| GatewayError::NearTransaction(error.to_string()))?;

            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}
