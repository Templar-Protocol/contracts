use tokio::sync::mpsc;

use blockchain_gateway_core::{
    operation::{
        OperationId, OperationOutcome, OperationRecord, OperationStatus, StepStatus,
        TransactionStepRecord,
    },
    rpc::common::{ContractArgs, WriteOperationResult, WriteRequest},
    storage, tx, ContractMethodName, ManagedAccountId, NearGas,
};
use futures::future::BoxFuture;
use near_api::types::transaction::result::TransactionResult;
use uuid::Uuid;

use crate::{
    actor::request::{respond, Actor, ActorGroup, ActorRequest, MessageEnvelope, RequestHandle},
    GatewayResult, NearWriteClient,
};

const WRITE_ACTOR_NAME: &str = "write-actor";

pub struct WriteActors {
    client: NearWriteClient,
}

impl WriteActors {
    pub fn new(client: NearWriteClient) -> Self {
        Self { client }
    }

    pub fn spawn(self) -> (WriteHandle, ActorGroup) {
        let mut tasks = ActorGroup::new();
        let senders = self
            .client
            .signers()
            .iter()
            .map(|(account_id, signer_entry)| {
                let (sender, task) = AccountWriteActor::new(
                    self.client.clone(),
                    signer_entry.key_count,
                )
                .spawn();
                tasks.push(task);
                (account_id.clone(), sender)
            })
            .collect();

        (WriteHandle { senders }, tasks)
    }
}

#[derive(Clone)]
struct AccountWriteActor {
    client: NearWriteClient,
    concurrency: usize,
}

impl AccountWriteActor {
    fn new(client: NearWriteClient, concurrency: usize) -> Self {
        Self { client, concurrency }
    }
}

#[derive(Clone)]
pub struct WriteHandle {
    senders: std::collections::HashMap<ManagedAccountId, mpsc::Sender<WriteMessage>>,
}

impl WriteHandle {
    pub async fn request<T>(&self, params: WriteRequest<T>) -> GatewayResult<WriteOperationResult>
    where
        WriteRequest<T>: ActorRequest<Actor = NearWriteClient, Response = WriteOperationResult>,
        WriteMessage: From<MessageEnvelope<WriteRequest<T>>>,
    {
        let sender = self.sender_for(&params.signer_account_id)?;
        <Self as RequestHandle<WriteMessage>>::request_on(self, sender, params).await
    }

    fn sender_for(
        &self,
        signer_account_id: &ManagedAccountId,
    ) -> GatewayResult<&mpsc::Sender<WriteMessage>> {
        self.senders.get(signer_account_id).ok_or_else(|| {
            crate::GatewayError::UnsupportedSignerAccount(signer_account_id.0.to_string())
        })
    }
}

impl RequestHandle<WriteMessage> for WriteHandle {
    const ACTOR_NAME: &'static str = WRITE_ACTOR_NAME;

    fn sender(&self) -> &mpsc::Sender<WriteMessage> {
        unreachable!("write handle requires signer-based routing")
    }
}

pub enum WriteMessage {
    FunctionCall(MessageEnvelope<WriteRequest<tx::FunctionCallBody>>),
    StorageDeposit(MessageEnvelope<WriteRequest<storage::DepositBody>>),
}

fn operation_outcome_from_transaction_result(
    signer_account_id: blockchain_gateway_core::ManagedAccountId,
    tx_result: TransactionResult,
) -> WriteOperationResult {
    let (status, step_status, tx_hash) = if let Some(full) = tx_result.into_full() {
        let outcome = full.outcome();
        let tx_hash = Some(outcome.transaction_hash.to_string());
        let step_status = if full.is_success() {
            StepStatus::Succeeded
        } else {
            StepStatus::Failed
        };
        let status = if full.is_success() {
            OperationStatus::Succeeded
        } else {
            OperationStatus::Failed
        };
        (status, step_status, tx_hash)
    } else {
        (OperationStatus::InProgress, StepStatus::Submitted, None)
    };

    WriteOperationResult {
        outcome: OperationOutcome {
            operation: OperationRecord {
                id: OperationId(
                    tx_hash
                        .clone()
                        .unwrap_or_else(|| Uuid::new_v4().to_string()),
                ),
                signer_account_id,
                status,
                steps: vec![TransactionStepRecord {
                    index: 0,
                    status: step_status,
                    tx_hash,
                }],
            },
        },
    }
}

impl ActorRequest for WriteRequest<tx::FunctionCallBody> {
    type Actor = NearWriteClient;
    type Response = tx::FunctionCallResult;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>> {
        Box::pin(async move {
            let signer_account_id = self.signer_account_id.clone();
            let tx_result = actor
                .tx(self.signer_account_id.clone())?
                .function_call(self.body, self.wait_until)
                .await?;

            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }
}

impl From<MessageEnvelope<WriteRequest<tx::FunctionCallBody>>> for WriteMessage {
    fn from(envelope: MessageEnvelope<WriteRequest<tx::FunctionCallBody>>) -> Self {
        WriteMessage::FunctionCall(envelope)
    }
}

impl ActorRequest for WriteRequest<storage::DepositBody> {
    type Actor = NearWriteClient;
    type Response = storage::DepositResult;

    fn dispatch(self, actor: &Self::Actor) -> BoxFuture<'_, GatewayResult<Self::Response>> {
        Box::pin(async move {
            let signer_account_id = self.signer_account_id.clone();
            let body = self.body;
            let tx_result = actor
                .tx(self.signer_account_id)?
                .function_call(
                    tx::FunctionCallBody {
                        receiver_id: body.contract_id,
                        method_name: ContractMethodName("storage_deposit".to_owned()),
                        args: ContractArgs::Json(serde_json::json!({
                            "account_id": body.beneficiary_id,
                            "registration_only": body.registration_only,
                        })),
                        gas: NearGas::from_tgas(100),
                        deposit: body.deposit,
                    },
                    self.wait_until,
                )
                .await?;

            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }
}

impl From<MessageEnvelope<WriteRequest<storage::DepositBody>>> for WriteMessage {
    fn from(envelope: MessageEnvelope<WriteRequest<storage::DepositBody>>) -> Self {
        WriteMessage::StorageDeposit(envelope)
    }
}

async fn dispatch(actor: &NearWriteClient, message: WriteMessage) {
    match message {
        WriteMessage::FunctionCall(envelope) => respond(actor, envelope).await,
        WriteMessage::StorageDeposit(envelope) => respond(actor, envelope).await,
    }
}

impl Actor for AccountWriteActor {
    type Message = WriteMessage;
    type Handle = mpsc::Sender<WriteMessage>;

    const NAME: &'static str = WRITE_ACTOR_NAME;

    fn concurrency(&self) -> usize {
        self.concurrency
    }

    fn into_handle(sender: mpsc::Sender<Self::Message>) -> Self::Handle {
        sender
    }

    fn on_message(&self, message: Self::Message) -> BoxFuture<'_, ()> {
        Box::pin(async move { dispatch(&self.client, message).await })
    }
}
