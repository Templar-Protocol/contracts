use blockchain_gateway_core::{
    account, contract, ft, market, oracle, proxy_oracle, proxy_oracle_governance,
    proxy_oracle_owner, registry, storage, tx, universal_account,
};
use blockchain_gateway_near::{
    actor::{DispatchRead, DispatchWrite},
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

fn register_write<Spec: DispatchWrite>(
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

pub fn attach_gateway(
    service: GatewayService,
) -> Result<RpcModule<GatewayService>, RegisterMethodError> {
    let mut m = RpcModule::new(service);

    register_read::<account::Get>(&mut m)?;
    register_write::<account::Delete>(&mut m)?;
    register_read::<contract::ViewFunction>(&mut m)?;
    register_read::<contract::GetVersion>(&mut m)?;
    register_read::<ft::GetBalanceOf>(&mut m)?;
    register_write::<ft::Transfer>(&mut m)?;
    register_read::<market::GetConfiguration>(&mut m)?;
    register_read::<market::GetCurrentSnapshot>(&mut m)?;
    register_read::<market::GetFinalizedSnapshotsLen>(&mut m)?;
    register_read::<market::ListFinalizedSnapshots>(&mut m)?;
    register_read::<market::GetBorrowAssetMetrics>(&mut m)?;
    register_read::<market::ListBorrowPositions>(&mut m)?;
    register_read::<market::GetBorrowPosition>(&mut m)?;
    register_read::<market::GetBorrowPositionPendingInterest>(&mut m)?;
    register_read::<market::GetBorrowStatus>(&mut m)?;
    register_read::<market::ListSupplyPositions>(&mut m)?;
    register_read::<market::GetSupplyPosition>(&mut m)?;
    register_read::<market::GetSupplyPositionPendingYield>(&mut m)?;
    register_read::<market::GetSupplyWithdrawalRequestStatus>(&mut m)?;
    register_read::<market::GetSupplyWithdrawalQueueStatus>(&mut m)?;
    register_read::<market::GetLastYieldRate>(&mut m)?;
    register_read::<market::GetStaticYield>(&mut m)?;
    register_write::<market::Create>(&mut m)?;
    register_write::<market::Borrow>(&mut m)?;
    register_write::<market::Supply>(&mut m)?;
    register_write::<market::WithdrawCollateral>(&mut m)?;
    register_write::<market::ApplyInterest>(&mut m)?;
    register_write::<market::Repay>(&mut m)?;
    register_write::<market::CreateSupplyWithdrawalRequest>(&mut m)?;
    register_write::<market::CancelSupplyWithdrawalRequest>(&mut m)?;
    register_write::<market::ExecuteNextSupplyWithdrawalRequest>(&mut m)?;
    register_write::<market::WithdrawSupply>(&mut m)?;
    register_write::<market::Liquidate>(&mut m)?;
    register_write::<market::HarvestYield>(&mut m)?;
    register_write::<market::AccumulateStaticYield>(&mut m)?;
    register_write::<market::WithdrawStaticYield>(&mut m)?;
    register_read::<oracle::GetKind>(&mut m)?;
    register_read::<oracle::GetPriceResolutionDependencies>(&mut m)?;
    register_read::<oracle::ResolvePrice>(&mut m)?;
    register_read::<oracle::ResolvePrices>(&mut m)?;
    register_read::<oracle::GetPrice>(&mut m)?;
    register_read::<oracle::GetPrices>(&mut m)?;
    register_write::<oracle::UpdatePyth>(&mut m)?;
    register_write::<oracle::UpdateRedStone>(&mut m)?;
    register_write::<oracle::UpdatePrices>(&mut m)?;
    register_read::<proxy_oracle::ListProxies>(&mut m)?;
    register_read::<proxy_oracle::GetProxy>(&mut m)?;
    register_read::<proxy_oracle::PriceFeedExists>(&mut m)?;
    register_read::<proxy_oracle_governance::GetNextId>(&mut m)?;
    register_read::<proxy_oracle_governance::GetTtl>(&mut m)?;
    register_read::<proxy_oracle_governance::GetCount>(&mut m)?;
    register_read::<proxy_oracle_governance::List>(&mut m)?;
    register_read::<proxy_oracle_governance::Get>(&mut m)?;
    register_write::<proxy_oracle_governance::Create>(&mut m)?;
    register_write::<proxy_oracle_governance::Cancel>(&mut m)?;
    register_write::<proxy_oracle_governance::Execute>(&mut m)?;
    register_read::<proxy_oracle_owner::GetOwner>(&mut m)?;
    register_read::<proxy_oracle_owner::GetProposedOwner>(&mut m)?;
    register_write::<proxy_oracle_owner::ProposeOwner>(&mut m)?;
    register_write::<proxy_oracle_owner::AcceptOwner>(&mut m)?;
    register_write::<proxy_oracle_owner::RenounceOwner>(&mut m)?;
    register_read::<registry::GetDeployment>(&mut m)?;
    register_read::<registry::ListDeployments>(&mut m)?;
    register_read::<registry::ListVersions>(&mut m)?;
    register_write::<registry::AddVersion>(&mut m)?;
    register_write::<registry::RemoveVersion>(&mut m)?;
    register_write::<registry::Deploy>(&mut m)?;
    register_read::<storage::GetBalanceBounds>(&mut m)?;
    register_read::<storage::GetBalanceOf>(&mut m)?;
    register_write::<storage::Deposit>(&mut m)?;
    register_write::<storage::EnsureDeposit>(&mut m)?;
    register_write::<storage::Unregister>(&mut m)?;
    register_read::<tx::Get>(&mut m)?;
    register_write::<tx::FunctionCall>(&mut m)?;
    register_read::<universal_account::GetKey>(&mut m)?;
    register_write::<universal_account::Execute>(&mut m)?;
    register_write::<universal_account::Create>(&mut m)?;

    Ok(m)
}

#[cfg(test)]
mod tests;
