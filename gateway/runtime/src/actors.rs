use std::sync::Arc;

use actix::{Actor, Addr, ArbiterHandle, Context, Handler, ResponseFuture};
use templar_gateway_core::{DispatchRead, GatewayContext, GatewayError, GatewayResult};
use templar_gateway_types::MethodSpec;
use tokio::sync::Semaphore;

const READ_ACTOR_NAME: &str = "read-actor";
const READ_ACTOR_MAX_CONCURRENCY: usize = 64;

pub struct RpcMessage<Spec: MethodSpec>(pub Spec::Input);

impl<Spec: MethodSpec> actix::Message for RpcMessage<Spec> {
    type Result = GatewayResult<Spec::Output>;
}

pub fn map_mailbox_error(error: actix::MailboxError, actor_name: &'static str) -> GatewayError {
    GatewayError::ActorError {
        actor: actor_name,
        source: error,
    }
}

#[derive(Clone)]
pub struct ReadActor {
    context: GatewayContext,
    semaphore: Arc<Semaphore>,
}

impl ReadActor {
    fn new(context: GatewayContext) -> Self {
        Self {
            context,
            semaphore: Arc::new(Semaphore::new(READ_ACTOR_MAX_CONCURRENCY)),
        }
    }

    pub fn spawn(arbiter: &ArbiterHandle, context: GatewayContext) -> Addr<Self> {
        Self::start_in_arbiter(arbiter, move |_ctx| Self::new(context))
    }
}

impl<Spec> Handler<RpcMessage<Spec>> for ReadActor
where
    Spec: DispatchRead<GatewayContext>,
{
    type Result = ResponseFuture<GatewayResult<Spec::Output>>;

    fn handle(&mut self, message: RpcMessage<Spec>, _ctx: &mut Self::Context) -> Self::Result {
        let context = self.context.clone();
        let semaphore = self.semaphore.clone();

        Box::pin(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_error| GatewayError::ActorUnavailable(READ_ACTOR_NAME))?;
            Spec::dispatch(message.0, context).await
        })
    }
}

impl Actor for ReadActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(64);
    }
}
