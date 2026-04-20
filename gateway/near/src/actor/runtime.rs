use std::{collections::HashMap, sync::Arc};

use actix::{Actor, Addr, ArbiterHandle, Context, Handler, ResponseFuture};
use blockchain_gateway_core::common::WriteRequest;
use blockchain_gateway_core::{IdempotencyKey, ManagedAccountId, MethodSpec};
use futures::future::BoxFuture;
use near_api::types::transaction::result::TransactionResult;
use near_api::types::transaction::PrepopulateTransaction;
use tokio::sync::Semaphore;

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

pub trait HasIdempotencyKey {
    fn idempotency_key(&self) -> Option<&IdempotencyKey>;
}

impl<T> HasIdempotencyKey for WriteRequest<T> {
    fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        self.idempotency_key.as_ref()
    }
}

pub trait HasSignerAccountId {
    fn signer_account_id(&self) -> &ManagedAccountId;
}

impl<T> HasSignerAccountId for WriteRequest<T> {
    fn signer_account_id(&self) -> &ManagedAccountId {
        &self.signer_account_id
    }
}

pub trait HasWaitUntil {
    fn wait_until(&self) -> blockchain_gateway_core::rpc::common::TxExecutionStatus;
}

impl<T> HasWaitUntil for WriteRequest<T> {
    fn wait_until(&self) -> blockchain_gateway_core::rpc::common::TxExecutionStatus {
        self.wait_until
    }
}

pub trait PlanWrite:
    MethodSpec<
        Output = blockchain_gateway_core::rpc::common::WriteOperationResult,
        Input: HasIdempotencyKey + HasSignerAccountId + HasWaitUntil,
    > + Sized
    + Send
    + 'static
{
    fn plan(
        request: Self::Input,
        context: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<OperationPlan>>;
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

impl Actor for AccountWriteActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(64);
    }
}
