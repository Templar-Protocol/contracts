use futures::future::BoxFuture;
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

pub trait DispatchRead<Context>: MethodSpec + Sized + Send + 'static {
    fn dispatch(
        request: Self::Input,
        context: Context,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>>;
}

pub trait PlanWrite<Context>:
    MethodSpec<Output = WriteOperationResult, Input: HasIdempotencyKey + HasSignerAccountId>
    + Sized
    + Send
    + 'static
{
    fn plan(
        request: Self::Input,
        context: Context,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>>;
}
