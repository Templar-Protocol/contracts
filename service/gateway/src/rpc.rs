use jsonrpsee::{
    core::{RegisterMethodError, RpcResult},
    types::ErrorObjectOwned,
    RpcModule,
};
use templar_gateway_core::{DispatchRead, GatewayError, HasNearClient, PlanWrite};
use templar_gateway_methods_dispatch::Dispatch as MethodsDispatch;
use templar_gateway_methods_spec::op;
use templar_gateway_oracle_updates_dispatch::{
    Dispatch as OracleUpdatesDispatch, ProvidesPythSource, ProvidesRedStoneSource,
};
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    MethodSpec,
};

use crate::gateway_service::GatewayService;

const GATEWAY_SERVER_ERROR_CODE: i32 = -32000;

#[allow(clippy::needless_pass_by_value, reason = "ease of use")]
fn map_gateway_error(error: GatewayError) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(GATEWAY_SERVER_ERROR_CODE, error.to_string(), None::<()>)
}

struct GatewayRpcBuilder<ContextType: Clone + Send + std::marker::Unpin + 'static> {
    module: RpcModule<GatewayService<ContextType>>,
}

impl<ContextType: HasNearClient + std::marker::Unpin> GatewayRpcBuilder<ContextType> {
    fn new(service: GatewayService<ContextType>) -> Self {
        Self {
            module: RpcModule::new(service),
        }
    }

    fn finish(self) -> RpcModule<GatewayService<ContextType>> {
        self.module
    }

    fn register_write<Spec, Impl>(&mut self) -> Result<(), RegisterMethodError>
    where
        Spec: MethodSpec<Output = WriteOperationResult> + 'static,
        Impl: PlanWrite<Spec, ContextType>,
    {
        self.module.register_async_method(
            Spec::RPC_METHOD,
            move |params, service, _| async move {
                let params: WriteRequest<Spec> = params.parse()?;
                let result = service
                    .request_write::<Spec, Impl>(params)
                    .await
                    .map_err(map_gateway_error)?;
                RpcResult::Ok(result)
            },
        )?;
        Ok(())
    }

    fn register_read<Spec, Impl>(&mut self) -> Result<(), RegisterMethodError>
    where
        Spec: MethodSpec + 'static,
        Impl: DispatchRead<Spec, ContextType>,
    {
        self.module.register_async_method(
            Spec::RPC_METHOD,
            move |params, service, _| async move {
                let params: Spec = params.parse()?;
                let result = service
                    .request_read::<Spec, Impl>(params)
                    .await
                    .map_err(map_gateway_error)?;
                RpcResult::Ok(result)
            },
        )?;
        Ok(())
    }

    fn register_operation_get(&mut self) -> Result<(), RegisterMethodError> {
        self.module.register_async_method(
            op::Get::RPC_METHOD,
            move |params, service, _| async move {
                let params: op::Get = params.parse()?;
                let result = service
                    .get_operation(&params.operation_id)
                    .await
                    .map_err(map_gateway_error)?;
                RpcResult::Ok(op::GetResult { operation: result })
            },
        )?;
        Ok(())
    }
}

fn register_gateway_methods<ContextType>(
    builder: &mut GatewayRpcBuilder<ContextType>,
) -> Result<(), RegisterMethodError>
where
    ContextType: HasNearClient + ProvidesPythSource + ProvidesRedStoneSource + std::marker::Unpin,
{
    // The method lists live in the spec crates (`for_each_read_method!` /
    // `for_each_write_method!` / `for_each_oracle_update_method!`) and are shared
    // with the catalog crate, so registration and the generated method reference
    // cannot drift apart. These callbacks supply the dispatcher per kind.
    macro_rules! register_read {
        ($spec:ty) => {
            builder.register_read::<$spec, MethodsDispatch>()?;
        };
    }
    macro_rules! register_write {
        ($spec:ty) => {
            builder.register_write::<$spec, MethodsDispatch>()?;
        };
    }
    macro_rules! register_oracle_write {
        ($spec:ty) => {
            builder.register_write::<$spec, OracleUpdatesDispatch>()?;
        };
    }

    templar_gateway_methods_spec::for_each_read_method!(register_read);
    templar_gateway_methods_spec::for_each_write_method!(register_write);
    templar_gateway_oracle_updates_spec::for_each_oracle_update_method!(register_oracle_write);

    // `op.get` reads the operation store rather than the chain, so it is the one
    // method registered outside the shared macros (it has no `DispatchRead`).
    builder.register_operation_get()?;
    Ok(())
}

pub fn attach_gateway<ContextType>(
    service: GatewayService<ContextType>,
) -> Result<RpcModule<GatewayService<ContextType>>, RegisterMethodError>
where
    ContextType: HasNearClient + ProvidesPythSource + ProvidesRedStoneSource + std::marker::Unpin,
{
    let mut builder = GatewayRpcBuilder::new(service);
    register_gateway_methods(&mut builder)?;
    Ok(builder.finish())
}

#[cfg(test)]
mod tests;
