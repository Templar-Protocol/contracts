use jsonrpsee::{
    core::{RegisterMethodError, RpcResult},
    types::ErrorObjectOwned,
    RpcModule,
};
use templar_gateway_core::{
    DispatchRead, GatewayError, HasIdempotencyKey, HasNearClient, HasSignerAccountId, PlanWrite,
};
use templar_gateway_methods_dispatch::Dispatch as MethodsDispatch;
use templar_gateway_methods_spec::{
    account, contract, ft, lst_oracle, market, mt, op, oracle, proxy_oracle,
    proxy_oracle_governance, proxy_oracle_owner, pyth, redstone, ref_finance, registry, storage,
    token, tx, universal_account,
};
use templar_gateway_oracle_updates_dispatch::{
    Dispatch as OracleUpdatesDispatch, ProvidesPythSource, ProvidesRedStoneSource,
};
use templar_gateway_oracle_updates_spec::oracle as oracle_updates;
use templar_gateway_types::{rpc::common::WriteOperationResult, MethodSpec};

use crate::gateway_service::GatewayService;

const GATEWAY_SERVER_ERROR_CODE: i32 = -32000;

#[allow(clippy::needless_pass_by_value)]
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
        Spec::Input: HasIdempotencyKey + HasSignerAccountId,
        Impl: PlanWrite<Spec, ContextType>,
    {
        self.module.register_async_method(
            Spec::RPC_METHOD,
            move |params, service, _| async move {
                let params: Spec::Input = params.parse()?;
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
                let params: Spec::Input = params.parse()?;
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
                let params: <op::Get as MethodSpec>::Input = params.parse()?;
                let result = service
                    .get_operation(&params.params.operation_id)
                    .await
                    .map_err(map_gateway_error)?;
                RpcResult::Ok(op::GetResult { operation: result })
            },
        )?;
        Ok(())
    }
}

