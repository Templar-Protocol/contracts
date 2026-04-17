use std::sync::Arc;

use blockchain_gateway_core::{proxy_oracle_governance, ManagedAccountId, NearGas};
use futures::future::BoxFuture;

use crate::{
    actor::{operation_outcome_from_transaction_result, DispatchRead, DispatchWrite},
    client::{
        proxy_oracle::{GovActionArgs, GovCreateArgs, GovGetArgs, GovListArgs},
        ContractWriteOptions,
    },
    GatewayResult, NearClient,
};

impl DispatchRead for proxy_oracle_governance::GetNextId {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .proxy_oracle(request.params.oracle_id)
                .gov_next_id(())
                .await
        })
    }
}

impl DispatchRead for proxy_oracle_governance::GetTtl {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let ttl_ns = client
                .proxy_oracle(request.params.oracle_id)
                .gov_ttl_ns(())
                .await?;
            Ok(Self::Output { ttl_ns })
        })
    }
}

impl DispatchRead for proxy_oracle_governance::GetCount {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .proxy_oracle(request.params.oracle_id)
                .gov_count(())
                .await
        })
    }
}

impl DispatchRead for proxy_oracle_governance::List {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .proxy_oracle(request.params.oracle_id)
                .gov_list(GovListArgs {
                    offset: request.params.offset,
                    count: request.params.count,
                })
                .await
                .map(|ids| proxy_oracle_governance::ListResult { ids })
        })
    }
}

impl DispatchRead for proxy_oracle_governance::Get {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            client
                .proxy_oracle(params.oracle_id)
                .gov_get(GovGetArgs { id: params.id })
                .await
                .map(|proposal| proxy_oracle_governance::GetResult { proposal })
        })
    }
}

impl DispatchWrite for proxy_oracle_governance::Create {
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
                .gov_create(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .one_yocto()
                        .gas(NearGas::from_tgas(300)),
                    GovCreateArgs {
                        id: body.id,
                        operation: body.operation,
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

impl DispatchWrite for proxy_oracle_governance::Cancel {
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
                .gov_cancel(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .one_yocto()
                        .gas(NearGas::from_tgas(300)),
                    GovActionArgs { id: body.id },
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

impl DispatchWrite for proxy_oracle_governance::Execute {
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
                .gov_execute(
                    ContractWriteOptions::new(request.signer_account_id, signer)
                        .wait_until(request.wait_until)
                        .one_yocto()
                        .gas(NearGas::from_tgas(300)),
                    GovActionArgs { id: body.id },
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
