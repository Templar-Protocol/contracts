use async_trait::async_trait;
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    IdempotencyKey, ManagedAccountId, MethodSpec,
};

use crate::{GatewayResult, OperationPlan};

pub trait HasIdempotencyKey {
    fn idempotency_key(&self) -> Option<&IdempotencyKey>;
}

impl<T> HasIdempotencyKey for WriteRequest<T> {
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        self.idempotency_key.as_ref()
    }
}

pub trait HasSignerAccountId {
    fn signer_account_id(&self) -> &ManagedAccountId;
}

impl<T> HasSignerAccountId for WriteRequest<T> {
    fn signer_account_id(&self) -> &ManagedAccountId {
        &self.signer_account_id
    }
}

#[async_trait]
pub trait DispatchRead<Spec, Context>: Send + 'static
where
    Spec: MethodSpec,
{
    async fn dispatch(request: Spec, context: Context) -> GatewayResult<Spec::Output>;
}

#[async_trait]
pub trait PlanWrite<Spec, Context>: Send + 'static
where
    Spec: MethodSpec<Output = WriteOperationResult>,
{
    async fn plan(request: WriteRequest<Spec>, context: Context) -> GatewayResult<OperationPlan>;
}
