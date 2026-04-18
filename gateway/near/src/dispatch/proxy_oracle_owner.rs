use std::sync::Arc;

use blockchain_gateway_core::{proxy_oracle_owner, ManagedAccountId};
use futures::future::BoxFuture;

use crate::{
    actor::{operation_outcome_from_transaction_result, DispatchRead, DispatchWrite},
    client::{proxy_oracle::OwnerProposeArgs, ContractWriteOptions},
    GatewayResult, NearClient,
};

impl DispatchRead for proxy_oracle_owner::GetOwner {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .proxy_oracle(request.params.oracle_id)
                .own_get_owner(())
                .await
                .map(|owner| proxy_oracle_owner::GetOwnerResult { owner })
        })
    }
}

impl DispatchRead for proxy_oracle_owner::GetProposedOwner {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .proxy_oracle(request.params.oracle_id)
                .own_get_proposed_owner(())
                .await
                .map(|proposed_owner| proxy_oracle_owner::GetProposedOwnerResult { proposed_owner })
        })
    }
}

impl DispatchWrite for proxy_oracle_owner::ProposeOwner {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx = client
                .proxy_oracle(body.oracle_id)
                .own_propose_owner(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .one_yocto()
                        .tgas(300),
                    OwnerProposeArgs {
                        account_id: body.account_id,
                    },
                )
                .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for proxy_oracle_owner::AcceptOwner {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx = client
                .proxy_oracle(request.body.oracle_id)
                .own_accept_owner(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .one_yocto()
                        .tgas(300),
                    (),
                )
                .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for proxy_oracle_owner::RenounceOwner {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx = client
                .proxy_oracle(request.body.oracle_id)
                .own_renounce_owner(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .one_yocto()
                        .tgas(300),
                    (),
                )
                .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &ManagedAccountId {
        &request.signer_account_id
    }
}
