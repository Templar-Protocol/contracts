use tokio::sync::{mpsc, oneshot};

use crate::{GatewayError, GatewayResult};
use futures::future::BoxFuture;

pub trait ActorRequest {
    type Actor;
    type Response;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>>;
}

pub struct MessageEnvelope<Request>
where
    Request: ActorRequest,
{
    pub params: Request,
    pub reply: oneshot::Sender<GatewayResult<Request::Response>>,
}

pub async fn request<Request, Message>(
    sender: &mpsc::Sender<Message>,
    actor_name: &'static str,
    request: Request,
) -> GatewayResult<Request::Response>
where
    Request: ActorRequest,
    Message: From<MessageEnvelope<Request>>,
{
    let (reply_tx, reply_rx) = oneshot::channel();
    sender
        .send(Message::from(MessageEnvelope {
            params: request,
            reply: reply_tx,
        }))
        .await
        .map_err(|_| GatewayError::ActorUnavailable(actor_name))?;
    reply_rx
        .await
        .map_err(|_| GatewayError::ActorUnavailable(actor_name))?
}

pub async fn respond<Request>(actor: &Request::Actor, envelope: MessageEnvelope<Request>)
where
    Request: ActorRequest,
{
    let _ = envelope.reply.send(envelope.params.dispatch(actor).await);
}
