use blockchain_gateway_core::{chain, market, registry, storage, tx, universal_account};
use blockchain_gateway_near::{
    actor::{read::ReadRpcRequest, write::WriteRpcRequest},
    GatewayError, GatewayService,
};
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

fn register_write<Spec: WriteRpcRequest>(
    module: &mut RpcModule<GatewayService>,
) -> Result<(), RegisterMethodError> {
    module.register_async_method(Spec::RPC_METHOD, move |params, service, _| async move {
        let params: Spec::Input = params.parse()?;
        let result = service
            .request_write::<Spec>(params)
            .await
            .map_err(map_gateway_error)?;
        RpcResult::Ok(result)
    })?;

    Ok(())
}

fn register_read<Spec: ReadRpcRequest>(
    module: &mut RpcModule<GatewayService>,
) -> Result<(), RegisterMethodError> {
    module.register_async_method(Spec::RPC_METHOD, move |params, service, _| async move {
        let params: Spec::Input = params.parse()?;
        let result = service
            .request_read::<Spec>(params)
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

    register_read::<chain::ViewAccount>(&mut m)?;
    register_read::<chain::ViewFunction>(&mut m)?;
    register_read::<chain::GetTransaction>(&mut m)?;
    register_read::<registry::ListDeployments>(&mut m)?;
    register_read::<registry::ListVersions>(&mut m)?;
    register_read::<market::GetConfiguration>(&mut m)?;
    register_read::<market::ListBorrowPositions>(&mut m)?;
    register_read::<storage::GetBalanceBounds>(&mut m)?;
    register_read::<storage::GetBalanceOf>(&mut m)?;
    register_write::<storage::Deposit>(&mut m)?;
    register_read::<universal_account::GetKey>(&mut m)?;
    register_write::<tx::FunctionCall>(&mut m)?;

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
        let near = blockchain_gateway_near::NearClient::new(network);
        let signer = near_api::Signer::from_secret_key(
            "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q"
                .parse()
                .expect("valid test secret key"),
        )
        .expect("signer should initialize");
        let account_id = ManagedAccountId("test.near".parse().expect("valid signer account id"));
        GatewayService::spawn(
            near,
            HashMap::from([(
                account_id,
                blockchain_gateway_near::ManagedSigner {
                    signer,
                    key_count: 1,
                },
            )]),
        )
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
