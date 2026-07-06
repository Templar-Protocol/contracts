use async_trait::async_trait;
use templar_gateway_artifacts_dispatch::Dispatch as ArtifactsDispatch;
use templar_gateway_artifacts_spec::artifact::{AddArtifactVersion, GetArtifact, ListArtifacts};
use templar_gateway_core::{DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite};
use templar_gateway_types::common::WriteRequest;

use crate::Dispatch;

#[async_trait]
impl<C> DispatchRead<GetArtifact, C> for Dispatch
where
    C: Send + 'static,
{
    async fn dispatch(
        request: GetArtifact,
        context: C,
    ) -> GatewayResult<<GetArtifact as templar_gateway_types::MethodSpec>::Output> {
        <ArtifactsDispatch as DispatchRead<GetArtifact, C>>::dispatch(request, context).await
    }
}

#[async_trait]
impl<C> DispatchRead<ListArtifacts, C> for Dispatch
where
    C: Send + 'static,
{
    async fn dispatch(
        request: ListArtifacts,
        context: C,
    ) -> GatewayResult<<ListArtifacts as templar_gateway_types::MethodSpec>::Output> {
        <ArtifactsDispatch as DispatchRead<ListArtifacts, C>>::dispatch(request, context).await
    }
}

#[async_trait]
impl<C> PlanWrite<AddArtifactVersion, C> for Dispatch
where
    C: HasNearClient,
{
    async fn plan(
        request: WriteRequest<AddArtifactVersion>,
        context: C,
    ) -> GatewayResult<OperationPlan> {
        <ArtifactsDispatch as PlanWrite<AddArtifactVersion, C>>::plan(request, context).await
    }
}
