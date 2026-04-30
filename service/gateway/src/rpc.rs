use jsonrpsee::{
    core::{RegisterMethodError, RpcResult},
    types::ErrorObjectOwned,
    RpcModule,
};
use templar_gateway_core::{
    Dispatch as CoreDispatch, DispatchRead, GatewayError, HasIdempotencyKey, HasNearClient,
    HasSignerAccountId, PlanWrite,
};
use templar_gateway_oracle::{
    Dispatch as OracleDispatch, ProvidesPythSource, ProvidesRedStoneSource,
};
use templar_gateway_types::{
    account, contract, ft, lst_oracle, market, mt, op, oracle, proxy_oracle,
    proxy_oracle_governance, proxy_oracle_owner, pyth, redstone, ref_finance, registry,
    rpc::common::WriteOperationResult, storage, token, tx, universal_account, MethodSpec,
};

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
fn register_core_gateway_methods<ContextType>(
    builder: &mut GatewayRpcBuilder<ContextType>,
) -> Result<(), RegisterMethodError>
where
    ContextType: HasNearClient + std::marker::Unpin,
{
    builder.register_read::<account::Get, CoreDispatch>()?;
    builder.register_write::<account::Delete, CoreDispatch>()?;
    builder.register_read::<contract::ViewFunction, CoreDispatch>()?;
    builder.register_read::<contract::GetKind, CoreDispatch>()?;
    builder.register_read::<contract::GetVersion, CoreDispatch>()?;
    builder.register_read::<ft::GetBalanceOf, CoreDispatch>()?;
    builder.register_write::<ft::Transfer, CoreDispatch>()?;
    builder.register_write::<ft::TransferCall, CoreDispatch>()?;
    builder.register_read::<lst_oracle::GetOracleId, CoreDispatch>()?;
    builder.register_read::<lst_oracle::ListTransformers, CoreDispatch>()?;
    builder.register_read::<lst_oracle::GetTransformer, CoreDispatch>()?;
    builder.register_read::<market::GetConfiguration, CoreDispatch>()?;
    builder.register_read::<market::GetCurrentSnapshot, CoreDispatch>()?;
    builder.register_read::<market::GetFinalizedSnapshotsLen, CoreDispatch>()?;
    builder.register_read::<market::ListFinalizedSnapshots, CoreDispatch>()?;
    builder.register_read::<market::GetBorrowAssetMetrics, CoreDispatch>()?;
    builder.register_read::<market::ListBorrowPositions, CoreDispatch>()?;
    builder.register_read::<market::GetBorrowPosition, CoreDispatch>()?;
    builder.register_read::<market::GetBorrowPositionPendingInterest, CoreDispatch>()?;
    builder.register_read::<market::GetBorrowStatus, CoreDispatch>()?;
    builder.register_read::<market::ListSupplyPositions, CoreDispatch>()?;
    builder.register_read::<market::GetSupplyPosition, CoreDispatch>()?;
    builder.register_read::<market::GetSupplyPositionPendingYield, CoreDispatch>()?;
    builder.register_read::<market::GetSupplyWithdrawalRequestStatus, CoreDispatch>()?;
    builder.register_read::<market::GetSupplyWithdrawalQueueStatus, CoreDispatch>()?;
    builder.register_read::<market::GetLastYieldRate, CoreDispatch>()?;
    builder.register_read::<market::GetStaticYield, CoreDispatch>()?;
    builder.register_write::<market::Create, CoreDispatch>()?;
    builder.register_write::<market::Borrow, CoreDispatch>()?;
    builder.register_write::<market::Supply, CoreDispatch>()?;
    builder.register_write::<market::WithdrawCollateral, CoreDispatch>()?;
    builder.register_write::<market::ApplyInterest, CoreDispatch>()?;
    builder.register_write::<market::Repay, CoreDispatch>()?;
    builder.register_write::<market::CreateSupplyWithdrawalRequest, CoreDispatch>()?;
    builder.register_write::<market::CancelSupplyWithdrawalRequest, CoreDispatch>()?;
    builder.register_write::<market::ExecuteNextSupplyWithdrawalRequest, CoreDispatch>()?;
    builder.register_write::<market::WithdrawSupply, CoreDispatch>()?;
    builder.register_write::<market::Liquidate, CoreDispatch>()?;
    builder.register_write::<market::HarvestYield, CoreDispatch>()?;
    builder.register_write::<market::AccumulateStaticYield, CoreDispatch>()?;
    builder.register_write::<market::WithdrawStaticYield, CoreDispatch>()?;
    builder.register_read::<mt::GetBalanceOf, CoreDispatch>()?;
    builder.register_read::<mt::GetBatchBalanceOf, CoreDispatch>()?;
    builder.register_read::<mt::GetSupply, CoreDispatch>()?;
    builder.register_read::<mt::GetBatchSupply, CoreDispatch>()?;
    builder.register_write::<mt::Transfer, CoreDispatch>()?;
    builder.register_write::<mt::TransferCall, CoreDispatch>()?;
    builder.register_read::<pyth::ListEmaPricesNoOlderThan, CoreDispatch>()?;
    builder.register_read::<pyth::ListEmaPricesUnsafe, CoreDispatch>()?;
    builder.register_write::<pyth::UpdatePriceFeeds, CoreDispatch>()?;
    builder.register_read::<proxy_oracle::ListProxies, CoreDispatch>()?;
    builder.register_read::<proxy_oracle::GetProxy, CoreDispatch>()?;
    builder.register_read::<proxy_oracle::PriceFeedExists, CoreDispatch>()?;
    builder.register_read::<proxy_oracle_governance::GetNextId, CoreDispatch>()?;
    builder.register_read::<proxy_oracle_governance::GetTtl, CoreDispatch>()?;
    builder.register_read::<proxy_oracle_governance::GetCount, CoreDispatch>()?;
    builder.register_read::<proxy_oracle_governance::List, CoreDispatch>()?;
    builder.register_read::<proxy_oracle_governance::Get, CoreDispatch>()?;
    builder.register_write::<proxy_oracle_governance::Create, CoreDispatch>()?;
    builder.register_write::<proxy_oracle_governance::Cancel, CoreDispatch>()?;
    builder.register_write::<proxy_oracle_governance::Execute, CoreDispatch>()?;
    builder.register_read::<proxy_oracle_owner::GetOwner, CoreDispatch>()?;
    builder.register_read::<proxy_oracle_owner::GetProposedOwner, CoreDispatch>()?;
    builder.register_write::<proxy_oracle_owner::ProposeOwner, CoreDispatch>()?;
    builder.register_write::<proxy_oracle_owner::AcceptOwner, CoreDispatch>()?;
    builder.register_write::<proxy_oracle_owner::RenounceOwner, CoreDispatch>()?;
    builder.register_read::<redstone::GetConfig, CoreDispatch>()?;
    builder.register_read::<redstone::ReadPriceData, CoreDispatch>()?;
    builder.register_read::<redstone::ListRole, CoreDispatch>()?;
    builder.register_write::<redstone::SetRole, CoreDispatch>()?;
    builder.register_write::<redstone::WritePrices, CoreDispatch>()?;
    builder.register_read::<ref_finance::GetPools, CoreDispatch>()?;
    builder.register_read::<registry::GetDeployment, CoreDispatch>()?;
    builder.register_read::<registry::ListDeployments, CoreDispatch>()?;
    builder.register_read::<registry::ListDeploymentsByKind, CoreDispatch>()?;
    builder.register_read::<registry::ListVersions, CoreDispatch>()?;
    builder.register_write::<registry::AddVersion, CoreDispatch>()?;
    builder.register_write::<registry::RemoveVersion, CoreDispatch>()?;
    builder.register_write::<registry::Deploy, CoreDispatch>()?;
    builder.register_read::<storage::GetBalanceBounds, CoreDispatch>()?;
    builder.register_read::<storage::GetBalanceOf, CoreDispatch>()?;
    builder.register_write::<storage::Deposit, CoreDispatch>()?;
    builder.register_write::<storage::EnsureDeposit, CoreDispatch>()?;
    builder.register_write::<storage::Unregister, CoreDispatch>()?;
    builder.register_read::<token::GetBalanceOf, CoreDispatch>()?;
    builder.register_write::<token::Transfer, CoreDispatch>()?;
    builder.register_write::<token::TransferCall, CoreDispatch>()?;
    builder.register_read::<tx::Get, CoreDispatch>()?;
    builder.register_write::<tx::FunctionCall, CoreDispatch>()?;
    builder.register_write::<tx::Transfer, CoreDispatch>()?;
    builder.register_write::<tx::DeployContract, CoreDispatch>()?;
    builder.register_write::<tx::DeployAndInit, CoreDispatch>()?;
    builder.register_read::<universal_account::GetKey, CoreDispatch>()?;
    builder.register_write::<universal_account::Execute, CoreDispatch>()?;
    builder.register_write::<universal_account::Create, CoreDispatch>()?;
    builder.register_operation_get()?;
    Ok(())
}

