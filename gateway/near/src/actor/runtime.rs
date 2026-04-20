use std::{collections::HashMap, sync::Arc};

use actix::{Actor, Addr, ArbiterHandle, Context, Handler, ResponseFuture};
use blockchain_gateway_core::{
    operation::{OperationId, OperationRecord, OperationStatus, StepStatus, TransactionStepRecord},
    rpc::common::WriteOperationResult,
    IdempotencyKey, ManagedAccountId, MethodSpec,
};
use futures::future::BoxFuture;
use near_api::types::transaction::result::TransactionResult;
use near_api::types::transaction::PrepopulateTransaction;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::operation::{OperationPlan, PlannedTransaction};
use crate::{GatewayContext, GatewayError, GatewayResult};

use super::ManagedSigner;

const READ_ACTOR_NAME: &str = "read-actor";
const READ_ACTOR_MAX_CONCURRENCY: usize = 64;
const WRITE_ACTOR_NAME: &str = "write-actor";

pub struct RpcMessage<Spec: MethodSpec>(pub Spec::Input);

pub struct PlannedTransactionMessage {
    pub transaction: PlannedTransaction,
    pub wait_until: blockchain_gateway_core::rpc::common::TxExecutionStatus,
}

impl<Spec: MethodSpec> actix::Message for RpcMessage<Spec> {
    type Result = GatewayResult<Spec::Output>;
}

impl actix::Message for PlannedTransactionMessage {
    type Result = GatewayResult<TransactionResult>;
}

pub(crate) fn map_mailbox_error(
    error: actix::MailboxError,
    actor_name: &'static str,
) -> crate::GatewayError {
    crate::GatewayError::ActorError {
        actor: actor_name,
        source: error,
    }
}

pub trait DispatchRead: MethodSpec + Sized + Send + 'static {
    fn dispatch(
        request: Self::Input,
        context: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>>;
}

pub trait DispatchWrite:
    MethodSpec<Output = blockchain_gateway_core::rpc::common::WriteOperationResult>
    + Sized
    + Send
    + 'static
{
    #[allow(unused_variables)]
    fn dispatch(
        request: Self::Input,
        context: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async {
            Err(GatewayError::NearTransaction(format!(
                "legacy write dispatch path is disabled for {}",
                Self::RPC_METHOD
            )))
        })
    }

    fn signer_account_id(params: &Self::Input) -> &ManagedAccountId;

    fn idempotency_key(_params: &Self::Input) -> Option<&IdempotencyKey> {
        None
    }

    fn wait_until(
        _params: &Self::Input,
    ) -> blockchain_gateway_core::rpc::common::TxExecutionStatus {
        blockchain_gateway_core::rpc::common::TxExecutionStatus::Final
    }

    fn uses_operation_planning() -> bool {
        false
    }

    fn plan(
        _request: Self::Input,
        _context: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async {
            Err(GatewayError::NearTransaction(
                "operation planning is not implemented for this method".to_owned(),
            ))
        })
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

    pub(crate) fn spawn(arbiter: &ArbiterHandle, context: GatewayContext) -> Addr<Self> {
        Self::start_in_arbiter(arbiter, move |_ctx| Self::new(context))
    }
}

impl<Spec> Handler<RpcMessage<Spec>> for ReadActor
where
    Spec: DispatchRead,
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

pub struct WriteActors {
    senders: HashMap<ManagedAccountId, Addr<AccountWriteActor>>,
}

impl WriteActors {
    pub(crate) fn spawn(
        arbiter: &ArbiterHandle,
        context: &GatewayContext,
        signers: HashMap<ManagedAccountId, ManagedSigner>,
    ) -> Self {
        let senders = signers
            .into_iter()
            .map(|(account_id, signer_entry)| {
                let actor = AccountWriteActor::spawn(
                    arbiter,
                    context.clone(),
                    signer_entry.signer,
                    signer_entry.key_count,
                );
                (account_id, actor)
            })
            .collect();

        Self { senders }
    }

    fn sender_for(
        &self,
        signer_account_id: &ManagedAccountId,
    ) -> GatewayResult<&Addr<AccountWriteActor>> {
        self.senders.get(signer_account_id).ok_or_else(|| {
            crate::GatewayError::UnsupportedSignerAccount(signer_account_id.0.to_string())
        })
    }

    pub(crate) async fn request<Request>(
        &self,
        params: Request::Input,
    ) -> GatewayResult<Request::Output>
    where
        Request: DispatchWrite,
        AccountWriteActor: Handler<RpcMessage<Request>>,
    {
        let sender = self.sender_for(Request::signer_account_id(&params))?;
        sender
            .send(RpcMessage(params))
            .await
            .map_err(|error| map_mailbox_error(error, WRITE_ACTOR_NAME))?
    }

