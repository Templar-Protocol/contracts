use tokio::sync::mpsc;

use blockchain_gateway_core::{chain, market, registry, storage, universal_account};
use futures::future::BoxFuture;

use crate::{
    actor::request::{self, ActorRequest, MessageEnvelope},
    service::universal_account::into_parameters_view,
    GatewayResult, NearReadClient,
};

const READ_ACTOR_NAME: &str = "read-actor";

#[derive(Clone)]
pub struct ReadHandle {
    sender: mpsc::Sender<ReadMessage>,
}

impl ReadHandle {
    pub async fn request<Request>(&self, params: Request) -> GatewayResult<Request::Response>
    where
        Request: ActorRequest<Actor = NearReadClient>,
        ReadMessage: From<MessageEnvelope<Request>>,
    {
        request::request(&self.sender, READ_ACTOR_NAME, params).await
    }
}

pub enum ReadMessage {
    ViewAccount(MessageEnvelope<chain::ViewAccountParams>),
    ViewFunction(MessageEnvelope<chain::ViewFunctionParams>),
    GetTransaction(MessageEnvelope<chain::GetTransactionParams>),
    ListDeployments(MessageEnvelope<registry::ListDeploymentsParams>),
    ListVersions(MessageEnvelope<registry::ListVersionsParams>),
    GetConfiguration(MessageEnvelope<market::GetConfigurationParams>),
    ListBorrowPositions(MessageEnvelope<market::ListBorrowPositionsParams>),
    GetBalanceBounds(MessageEnvelope<storage::GetBalanceBoundsParams>),
    GetBalanceOf(MessageEnvelope<storage::GetBalanceOfParams>),
    GetKey(MessageEnvelope<universal_account::GetKeyParams>),
}

impl ActorRequest for chain::ViewAccountParams {
    type Actor = NearReadClient;
    type Response = chain::ViewAccountResult;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>> {
        Box::pin(async move { actor.chain().view_account(self).await })
    }
}

impl From<MessageEnvelope<chain::ViewAccountParams>> for ReadMessage {
    fn from(envelope: MessageEnvelope<chain::ViewAccountParams>) -> Self {
        ReadMessage::ViewAccount(envelope)
    }
}

impl ActorRequest for chain::ViewFunctionParams {
    type Actor = NearReadClient;
    type Response = chain::ViewFunctionResult;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>> {
        Box::pin(async move { actor.chain().view_function(self).await })
    }
}

impl From<MessageEnvelope<chain::ViewFunctionParams>> for ReadMessage {
    fn from(envelope: MessageEnvelope<chain::ViewFunctionParams>) -> Self {
        ReadMessage::ViewFunction(envelope)
    }
}

impl ActorRequest for chain::GetTransactionParams {
    type Actor = NearReadClient;
    type Response = chain::GetTransactionResult;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>> {
        Box::pin(async move { actor.chain().get_transaction(self).await })
    }
}

impl From<MessageEnvelope<chain::GetTransactionParams>> for ReadMessage {
    fn from(envelope: MessageEnvelope<chain::GetTransactionParams>) -> Self {
        ReadMessage::GetTransaction(envelope)
    }
}

impl ActorRequest for registry::ListDeploymentsParams {
    type Actor = NearReadClient;
    type Response = registry::ListDeploymentsResult;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>> {
        Box::pin(async move {
            actor
                .registry(self.registry_id)
                .list_deployments(self.args)
                .await
                .map(|account_ids| registry::ListDeploymentsResult { account_ids })
        })
    }
}

impl From<MessageEnvelope<registry::ListDeploymentsParams>> for ReadMessage {
    fn from(envelope: MessageEnvelope<registry::ListDeploymentsParams>) -> Self {
        ReadMessage::ListDeployments(envelope)
    }
}

impl ActorRequest for registry::ListVersionsParams {
    type Actor = NearReadClient;
    type Response = registry::ListVersionsResult;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>> {
        Box::pin(async move {
            actor
                .registry(self.registry_id)
                .list_versions(self.args)
                .await
                .map(|values| registry::ListVersionsResult { values })
        })
    }
}

impl From<MessageEnvelope<registry::ListVersionsParams>> for ReadMessage {
    fn from(envelope: MessageEnvelope<registry::ListVersionsParams>) -> Self {
        ReadMessage::ListVersions(envelope)
    }
}

impl ActorRequest for market::GetConfigurationParams {
    type Actor = NearReadClient;
    type Response = market::GetConfigurationResult;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>> {
        Box::pin(async move { actor.market(self.market_id).get_configuration(()).await })
    }
}

impl From<MessageEnvelope<market::GetConfigurationParams>> for ReadMessage {
    fn from(envelope: MessageEnvelope<market::GetConfigurationParams>) -> Self {
        ReadMessage::GetConfiguration(envelope)
    }
}

