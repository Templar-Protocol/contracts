use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio::task::{JoinHandle, JoinSet};

use crate::{GatewayError, GatewayResult};
use futures::future::BoxFuture;

pub trait Actor: Clone + Send + Sync + 'static {
    type Message: Send + 'static;
    type Handle: Clone + Send + Sync + 'static;

    const NAME: &'static str;
    const CHANNEL_CAPACITY: usize = 64;

    fn concurrency(&self) -> usize {
        1
    }

    fn into_handle(sender: mpsc::Sender<Self::Message>) -> Self::Handle;

    fn on_message(&self, message: Self::Message) -> BoxFuture<'_, ()>;

    fn spawn(self) -> (Self::Handle, ActorTask) {
        let (sender, mut receiver) = mpsc::channel(Self::CHANNEL_CAPACITY);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let semaphore = Arc::new(Semaphore::new(self.concurrency()));

        let join = tokio::spawn(async move {
            let mut tasks = JoinSet::new();

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    maybe_message = receiver.recv() => {
                        let Some(message) = maybe_message else {
                            break;
                        };

                        let actor = self.clone();
                        let semaphore = semaphore.clone();
                        tasks.spawn(async move {
                            let _permit = semaphore
                                .acquire_owned()
                                .await
                                .expect("actor semaphore should remain available while actor is alive");
                            actor.on_message(message).await;
                        });
                    }
                }
            }

            while tasks.join_next().await.is_some() {}
        });

        (Self::into_handle(sender), ActorTask::new(shutdown_tx, join))
    }
}

pub trait ActorRequest {
    type Actor;
    type Response;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>>;
}

pub trait RequestHandle<Message>: Clone {
    const ACTOR_NAME: &'static str;

    fn sender(&self) -> &mpsc::Sender<Message>;

    async fn request_on<Request>(
        &self,
        sender: &mpsc::Sender<Message>,
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
            .map_err(|_| GatewayError::ActorUnavailable(Self::ACTOR_NAME))?;
        reply_rx
            .await
            .map_err(|_| GatewayError::ActorUnavailable(Self::ACTOR_NAME))?
    }

    async fn request<Request>(&self, request: Request) -> GatewayResult<Request::Response>
    where
        Request: ActorRequest,
        Message: From<MessageEnvelope<Request>>,
    {
        self.request_on(self.sender(), request).await
    }
}

pub struct MessageEnvelope<Request>
where
    Request: ActorRequest,
{
    pub params: Request,
    pub reply: oneshot::Sender<GatewayResult<Request::Response>>,
}

/// Lifecycle handle for one spawned actor task.
pub struct ActorTask {
    shutdown: Option<oneshot::Sender<()>>,
    join: JoinHandle<()>,
}

impl ActorTask {
    pub fn new(shutdown: oneshot::Sender<()>, join: JoinHandle<()>) -> Self {
        Self {
            shutdown: Some(shutdown),
            join,
        }
    }

    pub async fn shutdown_and_wait(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let _ = self.join.await;
    }
}

/// Collection of actor tasks that should be shut down together.
#[derive(Default)]
pub struct ActorGroup {
    tasks: Vec<ActorTask>,
}

impl ActorGroup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, task: ActorTask) {
        self.tasks.push(task);
    }

    pub fn extend(&mut self, tasks: impl IntoIterator<Item = ActorTask>) {
        self.tasks.extend(tasks);
    }

    pub fn extend_group(&mut self, mut other: ActorGroup) {
        self.tasks.append(&mut other.tasks);
    }

    pub async fn shutdown_and_wait(self) {
        for task in self.tasks {
            task.shutdown_and_wait().await;
        }
    }
}

pub async fn respond<Request>(actor: &Request::Actor, envelope: MessageEnvelope<Request>)
where
    Request: ActorRequest,
{
    let _ = envelope.reply.send(envelope.params.dispatch(actor).await);
}
