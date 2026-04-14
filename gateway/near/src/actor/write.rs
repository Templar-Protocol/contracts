use tokio::sync::mpsc;

use blockchain_gateway_core::{
    operation::{
        OperationId, OperationOutcome, OperationRecord, OperationStatus, StepStatus,
        TransactionStepRecord,
    },
    rpc::common::{ContractArgs, WriteOperationResult, WriteRequest},
    storage, tx, ContractMethodName, NearGas,
};
use futures::future::BoxFuture;
use near_api::types::transaction::result::TransactionResult;
use uuid::Uuid;

use crate::{
    actor::request::{self, ActorRequest, MessageEnvelope},
    GatewayResult, NearWriteClient,
};

const WRITE_ACTOR_NAME: &str = "write-actor";

#[derive(Clone)]
pub struct WriteHandle {
    sender: mpsc::Sender<WriteMessage>,
}

impl WriteHandle {
    pub async fn request<Request>(&self, params: Request) -> GatewayResult<Request::Response>
    where
        Request: ActorRequest<Actor = NearWriteClient>,
        WriteMessage: From<MessageEnvelope<Request>>,
    {
        request::request(&self.sender, WRITE_ACTOR_NAME, params).await
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
        WriteMessage::FunctionCall(envelope) => {
            let _ = envelope.reply.send(envelope.params.dispatch(actor).await);
        }
        WriteMessage::StorageDeposit(envelope) => {
            let _ = envelope.reply.send(envelope.params.dispatch(actor).await);
        }
    }
}

pub fn spawn(client: NearWriteClient) -> WriteHandle {
    let (sender, mut receiver) = mpsc::channel(64);

    tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            dispatch(&client, message).await;
        }
    });

    WriteHandle { sender }
}
