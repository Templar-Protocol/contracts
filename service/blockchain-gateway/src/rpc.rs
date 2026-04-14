use blockchain_gateway_core::{
    chain,
    common::ReadRequest,
    market, registry,
    rpc::common::{WriteOperationResult, WriteRequest},
    storage, tx, universal_account, MethodSpec,
};
use blockchain_gateway_near::{
    actor::{read::ReadMessage, write::WriteMessage, ActorRequest, MessageEnvelope},
    GatewayError, GatewayResult, GatewayService, NearReadClient, NearWriteClient,
};
use futures::future::BoxFuture;
use jsonrpsee::{
    core::{RegisterMethodError, RpcResult},
    types::ErrorObjectOwned,
    RpcModule,
};

const GATEWAY_SERVER_ERROR_CODE: i32 = -32000;

#[allow(clippy::needless_pass_by_value)]
fn map_gateway_error(error: GatewayError) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(GATEWAY_SERVER_ERROR_CODE, error.to_string(), None::<()>)
}

fn register<Spec>(module: &mut RpcModule<GatewayService>) -> Result<(), RegisterMethodError>
where
    Spec: MethodSpec,
    Spec::Input: DirectRequest + Send + 'static,
    Spec::Output: Clone + Send + Sync + 'static,
{
    module.register_async_method(Spec::RPC_METHOD, move |params, service, _| async move {
        let params: Spec::Input = params.parse()?;
        let result = params
            .request(service.as_ref())
            .await
            .map_err(map_gateway_error)?;
        RpcResult::Ok(result)
    })?;

    Ok(())
}

trait DirectRequest {
    type Output: Clone + serde::Serialize;
    fn request(self, service: &GatewayService) -> BoxFuture<'_, GatewayResult<Self::Output>>;
}

impl<I, O> DirectRequest for ReadRequest<I>
where
    I: Send + 'static,
    O: Send + 'static + serde::Serialize + Clone,
    I: ActorRequest<Actor = NearReadClient, Response = O>,
    ReadMessage: From<MessageEnvelope<I>>,
{
    type Output = O;

    fn request(self, service: &GatewayService) -> BoxFuture<'_, GatewayResult<Self::Output>> {
        Box::pin(async move { service.read().request(self).await })
    }
}

impl<T> DirectRequest for WriteRequest<T>
where
    T: Send + 'static,
    WriteRequest<T>: ActorRequest<Actor = NearWriteClient, Response = WriteOperationResult>,
    WriteMessage: From<MessageEnvelope<WriteRequest<T>>>,
{
    type Output = WriteOperationResult;

    fn request(self, service: &GatewayService) -> BoxFuture<'_, GatewayResult<Self::Output>> {
        Box::pin(async move { service.write().request(self).await })
    }
}

pub fn attach_gateway(
    service: GatewayService,
) -> Result<RpcModule<GatewayService>, RegisterMethodError> {
    let mut m = RpcModule::new(service);

    register::<chain::ViewAccount>(&mut m)?;
    register::<chain::ViewFunction>(&mut m)?;
    register::<chain::GetTransaction>(&mut m)?;
    register::<registry::ListDeployments>(&mut m)?;
    register::<registry::ListVersions>(&mut m)?;
    register::<market::GetConfiguration>(&mut m)?;
    register::<market::ListBorrowPositions>(&mut m)?;
    register::<storage::GetBalanceBounds>(&mut m)?;
    register::<storage::GetBalanceOf>(&mut m)?;
    register::<storage::Deposit>(&mut m)?;
    register::<universal_account::GetKey>(&mut m)?;
    register::<tx::FunctionCall>(&mut m)?;

    Ok(m)
}

#[cfg(test)]
mod tests {
    use blockchain_gateway_core::ManagedAccountId;

    use super::*;
    use std::collections::HashMap;

    fn test_gateway() -> GatewayService {
        let network = near_api::NetworkConfig::from_rpc_url(
            "test",
            "http://127.0.0.1:3030".parse().expect("valid url"),
        );
        let near = blockchain_gateway_near::NearReadClient::new(network.clone());
        let signer = near_api::Signer::from_secret_key(
            "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q"
                .parse()
                .expect("valid test secret key"),
        )
        .expect("signer should initialize");
        let account_id = ManagedAccountId("test.near".parse().expect("valid signer account id"));
        let writer = blockchain_gateway_near::NearWriteClient::new(
            network,
            HashMap::from([(
                account_id,
                blockchain_gateway_near::ManagedSigner {
                    signer,
                    key_count: 1,
                },
            )]),
        );
        GatewayService::spawn(near, writer).0
    }

    #[tokio::test]
    async fn chain_view_account_method_is_registered() {
        let module = attach_gateway(test_gateway()).expect("module should register");

        let (response, _stream) = module
            .raw_json_request(
                r#"{"jsonrpc":"2.0","method":"chain.viewAccount","params":{},"id":1}"#,
                1,
            )
            .await
            .expect("raw request should return a response");

        assert!(
            response.get().contains("-32602"),
            "unexpected response: {response:?}"
        );
    }

    #[tokio::test]
    async fn tx_function_call_method_is_registered() {
        let module = attach_gateway(test_gateway()).expect("module should register");

        let (response, _stream) = module
            .raw_json_request(
                r#"{"jsonrpc":"2.0","method":"tx.functionCall","params":{},"id":1}"#,
                1,
            )
            .await
            .expect("raw request should return a response");

        assert!(
            response.get().contains("-32602"),
            "unexpected response: {response:?}"
        );
    }

    #[tokio::test]
    async fn storage_deposit_method_is_registered() {
        let module = attach_gateway(test_gateway()).expect("module should register");

        let (response, _stream) = module
            .raw_json_request(
                r#"{"jsonrpc":"2.0","method":"storage.deposit","params":{},"id":1}"#,
                1,
            )
            .await
            .expect("raw request should return a response");

        assert!(
            response.get().contains("-32602"),
            "unexpected response: {response:?}"
        );
    }
}
