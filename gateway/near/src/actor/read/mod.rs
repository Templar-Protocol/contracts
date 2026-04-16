mod account;
mod contract;
mod ft;
mod market;
mod registry;
mod storage;
mod tx;
mod universal_account;

use std::sync::Arc;

use actix::{Actor, Addr, ArbiterHandle, Context, Handler, ResponseFuture};
use blockchain_gateway_core::MethodSpec;
use futures::future::BoxFuture;
use tokio::sync::Semaphore;

use crate::{GatewayError, GatewayResult, NearClient};

use super::RpcMessage;

const READ_ACTOR_NAME: &str = "read-actor";
const READ_ACTOR_MAX_CONCURRENCY: usize = 64;

pub trait DispatchRead: MethodSpec + Sized + Send + 'static {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>>;
}

#[derive(Clone)]
pub struct ReadActor {
    client: NearClient,
    semaphore: Arc<Semaphore>,
}

impl ReadActor {
    fn new(client: NearClient) -> Self {
        Self {
            client,
            semaphore: Arc::new(Semaphore::new(READ_ACTOR_MAX_CONCURRENCY)),
        }
    }

    pub(crate) fn spawn(arbiter: &ArbiterHandle, client: NearClient) -> Addr<Self> {
        Self::start_in_arbiter(arbiter, move |_ctx| Self::new(client))
    }
}

impl<Spec> Handler<RpcMessage<Spec>> for ReadActor
where
    Spec: DispatchRead,
{
    type Result = ResponseFuture<GatewayResult<Spec::Output>>;

    fn handle(&mut self, message: RpcMessage<Spec>, _ctx: &mut Self::Context) -> Self::Result {
        let client = self.client.clone();
        let semaphore = self.semaphore.clone();

        Box::pin(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_error| GatewayError::ActorUnavailable(READ_ACTOR_NAME))?;
            Spec::dispatch(message, client).await
        })
    }
}

impl Actor for ReadActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(64);
    }
}
