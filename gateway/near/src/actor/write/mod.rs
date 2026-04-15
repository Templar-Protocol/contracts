mod storage;
mod tx;

use std::{collections::HashMap, sync::Arc};

use actix::{Actor, Addr, ArbiterHandle, Context, Handler, ResponseFuture};
use blockchain_gateway_core::{
    operation::{
        OperationId, OperationOutcome, OperationRecord, OperationStatus, StepStatus,
        TransactionStepRecord,
    },
    rpc::common::WriteOperationResult,
    ManagedAccountId, MethodSpec,
};
use futures::future::BoxFuture;
use near_api::types::transaction::result::TransactionResult;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::{GatewayError, GatewayResult, ManagedSigner, NearClient};

use super::rpc::RpcMessage;

pub trait WriteRpcRequest: MethodSpec + Sized + Send + 'static {
    fn dispatch(
        params: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>>;

    fn signer_account_id(params: &Self::Input) -> &ManagedAccountId;
}

pub struct WriteActors {
    senders: HashMap<ManagedAccountId, Addr<AccountWriteActor>>,
}

impl WriteActors {
    pub(crate) fn spawn(
        arbiter: &ArbiterHandle,
        client: &NearClient,
        signers: HashMap<ManagedAccountId, ManagedSigner>,
    ) -> Self {
        let senders = signers
            .into_iter()
            .map(|(account_id, signer_entry)| {
                let actor = AccountWriteActor::spawn(
                    arbiter,
                    client.clone(),
                    signer_entry.signer,
                    signer_entry.key_count,
                );
                (account_id, actor)
            })
            .collect();

        Self { senders }
    }

    pub(crate) async fn request<Request>(
        &self,
        params: Request::Input,
    ) -> GatewayResult<Request::Output>
    where
        Request: WriteRpcRequest,
        AccountWriteActor: Handler<RpcMessage<Request>>,
    {
        let sender = self.sender_for(Request::signer_account_id(&params))?;
        sender
            .send(RpcMessage(params))
            .await
            .map_err(|error| crate::actor::map_mailbox_error(error, "write-actor"))?
    }

    fn sender_for(
        &self,
        signer_account_id: &ManagedAccountId,
    ) -> GatewayResult<&Addr<AccountWriteActor>> {
        self.senders.get(signer_account_id).ok_or_else(|| {
            crate::GatewayError::UnsupportedSignerAccount(signer_account_id.0.to_string())
        })
    }
}

pub struct AccountWriteActor {
    client: NearClient,
    signer: Arc<near_api::Signer>,
    semaphore: Arc<Semaphore>,
}

impl AccountWriteActor {
    fn new(client: NearClient, signer: Arc<near_api::Signer>, concurrency: usize) -> Self {
        Self {
            client,
            signer,
            semaphore: Arc::new(Semaphore::new(concurrency)),
        }
    }

    fn spawn(
        arbiter: &ArbiterHandle,
        client: NearClient,
        signer: Arc<near_api::Signer>,
        concurrency: usize,
    ) -> Addr<Self> {
        Self::start_in_arbiter(arbiter, move |_ctx| Self::new(client, signer, concurrency))
    }
}

pub(super) fn operation_outcome_from_transaction_result(
    signer_account_id: ManagedAccountId,
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

impl<Request> Handler<RpcMessage<Request>> for AccountWriteActor
where
    Request: WriteRpcRequest,
{
    type Result = ResponseFuture<GatewayResult<Request::Output>>;

    fn handle(
        &mut self,
        RpcMessage(message): RpcMessage<Request>,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let client = self.client.clone();
        let signer = self.signer.clone();
        let semaphore = self.semaphore.clone();

        Box::pin(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_error| GatewayError::ActorUnavailable("write-actor"))?;
            Request::dispatch(message, client, signer).await
        })
    }
}

impl Actor for AccountWriteActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(64);
    }
}
