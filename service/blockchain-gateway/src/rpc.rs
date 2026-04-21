use blockchain_gateway_core::{
    account, contract, ft, lst_oracle, market, mt, op, oracle, proxy_oracle,
    proxy_oracle_governance, proxy_oracle_owner, pyth, redstone, ref_finance, registry, storage,
    token, tx, universal_account, MethodSpec, RpcMethodMeta,
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

const GATEWAY_SERVER_ERROR_CODE: i32 = -32000;

#[allow(clippy::needless_pass_by_value)]
fn map_gateway_error(error: GatewayError) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(GATEWAY_SERVER_ERROR_CODE, error.to_string(), None::<()>)
}

struct GatewayRpcBuilder {
    module: RpcModule<GatewayService>,
    methods: Vec<crate::openrpc::Method>,
}

impl GatewayRpcBuilder {
    fn new(service: GatewayService) -> Self {
        Self {
            module: RpcModule::new(service),
            methods: Vec::new(),
        }
    }

    fn register_write<Spec: PlanWrite + RpcMethodMeta>(&mut self) -> Result<(), RegisterMethodError>
    where
        Spec::Input: Clone + serde::Serialize,
        Spec::Output: serde::de::DeserializeOwned,
    {
        self.module.register_async_method(
            Spec::RPC_METHOD,
            move |params, service, _| async move {
                let params: Spec::Input = params.parse()?;
                let result = service
                    .request_write::<Spec>(params)
                    .await
                    .map_err(map_gateway_error)?;
                RpcResult::Ok(result)
            },
        )?;
        self.methods.push(crate::openrpc::method::<Spec>());
        Ok(())
    }

    fn register_read<Spec: DispatchRead + RpcMethodMeta>(
        &mut self,
    ) -> Result<(), RegisterMethodError> {
        self.module.register_async_method(
            Spec::RPC_METHOD,
            move |params, service, _| async move {
                let params: Spec::Input = params.parse()?;
                let result = service
                    .request_read::<Spec>(params)
                    .await
                    .map_err(map_gateway_error)?;
                RpcResult::Ok(result)
            },
        )?;
        self.methods.push(crate::openrpc::method::<Spec>());
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
        self.methods.push(crate::openrpc::method::<op::Get>());
        Ok(())
    }

    fn finish(mut self) -> Result<RpcModule<GatewayService>, RegisterMethodError> {
        self.methods.push(crate::openrpc::discover_method());
        let document = crate::openrpc::discover(self.methods);
        self.module
            .register_method("rpc.discover", move |_, _, _| {
                RpcResult::Ok(document.clone())
            })?;
        Ok(self.module)
    }
}

