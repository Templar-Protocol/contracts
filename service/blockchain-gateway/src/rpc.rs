use blockchain_gateway_core::{
    chain, market, registry, storage, universal_account, MethodSpec, ReadMethodSpec,
};
use blockchain_gateway_near::{service, GatewayError, GatewayService};
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

type GatewayMethodHandler<Spec> = for<'a> fn(
    &'a GatewayService,
    <Spec as MethodSpec>::Input,
) -> futures::future::BoxFuture<
    'a,
    blockchain_gateway_near::GatewayResult<<Spec as MethodSpec>::Output>,
>;

fn register_read_method<Spec>(
    module: &mut RpcModule<GatewayService>,
    handler: GatewayMethodHandler<Spec>,
) -> Result<(), RegisterMethodError>
where
    Spec: ReadMethodSpec,
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

    register_read_method::<chain::ViewAccount>(&mut m, service::chain::view_account)?;
    register_read_method::<chain::ViewFunction>(&mut m, service::chain::view_function)?;
    register_read_method::<chain::GetTransaction>(&mut m, service::chain::get_transaction)?;
    register_read_method::<registry::ListDeployments>(&mut m, service::registry::list_deployments)?;
    register_read_method::<registry::ListVersions>(&mut m, service::registry::list_versions)?;
    register_read_method::<market::GetConfiguration>(&mut m, service::market::get_configuration)?;
    register_read_method::<market::ListBorrowPositions>(
        &mut m,
        service::market::list_borrow_positions,
    )?;
    register_read_method::<storage::GetBalanceBounds>(
        &mut m,
        service::storage::get_balance_bounds,
    )?;
    register_read_method::<storage::GetBalanceOf>(&mut m, service::storage::get_balance_of)?;
    register_read_method::<universal_account::GetKey>(&mut m, service::universal_account::get_key)?;

    Ok(m)
}