#[allow(clippy::too_many_lines)]
fn register_gateway_methods<ContextType>(
    builder: &mut GatewayRpcBuilder<ContextType>,
) -> Result<(), RegisterMethodError>
where
    ContextType: HasNearClient + ProvidesPythSource + ProvidesRedStoneSource + std::marker::Unpin,
{
    builder.register_read::<account::Get, MethodsDispatch>()?;
    builder.register_write::<account::Delete, MethodsDispatch>()?;
    builder.register_read::<contract::ViewFunction, MethodsDispatch>()?;
    builder.register_read::<contract::GetKind, MethodsDispatch>()?;
    builder.register_read::<contract::GetVersion, MethodsDispatch>()?;
    builder.register_read::<ft::GetBalanceOf, MethodsDispatch>()?;
    builder.register_write::<ft::Transfer, MethodsDispatch>()?;
    builder.register_write::<ft::TransferCall, MethodsDispatch>()?;
    builder.register_read::<lst_oracle::GetOracleId, MethodsDispatch>()?;
    builder.register_read::<lst_oracle::ListTransformers, MethodsDispatch>()?;
    builder.register_read::<lst_oracle::GetTransformer, MethodsDispatch>()?;
    builder.register_read::<market::GetConfiguration, MethodsDispatch>()?;
    builder.register_read::<market::GetCurrentSnapshot, MethodsDispatch>()?;
    builder.register_read::<market::GetFinalizedSnapshotsLen, MethodsDispatch>()?;
    builder.register_read::<market::ListFinalizedSnapshots, MethodsDispatch>()?;
    builder.register_read::<market::GetBorrowAssetMetrics, MethodsDispatch>()?;
    builder.register_read::<market::ListBorrowPositions, MethodsDispatch>()?;
    builder.register_read::<market::GetBorrowPosition, MethodsDispatch>()?;
    builder.register_read::<market::GetBorrowPositionPendingInterest, MethodsDispatch>()?;
    builder.register_read::<market::GetBorrowStatus, MethodsDispatch>()?;
    builder.register_read::<market::ListSupplyPositions, MethodsDispatch>()?;
    builder.register_read::<market::GetSupplyPosition, MethodsDispatch>()?;
    builder.register_read::<market::GetSupplyPositionPendingYield, MethodsDispatch>()?;
    builder.register_read::<market::GetSupplyWithdrawalRequestStatus, MethodsDispatch>()?;
    builder.register_read::<market::GetSupplyWithdrawalQueueStatus, MethodsDispatch>()?;
    builder.register_read::<market::GetLastYieldRate, MethodsDispatch>()?;
    builder.register_read::<market::GetStaticYield, MethodsDispatch>()?;
    builder.register_write::<market::Create, MethodsDispatch>()?;
    builder.register_write::<market::Borrow, MethodsDispatch>()?;
    builder.register_write::<market::Supply, MethodsDispatch>()?;
    builder.register_write::<market::WithdrawCollateral, MethodsDispatch>()?;
    builder.register_write::<market::ApplyInterest, MethodsDispatch>()?;
    builder.register_write::<market::Repay, MethodsDispatch>()?;
    builder.register_write::<market::CreateSupplyWithdrawalRequest, MethodsDispatch>()?;
    builder.register_write::<market::CancelSupplyWithdrawalRequest, MethodsDispatch>()?;
    builder.register_write::<market::ExecuteNextSupplyWithdrawalRequest, MethodsDispatch>()?;
    builder.register_write::<market::WithdrawSupply, MethodsDispatch>()?;
    builder.register_write::<market::Liquidate, MethodsDispatch>()?;
    builder.register_write::<market::HarvestYield, MethodsDispatch>()?;
    builder.register_write::<market::AccumulateStaticYield, MethodsDispatch>()?;
    builder.register_write::<market::WithdrawStaticYield, MethodsDispatch>()?;
    builder.register_read::<mt::GetBalanceOf, MethodsDispatch>()?;
    builder.register_read::<mt::GetBatchBalanceOf, MethodsDispatch>()?;
    builder.register_read::<mt::GetSupply, MethodsDispatch>()?;
    builder.register_read::<mt::GetBatchSupply, MethodsDispatch>()?;
    builder.register_write::<mt::Transfer, MethodsDispatch>()?;
    builder.register_write::<mt::TransferCall, MethodsDispatch>()?;
    builder.register_read::<proxy_oracle::ListProxies, MethodsDispatch>()?;
    builder.register_read::<proxy_oracle::GetProxy, MethodsDispatch>()?;
    builder.register_read::<proxy_oracle::PriceFeedExists, MethodsDispatch>()?;
    builder.register_read::<proxy_oracle_governance::GetNextId, MethodsDispatch>()?;
    builder.register_read::<proxy_oracle_governance::GetTtl, MethodsDispatch>()?;
    builder.register_read::<proxy_oracle_governance::GetCount, MethodsDispatch>()?;
    builder.register_read::<proxy_oracle_governance::List, MethodsDispatch>()?;
    builder.register_read::<proxy_oracle_governance::Get, MethodsDispatch>()?;
    builder.register_write::<proxy_oracle_governance::Create, MethodsDispatch>()?;
    builder.register_write::<proxy_oracle_governance::Cancel, MethodsDispatch>()?;
    builder.register_write::<proxy_oracle_governance::Execute, MethodsDispatch>()?;
    builder.register_read::<proxy_oracle_owner::GetOwner, MethodsDispatch>()?;
    builder.register_read::<proxy_oracle_owner::GetProposedOwner, MethodsDispatch>()?;
    builder.register_write::<proxy_oracle_owner::ProposeOwner, MethodsDispatch>()?;
    builder.register_write::<proxy_oracle_owner::AcceptOwner, MethodsDispatch>()?;
    builder.register_write::<proxy_oracle_owner::RenounceOwner, MethodsDispatch>()?;
    builder.register_read::<ref_finance::GetPools, MethodsDispatch>()?;
    builder.register_read::<registry::GetDeployment, MethodsDispatch>()?;
    builder.register_read::<registry::ListDeployments, MethodsDispatch>()?;
    builder.register_read::<registry::ListDeploymentsByKind, MethodsDispatch>()?;
    builder.register_read::<registry::ListVersions, MethodsDispatch>()?;
    builder.register_write::<registry::AddVersion, MethodsDispatch>()?;
    builder.register_write::<registry::RemoveVersion, MethodsDispatch>()?;
    builder.register_write::<registry::Deploy, MethodsDispatch>()?;
    builder.register_read::<storage::GetBalanceBounds, MethodsDispatch>()?;
    builder.register_read::<storage::GetBalanceOf, MethodsDispatch>()?;
    builder.register_write::<storage::Deposit, MethodsDispatch>()?;
    builder.register_write::<storage::EnsureDeposit, MethodsDispatch>()?;
    builder.register_write::<storage::Unregister, MethodsDispatch>()?;
    builder.register_read::<token::GetBalanceOf, MethodsDispatch>()?;
    builder.register_write::<token::Transfer, MethodsDispatch>()?;
    builder.register_write::<token::TransferCall, MethodsDispatch>()?;
    builder.register_read::<tx::Get, MethodsDispatch>()?;
    builder.register_write::<tx::FunctionCall, MethodsDispatch>()?;
    builder.register_write::<tx::Transfer, MethodsDispatch>()?;
    builder.register_write::<tx::DeployContract, MethodsDispatch>()?;
    builder.register_write::<tx::DeployAndInit, MethodsDispatch>()?;
    builder.register_read::<universal_account::GetKey, MethodsDispatch>()?;
    builder.register_write::<universal_account::Execute, MethodsDispatch>()?;
    builder.register_write::<universal_account::Create, MethodsDispatch>()?;
    builder.register_operation_get()?;
    builder.register_read::<oracle::GetPriceResolutionDependencies, MethodsDispatch>()?;
    builder.register_read::<oracle::ResolvePrice, MethodsDispatch>()?;
    builder.register_read::<oracle::ResolvePrices, MethodsDispatch>()?;
    builder.register_read::<oracle::GetPrice, MethodsDispatch>()?;
    builder.register_read::<oracle::GetPrices, MethodsDispatch>()?;
    builder.register_write::<oracle_updates::UpdatePyth, OracleUpdatesDispatch>()?;
    builder.register_write::<oracle_updates::UpdateRedStone, OracleUpdatesDispatch>()?;
    builder.register_write::<oracle_updates::UpdatePrices, OracleUpdatesDispatch>()?;
    builder.register_read::<pyth::ListEmaPricesNoOlderThan, MethodsDispatch>()?;
    builder.register_read::<pyth::ListEmaPricesUnsafe, MethodsDispatch>()?;
    builder.register_write::<pyth::UpdatePriceFeeds, MethodsDispatch>()?;
    builder.register_read::<redstone::GetConfig, MethodsDispatch>()?;
    builder.register_read::<redstone::ReadPriceData, MethodsDispatch>()?;
    builder.register_read::<redstone::ListRole, MethodsDispatch>()?;
    builder.register_write::<redstone::SetRole, MethodsDispatch>()?;
    builder.register_write::<redstone::WritePrices, MethodsDispatch>()?;
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
