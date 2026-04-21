use blockchain_gateway_core::{
    account, contract, ft, lst_oracle, market, mt, op, oracle, proxy_oracle,
    proxy_oracle_governance, proxy_oracle_owner, pyth, redstone, ref_finance, registry, storage,
    token, tx, universal_account, MethodSpec,
};
use blockchain_gateway_near::{
    actor::{DispatchRead, PlanWrite},
    GatewayError, GatewayService,
};
use jsonrpsee::{
    core::{RegisterMethodError, RpcResult},
    types::ErrorObjectOwned,
    RpcModule,
};

macro_rules! for_each_gateway_method {
    ($callback:ident, $target:expr) => {
        $callback!($target, read, account::Get);
        $callback!($target, write, account::Delete);
        $callback!($target, read, contract::ViewFunction);
        $callback!($target, read, contract::GetVersion);
        $callback!($target, read, ft::GetBalanceOf);
        $callback!($target, write, ft::Transfer);
        $callback!($target, write, ft::TransferCall);
        $callback!($target, read, lst_oracle::GetOracleId);
        $callback!($target, read, lst_oracle::ListTransformers);
        $callback!($target, read, lst_oracle::GetTransformer);
        $callback!($target, read, market::GetConfiguration);
        $callback!($target, read, market::GetCurrentSnapshot);
        $callback!($target, read, market::GetFinalizedSnapshotsLen);
        $callback!($target, read, market::ListFinalizedSnapshots);
        $callback!($target, read, market::GetBorrowAssetMetrics);
        $callback!($target, read, market::ListBorrowPositions);
        $callback!($target, read, market::GetBorrowPosition);
        $callback!($target, read, market::GetBorrowPositionPendingInterest);
        $callback!($target, read, market::GetBorrowStatus);
        $callback!($target, read, market::ListSupplyPositions);
        $callback!($target, read, market::GetSupplyPosition);
        $callback!($target, read, market::GetSupplyPositionPendingYield);
        $callback!($target, read, market::GetSupplyWithdrawalRequestStatus);
        $callback!($target, read, market::GetSupplyWithdrawalQueueStatus);
        $callback!($target, read, market::GetLastYieldRate);
        $callback!($target, read, market::GetStaticYield);
        $callback!($target, write, market::Create);
        $callback!($target, write, market::Borrow);
        $callback!($target, write, market::Supply);
        $callback!($target, write, market::WithdrawCollateral);
        $callback!($target, write, market::ApplyInterest);
        $callback!($target, write, market::Repay);
        $callback!($target, write, market::CreateSupplyWithdrawalRequest);
        $callback!($target, write, market::CancelSupplyWithdrawalRequest);
        $callback!($target, write, market::ExecuteNextSupplyWithdrawalRequest);
        $callback!($target, write, market::WithdrawSupply);
        $callback!($target, write, market::Liquidate);
        $callback!($target, write, market::HarvestYield);
        $callback!($target, write, market::AccumulateStaticYield);
        $callback!($target, write, market::WithdrawStaticYield);
        $callback!($target, read, mt::GetBalanceOf);
        $callback!($target, read, mt::GetBatchBalanceOf);
        $callback!($target, read, mt::GetSupply);
        $callback!($target, read, mt::GetBatchSupply);
        $callback!($target, write, mt::Transfer);
        $callback!($target, write, mt::TransferCall);
        $callback!($target, read, oracle::GetKind);
        $callback!($target, read, oracle::GetPriceResolutionDependencies);
        $callback!($target, read, oracle::ResolvePrice);
        $callback!($target, read, oracle::ResolvePrices);
        $callback!($target, read, oracle::GetPrice);
        $callback!($target, read, oracle::GetPrices);
        $callback!($target, write, oracle::UpdatePyth);
        $callback!($target, write, oracle::UpdateRedStone);
        $callback!($target, write, oracle::UpdatePrices);
        $callback!($target, read, pyth::ListEmaPricesNoOlderThan);
        $callback!($target, read, pyth::ListEmaPricesUnsafe);
        $callback!($target, write, pyth::UpdatePriceFeeds);
        $callback!($target, read, proxy_oracle::ListProxies);
        $callback!($target, read, proxy_oracle::GetProxy);
        $callback!($target, read, proxy_oracle::PriceFeedExists);
        $callback!($target, read, proxy_oracle_governance::GetNextId);
        $callback!($target, read, proxy_oracle_governance::GetTtl);
        $callback!($target, read, proxy_oracle_governance::GetCount);
        $callback!($target, read, proxy_oracle_governance::List);
        $callback!($target, read, proxy_oracle_governance::Get);
        $callback!($target, write, proxy_oracle_governance::Create);
        $callback!($target, write, proxy_oracle_governance::Cancel);
        $callback!($target, write, proxy_oracle_governance::Execute);
        $callback!($target, read, proxy_oracle_owner::GetOwner);
        $callback!($target, read, proxy_oracle_owner::GetProposedOwner);
        $callback!($target, write, proxy_oracle_owner::ProposeOwner);
        $callback!($target, write, proxy_oracle_owner::AcceptOwner);
        $callback!($target, write, proxy_oracle_owner::RenounceOwner);
        $callback!($target, read, redstone::GetConfig);
        $callback!($target, read, redstone::ReadPriceData);
        $callback!($target, read, redstone::ListRole);
        $callback!($target, write, redstone::SetRole);
        $callback!($target, write, redstone::WritePrices);
        $callback!($target, read, ref_finance::GetPools);
        $callback!($target, read, registry::GetDeployment);
        $callback!($target, read, registry::ListDeployments);
        $callback!($target, read, registry::ListVersions);
        $callback!($target, write, registry::AddVersion);
        $callback!($target, write, registry::RemoveVersion);
        $callback!($target, write, registry::Deploy);
        $callback!($target, read, storage::GetBalanceBounds);
        $callback!($target, read, storage::GetBalanceOf);
        $callback!($target, write, storage::Deposit);
        $callback!($target, write, storage::EnsureDeposit);
        $callback!($target, write, storage::Unregister);
        $callback!($target, read, token::GetBalanceOf);
        $callback!($target, write, token::Transfer);
        $callback!($target, write, token::TransferCall);
        $callback!($target, read, tx::Get);
        $callback!($target, write, tx::FunctionCall);
        $callback!($target, write, tx::Transfer);
        $callback!($target, write, tx::DeployContract);
        $callback!($target, write, tx::DeployAndInit);
        $callback!($target, read, universal_account::GetKey);
        $callback!($target, write, universal_account::Execute);
        $callback!($target, write, universal_account::Create);
    };
}

