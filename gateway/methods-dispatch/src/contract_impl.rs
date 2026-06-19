use async_trait::async_trait;
use templar_gateway_core::{
    query_contract_kind, DispatchRead, GatewayError, GatewayResult, HasNearClient,
};
use templar_gateway_methods_spec::contract;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<contract::ViewFunction, C> for Dispatch {
    async fn dispatch(
        request: contract::ViewFunction,
        ctx: C,
    ) -> GatewayResult<contract::ViewFunctionResult> {
        let value = ctx
            .near_client()
            .contract(request.contract_id.clone())
            .view_function(&request.method_name.0, request.args.try_into_bytes()?)
            .await?;

        Ok(contract::ViewFunctionResult { value })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<contract::GetVersion, C> for Dispatch {
    async fn dispatch(
        request: contract::GetVersion,
        ctx: C,
    ) -> GatewayResult<contract::VersionResult> {
        let metadata = ctx
            .near_client()
            .contract(request.contract_id)
            .cached_contract_source_metadata()
            .await?;
        let version_string = metadata.version.ok_or_else(|| {
            GatewayError::NearQuery("contract metadata does not contain version".to_owned())
        })?;

        Ok(contract::VersionResult {
            parsed: version_string.parse().ok(),
            version_string,
        })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<contract::GetKind, C> for Dispatch {
    async fn dispatch(
        request: contract::GetKind,
        ctx: C,
    ) -> GatewayResult<contract::GetKindResult> {
        let kind = query_contract_kind(&ctx, request.contract_id).await?;
        Ok(contract::GetKindResult { kind })
    }
}