#[allow(clippy::too_many_lines)]
pub fn attach_gateway(
    service: GatewayService,
) -> Result<RpcModule<GatewayService>, RegisterMethodError> {
    let mut builder = GatewayRpcBuilder::new(service);

    builder.register_read::<account::Get>()?;
    builder.register_write::<account::Delete>()?;
    builder.register_read::<contract::ViewFunction>()?;
    builder.register_read::<contract::GetVersion>()?;
    builder.register_read::<ft::GetBalanceOf>()?;
    builder.register_write::<ft::Transfer>()?;
    builder.register_write::<ft::TransferCall>()?;
    builder.register_read::<lst_oracle::GetOracleId>()?;
    builder.register_read::<lst_oracle::ListTransformers>()?;
    builder.register_read::<lst_oracle::GetTransformer>()?;
    builder.register_read::<market::GetConfiguration>()?;
    builder.register_read::<market::GetCurrentSnapshot>()?;
    builder.register_read::<market::GetFinalizedSnapshotsLen>()?;
    builder.register_read::<market::ListFinalizedSnapshots>()?;
    builder.register_read::<market::GetBorrowAssetMetrics>()?;
    builder.register_read::<market::ListBorrowPositions>()?;
    builder.register_read::<market::GetBorrowPosition>()?;
    builder.register_read::<market::GetBorrowPositionPendingInterest>()?;
    builder.register_read::<market::GetBorrowStatus>()?;
    builder.register_read::<market::ListSupplyPositions>()?;
    builder.register_read::<market::GetSupplyPosition>()?;
    builder.register_read::<market::GetSupplyPositionPendingYield>()?;
    builder.register_read::<market::GetSupplyWithdrawalRequestStatus>()?;
    builder.register_read::<market::GetSupplyWithdrawalQueueStatus>()?;
    builder.register_read::<market::GetLastYieldRate>()?;
    builder.register_read::<market::GetStaticYield>()?;
    builder.register_write::<market::Create>()?;
    builder.register_write::<market::Borrow>()?;
    builder.register_write::<market::Supply>()?;
    builder.register_write::<market::WithdrawCollateral>()?;
    builder.register_write::<market::ApplyInterest>()?;
    builder.register_write::<market::Repay>()?;
    builder.register_write::<market::CreateSupplyWithdrawalRequest>()?;
    builder.register_write::<market::CancelSupplyWithdrawalRequest>()?;
    builder.register_write::<market::ExecuteNextSupplyWithdrawalRequest>()?;
    builder.register_write::<market::WithdrawSupply>()?;
    builder.register_write::<market::Liquidate>()?;
    builder.register_write::<market::HarvestYield>()?;
    builder.register_write::<market::AccumulateStaticYield>()?;
    builder.register_write::<market::WithdrawStaticYield>()?;
    builder.register_read::<mt::GetBalanceOf>()?;
    builder.register_read::<mt::GetBatchBalanceOf>()?;
    builder.register_read::<mt::GetSupply>()?;
    builder.register_read::<mt::GetBatchSupply>()?;
    builder.register_write::<mt::Transfer>()?;
    builder.register_write::<mt::TransferCall>()?;
    builder.register_read::<oracle::GetKind>()?;
    builder.register_read::<oracle::GetPriceResolutionDependencies>()?;
    builder.register_read::<oracle::ResolvePrice>()?;
    builder.register_read::<oracle::ResolvePrices>()?;
    builder.register_read::<oracle::GetPrice>()?;
    builder.register_read::<oracle::GetPrices>()?;
    builder.register_write::<oracle::UpdatePyth>()?;
    builder.register_write::<oracle::UpdateRedStone>()?;
    builder.register_write::<oracle::UpdatePrices>()?;
    builder.register_read::<pyth::ListEmaPricesNoOlderThan>()?;
    builder.register_read::<pyth::ListEmaPricesUnsafe>()?;
    builder.register_write::<pyth::UpdatePriceFeeds>()?;
    builder.register_read::<proxy_oracle::ListProxies>()?;
    builder.register_read::<proxy_oracle::GetProxy>()?;
    builder.register_read::<proxy_oracle::PriceFeedExists>()?;
    builder.register_read::<proxy_oracle_governance::GetNextId>()?;
    builder.register_read::<proxy_oracle_governance::GetTtl>()?;
    builder.register_read::<proxy_oracle_governance::GetCount>()?;
    builder.register_read::<proxy_oracle_governance::List>()?;
    builder.register_read::<proxy_oracle_governance::Get>()?;
    builder.register_write::<proxy_oracle_governance::Create>()?;
    builder.register_write::<proxy_oracle_governance::Cancel>()?;
    builder.register_write::<proxy_oracle_governance::Execute>()?;
    builder.register_read::<proxy_oracle_owner::GetOwner>()?;
    builder.register_read::<proxy_oracle_owner::GetProposedOwner>()?;
    builder.register_write::<proxy_oracle_owner::ProposeOwner>()?;
    builder.register_write::<proxy_oracle_owner::AcceptOwner>()?;
    builder.register_write::<proxy_oracle_owner::RenounceOwner>()?;
    builder.register_read::<redstone::GetConfig>()?;
    builder.register_read::<redstone::ReadPriceData>()?;
    builder.register_read::<redstone::ListRole>()?;
    builder.register_write::<redstone::SetRole>()?;
    builder.register_write::<redstone::WritePrices>()?;
    builder.register_read::<ref_finance::GetPools>()?;
    builder.register_read::<registry::GetDeployment>()?;
    builder.register_read::<registry::ListDeployments>()?;
    builder.register_read::<registry::ListVersions>()?;
    builder.register_write::<registry::AddVersion>()?;
    builder.register_write::<registry::RemoveVersion>()?;
    builder.register_write::<registry::Deploy>()?;
    builder.register_read::<storage::GetBalanceBounds>()?;
    builder.register_read::<storage::GetBalanceOf>()?;
    builder.register_write::<storage::Deposit>()?;
    builder.register_write::<storage::EnsureDeposit>()?;
    builder.register_write::<storage::Unregister>()?;
    builder.register_read::<token::GetBalanceOf>()?;
    builder.register_write::<token::Transfer>()?;
    builder.register_write::<token::TransferCall>()?;
    builder.register_read::<tx::Get>()?;
    builder.register_write::<tx::FunctionCall>()?;
    builder.register_write::<tx::Transfer>()?;
    builder.register_write::<tx::DeployContract>()?;
    builder.register_write::<tx::DeployAndInit>()?;
    builder.register_read::<universal_account::GetKey>()?;
    builder.register_write::<universal_account::Execute>()?;
    builder.register_write::<universal_account::Create>()?;
    builder.register_operation_get()?;

    builder.finish()
}

#[cfg(test)]
mod tests;