macro_rules! register_gateway_method {
    ($target:expr, read, $spec:path) => {
        register_read::<$spec>($target)?;
    };
    ($target:expr, write, $spec:path) => {
        register_write::<$spec>($target)?;
    };
}

macro_rules! push_gateway_method {
    ($target:expr, $kind:ident, $spec:path) => {
        $target.push(crate::openrpc::method::<$spec>());
    };
}

const GATEWAY_SERVER_ERROR_CODE: i32 = -32000;

#[allow(clippy::needless_pass_by_value)]
fn map_gateway_error(error: GatewayError) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(GATEWAY_SERVER_ERROR_CODE, error.to_string(), None::<()>)
}

fn register_write<Spec: PlanWrite>(
    module: &mut RpcModule<GatewayService>,
) -> Result<(), RegisterMethodError>
where
    Spec::Input: Clone + serde::Serialize,
    Spec::Output: serde::de::DeserializeOwned,
{
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

fn register_read<Spec: DispatchRead>(
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

fn discover_document() -> crate::openrpc::Document {
    let mut methods = Vec::new();
    for_each_gateway_method!(push_gateway_method, methods);
    methods.push(crate::openrpc::method::<op::Get>());
    methods.push(crate::openrpc::discover_method());
    crate::openrpc::discover(methods)
}

#[allow(clippy::too_many_lines)]
pub fn attach_gateway(
    service: GatewayService,
) -> Result<RpcModule<GatewayService>, RegisterMethodError> {
    let mut m = RpcModule::new(service);

    for_each_gateway_method!(register_gateway_method, &mut m);

    m.register_async_method(op::Get::RPC_METHOD, move |params, service, _| async move {
        let params: <op::Get as MethodSpec>::Input = params.parse()?;
        let result = service
            .get_operation(&params.params.operation_id)
            .await
            .map_err(map_gateway_error)?;
        RpcResult::Ok(op::GetResult { operation: result })
    })?;

    m.register_method("rpc.discover", move |_, _, _| {
        RpcResult::Ok(discover_document())
    })?;

    Ok(m)
}

#[cfg(test)]
mod tests;