impl ActorRequest for market::ListBorrowPositionsParams {
    type Actor = NearReadClient;
    type Response = market::ListBorrowPositionsResult;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>> {
        Box::pin(async move {
            actor
                .market(self.market_id)
                .list_borrow_positions(self.args)
                .await
                .map(|positions| market::ListBorrowPositionsResult { positions })
        })
    }
}

impl From<MessageEnvelope<market::ListBorrowPositionsParams>> for ReadMessage {
    fn from(envelope: MessageEnvelope<market::ListBorrowPositionsParams>) -> Self {
        ReadMessage::ListBorrowPositions(envelope)
    }
}

impl ActorRequest for storage::GetBalanceBoundsParams {
    type Actor = NearReadClient;
    type Response = storage::GetBalanceBoundsResult;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>> {
        Box::pin(async move {
            actor
                .storage(self.contract_id)
                .storage_balance_bounds(self.args)
                .await
                .map(|bounds| storage::GetBalanceBoundsResult {
                    bounds: blockchain_gateway_core::common::StorageBalanceBounds {
                        min: bounds.min,
                        max: bounds.max,
                    },
                })
        })
    }
}

impl From<MessageEnvelope<storage::GetBalanceBoundsParams>> for ReadMessage {
    fn from(envelope: MessageEnvelope<storage::GetBalanceBoundsParams>) -> Self {
        ReadMessage::GetBalanceBounds(envelope)
    }
}

impl ActorRequest for storage::GetBalanceOfParams {
    type Actor = NearReadClient;
    type Response = storage::GetBalanceOfResult;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>> {
        Box::pin(async move {
            actor
                .storage(self.contract_id)
                .storage_balance_of(self.args)
                .await
                .map(|balance| storage::GetBalanceOfResult {
                    balance: balance.map(|balance| {
                        blockchain_gateway_core::common::StorageBalance {
                            total: balance.total,
                            available: balance.available,
                        }
                    }),
                })
        })
    }
}

impl From<MessageEnvelope<storage::GetBalanceOfParams>> for ReadMessage {
    fn from(envelope: MessageEnvelope<storage::GetBalanceOfParams>) -> Self {
        ReadMessage::GetBalanceOf(envelope)
    }
}

impl ActorRequest for universal_account::GetKeyParams {
    type Actor = NearReadClient;
    type Response = universal_account::GetKeyResult;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>> {
        Box::pin(async move {
            actor
                .universal_account(self.account_id)
                .get_key(self.args)
                .await
                .map(|parameters| universal_account::GetKeyResult {
                    parameters: parameters.map(into_parameters_view),
                })
        })
    }
}

impl From<MessageEnvelope<universal_account::GetKeyParams>> for ReadMessage {
    fn from(envelope: MessageEnvelope<universal_account::GetKeyParams>) -> Self {
        ReadMessage::GetKey(envelope)
    }
}

async fn dispatch(client: &NearReadClient, message: ReadMessage) {
    match message {
        ReadMessage::ViewAccount(envelope) => {
            let _ = envelope.reply.send(envelope.params.dispatch(client).await);
        }
        ReadMessage::ViewFunction(envelope) => {
            let _ = envelope.reply.send(envelope.params.dispatch(client).await);
        }
        ReadMessage::GetTransaction(envelope) => {
            let _ = envelope.reply.send(envelope.params.dispatch(client).await);
        }
        ReadMessage::ListDeployments(envelope) => {
            let _ = envelope.reply.send(envelope.params.dispatch(client).await);
        }
        ReadMessage::ListVersions(envelope) => {
            let _ = envelope.reply.send(envelope.params.dispatch(client).await);
        }
        ReadMessage::GetConfiguration(envelope) => {
            let _ = envelope.reply.send(envelope.params.dispatch(client).await);
        }
        ReadMessage::ListBorrowPositions(envelope) => {
            let _ = envelope.reply.send(envelope.params.dispatch(client).await);
        }
        ReadMessage::GetBalanceBounds(envelope) => {
            let _ = envelope.reply.send(envelope.params.dispatch(client).await);
        }
        ReadMessage::GetBalanceOf(envelope) => {
            let _ = envelope.reply.send(envelope.params.dispatch(client).await);
        }
        ReadMessage::GetKey(envelope) => {
            let _ = envelope.reply.send(envelope.params.dispatch(client).await);
        }
    }
}

pub fn spawn(client: NearReadClient) -> ReadHandle {
    let (sender, mut receiver) = mpsc::channel(64);

    tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            dispatch(&client, message).await;
        }
    });

    ReadHandle { sender }
}