    pub(crate) async fn execute_planned_transaction(
        &self,
        transaction: PlannedTransaction,
        wait_until: blockchain_gateway_core::rpc::common::TxExecutionStatus,
    ) -> GatewayResult<TransactionResult> {
        let sender = self.sender_for(&transaction.signer_account_id)?;
        sender
            .send(PlannedTransactionMessage {
                transaction,
                wait_until,
            })
            .await
            .map_err(|error| map_mailbox_error(error, WRITE_ACTOR_NAME))?
    }
}

pub(crate) struct AccountWriteActor {
    context: GatewayContext,
    signer: Arc<near_api::Signer>,
    semaphore: Arc<Semaphore>,
}

impl AccountWriteActor {
    fn new(context: GatewayContext, signer: Arc<near_api::Signer>, concurrency: usize) -> Self {
        Self {
            context,
            signer,
            semaphore: Arc::new(Semaphore::new(concurrency)),
        }
    }

    fn spawn(
        arbiter: &ArbiterHandle,
        context: GatewayContext,
        signer: Arc<near_api::Signer>,
        concurrency: usize,
    ) -> Addr<Self> {
        Self::start_in_arbiter(arbiter, move |_ctx| Self::new(context, signer, concurrency))
    }
}

pub(crate) fn operation_outcome_from_transaction_result(
    signer_account_id: ManagedAccountId,
    tx_result: TransactionResult,
) -> WriteOperationResult {
    operation_outcome_from_transaction_results(signer_account_id, vec![tx_result])
}

pub(crate) fn operation_outcome_from_transaction_results(
    signer_account_id: ManagedAccountId,
    tx_results: Vec<TransactionResult>,
) -> WriteOperationResult {
    let mut status = OperationStatus::Succeeded;
    let mut operation_id = None;
    let mut steps = Vec::with_capacity(tx_results.len());

    for (index, tx_result) in tx_results.into_iter().enumerate() {
        let step_status = if let Some(full) = tx_result.into_full() {
            let outcome = full.outcome();
            let tx_hash: blockchain_gateway_core::CryptoHash = outcome.transaction_hash.into();
            if operation_id.is_none() {
                operation_id = Some(tx_hash.0.to_string());
            }
            if full.is_success() {
                StepStatus::Succeeded { tx_hash }
            } else {
                status = OperationStatus::Failed;
                StepStatus::Failed {
                    tx_hash: Some(tx_hash),
                }
            }
        } else {
            if status != OperationStatus::Failed {
                status = OperationStatus::InProgress;
            }
            StepStatus::Submitted { tx_hash: None }
        };

        steps.push(TransactionStepRecord {
            index: index as u32,
            status: step_status,
        });
    }

    WriteOperationResult {
        operation: OperationRecord {
            id: OperationId(operation_id.unwrap_or_else(|| Uuid::new_v4().to_string())),
            signer_account_id,
            status,
            steps,
        },
    }
}

impl Handler<PlannedTransactionMessage> for AccountWriteActor {
    type Result = ResponseFuture<GatewayResult<TransactionResult>>;

    fn handle(
        &mut self,
        message: PlannedTransactionMessage,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let context = self.context.clone();
        let signer = self.signer.clone();
        let semaphore = self.semaphore.clone();

        Box::pin(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_error| GatewayError::ActorUnavailable(WRITE_ACTOR_NAME))?;

            near_api::Transaction::use_transaction(
                PrepopulateTransaction {
                    signer_id: message.transaction.signer_account_id.0,
                    receiver_id: message.transaction.receiver_id,
                    actions: message.transaction.actions,
                },
                signer,
            )
            .wait_until(message.wait_until.into())
            .send_to(context.network())
            .await
            .map_err(|error| GatewayError::NearTransaction(error.to_string()))
        })
    }
}

impl<Request> Handler<RpcMessage<Request>> for AccountWriteActor
where
    Request: DispatchWrite,
{
    type Result = ResponseFuture<GatewayResult<Request::Output>>;

    fn handle(
        &mut self,
        RpcMessage(message): RpcMessage<Request>,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let context = self.context.clone();
        let signer = self.signer.clone();
        let semaphore = self.semaphore.clone();

        Box::pin(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_error| GatewayError::ActorUnavailable(WRITE_ACTOR_NAME))?;
            Request::dispatch(message, context, signer).await
        })
    }
}

impl Actor for AccountWriteActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(64);
    }
}