fn register_oracle_gateway_methods<ContextType>(
    builder: &mut GatewayRpcBuilder<ContextType>,
) -> Result<(), RegisterMethodError>
where
    ContextType: HasNearClient + ProvidesPythSource + ProvidesRedStoneSource + std::marker::Unpin,
{
    builder.register_read::<oracle::GetPriceResolutionDependencies, OracleDispatch>()?;
    builder.register_read::<oracle::ResolvePrice, OracleDispatch>()?;
    builder.register_read::<oracle::ResolvePrices, OracleDispatch>()?;
    builder.register_read::<oracle::GetPrice, OracleDispatch>()?;
    builder.register_read::<oracle::GetPrices, OracleDispatch>()?;
    builder.register_write::<oracle::UpdatePyth, OracleDispatch>()?;
    builder.register_write::<oracle::UpdateRedStone, OracleDispatch>()?;
    builder.register_write::<oracle::UpdatePrices, OracleDispatch>()?;
    Ok(())
}

pub fn attach_gateway<ContextType>(
    service: GatewayService<ContextType>,
) -> Result<RpcModule<GatewayService<ContextType>>, RegisterMethodError>
where
    ContextType: HasNearClient + ProvidesPythSource + ProvidesRedStoneSource + std::marker::Unpin,
{
    let mut builder = GatewayRpcBuilder::new(service);
    register_core_gateway_methods(&mut builder)?;
    register_oracle_gateway_methods(&mut builder)?;
    Ok(builder.finish())
}

#[cfg(test)]
mod tests;
