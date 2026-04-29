use std::{marker::PhantomData, sync::Arc};

use actix::{Actor, Addr, ArbiterHandle, Context, Handler, ResponseFuture};
use templar_gateway_core::{DispatchRead, GatewayError, GatewayResult};
use templar_gateway_types::MethodSpec;
use tokio::sync::Semaphore;

const READ_ACTOR_NAME: &str = "read-actor";
const READ_ACTOR_MAX_CONCURRENCY: usize = 64;

pub struct RpcMessage<Spec: MethodSpec, Impl>(pub Spec::Input, pub PhantomData<Impl>);

impl<Spec: MethodSpec, Impl> actix::Message for RpcMessage<Spec, Impl> {
    type Result = GatewayResult<Spec::Output>;
}

pub fn map_mailbox_error(error: actix::MailboxError, actor_name: &'static str) -> GatewayError {
    GatewayError::ActorError {
        actor: actor_name,
        source: error,
    }
}

#[derive(Clone)]
pub struct ReadActor<ContextType> {
    context: ContextType,
    semaphore: Arc<Semaphore>,
}

impl<ContextType> ReadActor<ContextType>
where
    ContextType: Clone + Send + std::marker::Unpin + 'static,
{
    fn new(context: ContextType) -> Self {
        Self {
            context,
            semaphore: Arc::new(Semaphore::new(READ_ACTOR_MAX_CONCURRENCY)),
        }
    }

    pub fn spawn(arbiter: &ArbiterHandle, context: ContextType) -> Addr<Self> {
        Self::start_in_arbiter(arbiter, move |_ctx| Self::new(context))
    }
}

impl<Spec, Impl, ContextType> Handler<RpcMessage<Spec, Impl>> for ReadActor<ContextType>
where
    Spec: MethodSpec,
    Impl: DispatchRead<Spec, ContextType>,
    ContextType: Clone + Send + std::marker::Unpin + 'static,
{
    type Result = ResponseFuture<GatewayResult<Spec::Output>>;

    fn handle(
        &mut self,
        message: RpcMessage<Spec, Impl>,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let context = self.context.clone();
        let semaphore = self.semaphore.clone();

        Box::pin(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_error| GatewayError::ActorUnavailable(READ_ACTOR_NAME))?;
            Impl::dispatch(message.0, context).await
        })
    }
}

impl<ContextType> Actor for ReadActor<ContextType>
where
    ContextType: Clone + Send + std::marker::Unpin + 'static,
{
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(64);
    }
}
