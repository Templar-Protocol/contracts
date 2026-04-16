use blockchain_gateway_core::ft;
use futures::future::BoxFuture;

use crate::{client::ContractWriteOptions, GatewayResult, NearClient};

use crate::client::ft::TransferArgs;

use super::{operation_outcome_from_transaction_result, DispatchWrite};

impl DispatchWrite for ft::Transfer {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: std::sync::Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = client
                .ft(body.token_id)
                .ft_transfer(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .gas(blockchain_gateway_core::NearGas::from_tgas(100))
                        .deposit(blockchain_gateway_core::NearToken::from_yoctonear(1)),
                    TransferArgs {
                        receiver_id: body.receiver_id,
                        amount: body.amount,
                    },
                )
                .await?;

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
