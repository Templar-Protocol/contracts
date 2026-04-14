use blockchain_gateway_core::{
    chain, market, registry, storage, tx, universal_account, MethodSpec,
};
use blockchain_gateway_near::{service, GatewayError, GatewayResult, GatewayService};
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

type GatewayMethodHandler<Spec> =
    for<'a> fn(
        &'a GatewayService,
        <Spec as MethodSpec>::Input,
    ) -> BoxFuture<'a, GatewayResult<<Spec as MethodSpec>::Output>>;

fn register<Spec>(
    module: &mut RpcModule<GatewayService>,
    handler: GatewayMethodHandler<Spec>,
) -> Result<(), RegisterMethodError>
where
    Spec: MethodSpec,
    Spec::Input: Send + 'static,
    Spec::Output: Clone + Send + Sync + 'static,
{
    module.register_async_method(Spec::RPC_METHOD, move |params, service, _| async move {
        let params: Spec::Input = params.parse()?;
        let result = handler(service.as_ref(), params)
            .await
            .map_err(map_gateway_error)?;
        RpcResult::Ok(result)
    })?;

    Ok(())
}

pub fn attach_gateway(
    service: GatewayService,
) -> Result<RpcModule<GatewayService>, RegisterMethodError> {
    let mut m = RpcModule::new(service);

    register::<chain::ViewAccount>(&mut m, service::chain::view_account)?;
    register::<chain::ViewFunction>(&mut m, service::chain::view_function)?;
    register::<chain::GetTransaction>(&mut m, service::chain::get_transaction)?;
    register::<registry::ListDeployments>(&mut m, service::registry::list_deployments)?;
    register::<registry::ListVersions>(&mut m, service::registry::list_versions)?;
    register::<market::GetConfiguration>(&mut m, service::market::get_configuration)?;
    register::<market::ListBorrowPositions>(&mut m, service::market::list_borrow_positions)?;
    register::<storage::GetBalanceBounds>(&mut m, service::storage::get_balance_bounds)?;
    register::<storage::GetBalanceOf>(&mut m, service::storage::get_balance_of)?;
    register::<storage::Deposit>(&mut m, service::storage::deposit)?;
    register::<universal_account::GetKey>(&mut m, service::universal_account::get_key)?;
    register::<tx::FunctionCall>(&mut m, service::tx::function_call)?;

    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

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
        let writer = blockchain_gateway_near::NearWriteClient::new(
            network,
            BTreeMap::from([(
                "test.near".parse().expect("valid signer account id"),
                signer,
            )]),
        );
        GatewayService::new(near, writer)
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
