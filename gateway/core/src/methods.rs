use async_trait::async_trait;
use templar_gateway_types::common::WriteRequest;
use templar_gateway_types::rpc::common::WriteOperationResult;
use templar_gateway_types::{IdempotencyKey, ManagedAccountId, MethodSpec};

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
    async fn dispatch(request: Spec::Input, context: Context) -> GatewayResult<Spec::Output>;
}

#[async_trait]
pub trait PlanWrite<Spec, Context>: Send + 'static
where
    Spec: MethodSpec<Output = WriteOperationResult>,
    Spec::Input: HasIdempotencyKey + HasSignerAccountId,
{
    async fn plan(request: Spec::Input, context: Context) -> GatewayResult<OperationPlan>;
}
