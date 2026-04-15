use std::sync::Arc;

use actix::{Actor, Addr, ArbiterHandle, Context, Handler, Message, ResponseFuture};
use blockchain_gateway_core::{chain, market, registry, storage, universal_account, MethodSpec};
use futures::future::BoxFuture;
use tokio::sync::Semaphore;

use crate::{GatewayError, GatewayResult, NearReadClient};

use super::rpc::RpcMessage;

const READ_ACTOR_NAME: &str = "read-actor";
const READ_ACTOR_MAX_CONCURRENCY: usize = 64;

fn into_parameters_view(
    parameters: templar_universal_account::PayloadExecutionParameters,
) -> universal_account::PayloadExecutionParametersView {
    universal_account::PayloadExecutionParametersView {
        block_height: parameters.block_height.0,
        index: parameters.index.0,
        nonce: parameters.nonce.0,
        name: parameters.name,
        version: parameters.version,
        chain_id: parameters.chain_id.map(|value| value.0),
        verifying_contract: parameters
            .verifying_contract
            .to_string()
            .parse()
            .expect("templar universal account should emit valid account ids"),
        salt: parameters
            .salt
            .and_then(|value| serde_json::to_value(value).ok())
            .and_then(|value| value.as_str().map(str::to_owned)),
    }
}

pub trait ReadRpcRequest: MethodSpec + Sized + Send + 'static {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>>;
}

#[derive(Clone)]
pub struct ReadActor {
    client: NearReadClient,
    semaphore: Arc<Semaphore>,
}

impl ReadActor {
    fn new(client: NearReadClient) -> Self {
        Self {
            client,
            semaphore: Arc::new(Semaphore::new(READ_ACTOR_MAX_CONCURRENCY)),
        }
    }

    pub(crate) fn spawn(arbiter: &ArbiterHandle, client: NearReadClient) -> Addr<Self> {
        Self::start_in_arbiter(arbiter, move |_ctx| Self::new(client))
    }
}

impl<Spec> Handler<RpcMessage<Spec>> for ReadActor
where
    Spec: ReadRpcRequest,
{
    type Result = ResponseFuture<GatewayResult<Spec::Output>>;

    fn handle(&mut self, message: RpcMessage<Spec>, _ctx: &mut Self::Context) -> Self::Result {
        let client = self.client.clone();
        let semaphore = self.semaphore.clone();

        Box::pin(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_error| GatewayError::ActorUnavailable(READ_ACTOR_NAME))?;
            Spec::dispatch(message, client).await
        })
    }
}

impl ReadRpcRequest for chain::ViewAccount {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move { client.chain().view_account(params.0.body).await })
    }
}

impl ReadRpcRequest for chain::ViewFunction {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move { client.chain().view_function(params.0.body).await })
    }
}

impl ReadRpcRequest for chain::GetTransaction {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move { client.chain().get_transaction(params.0.body).await })
    }
}

impl ReadRpcRequest for registry::ListDeployments {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.body;
            client
                .registry(params.registry_id)
                .list_deployments(params.args)
                .await
                .map(|account_ids| registry::ListDeploymentsResult { account_ids })
        })
    }
}

impl ReadRpcRequest for registry::ListVersions {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.body;
            client
                .registry(params.registry_id)
                .list_versions(params.args)
                .await
                .map(|values| registry::ListVersionsResult { values })
        })
    }
}

impl ReadRpcRequest for market::GetConfiguration {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(params.0.body.market_id)
                .get_configuration(())
                .await
        })
    }
}

impl ReadRpcRequest for market::ListBorrowPositions {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.body;
            client
                .market(params.market_id)
                .list_borrow_positions(params.args)
                .await
                .map(|positions| market::ListBorrowPositionsResult { positions })
        })
    }
}

impl ReadRpcRequest for storage::GetBalanceBounds {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.body;
            client
                .storage(params.contract_id)
                .storage_balance_bounds(params.args)
                .await
                .map(|bounds| storage::GetBalanceBoundsResult {
                    bounds: blockchain_gateway_core::common::StorageBalanceBounds {
                        min: bounds.min,
                        max: bounds.max,
                    },
                })
        })
    }
}

impl ReadRpcRequest for storage::GetBalanceOf {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.body;
            client
                .storage(params.contract_id)
                .storage_balance_of(params.args)
                .await
                .map(|balance| storage::GetBalanceOfResult {
                    balance: balance.map(|balance| {
                        blockchain_gateway_core::common::StorageBalance {
                            total: balance.total,
                            available: balance.available,
                        }
                    }),
                })
        })
    }
}

impl ReadRpcRequest for universal_account::GetKey {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.body;
            client
                .universal_account(params.account_id)
                .get_key(params.args)
                .await
                .map(|parameters| universal_account::GetKeyResult {
                    parameters: parameters.map(into_parameters_view),
                })
        })
    }
}

impl Actor for ReadActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(64);
    }
}
