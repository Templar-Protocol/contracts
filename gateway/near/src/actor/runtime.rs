use std::{collections::HashMap, sync::Arc};

use actix::{Actor, Addr, ArbiterHandle, Context, Handler, ResponseFuture};
use async_trait::async_trait;
use blockchain_gateway_core::common::WriteRequest;
use blockchain_gateway_core::rpc::common::WriteOperationResult;
use blockchain_gateway_core::{IdempotencyKey, ManagedAccountId, MethodSpec};
use futures::future::BoxFuture;
use near_api::advanced::{ExecuteSignedTransaction, TransactionableOrSigned};
use near_api::types::transaction::{
    result::TransactionResult, PrepopulateTransaction, SignedTransaction,
};
use tokio::sync::Semaphore;

use crate::operation::{OperationPlan, PlannedTransaction, PreparedTransactionResult};
use crate::{GatewayContext, GatewayError, GatewayResult};

use super::ManagedSigner;

const READ_ACTOR_NAME: &str = "read-actor";
const READ_ACTOR_MAX_CONCURRENCY: usize = 64;
const WRITE_ACTOR_NAME: &str = "write-actor";

pub struct RpcMessage<Spec: MethodSpec>(pub Spec::Input);

pub struct PreparedTransactionMessage {
    pub transaction: PlannedTransaction,
}

pub struct SubmitSignedTransactionMessage {
    pub signed_transaction: SignedTransaction,
    pub wait_until: blockchain_gateway_core::rpc::common::TxExecutionStatus,
}

#[derive(Debug, Clone)]
struct PrepopulatedTransactionCarrier(PrepopulateTransaction);

#[async_trait]
impl near_api::advanced::Transactionable for PrepopulatedTransactionCarrier {
    fn prepopulated(
        &self,
    ) -> Result<PrepopulateTransaction, near_api::errors::ArgumentValidationError> {
        Ok(self.0.clone())
    }

    async fn validate_with_network(
        &self,
        _network: &near_api::NetworkConfig,
    ) -> Result<(), near_api::errors::ValidationError> {
        Ok(())
    }
}

impl<Spec: MethodSpec> actix::Message for RpcMessage<Spec> {
    type Result = GatewayResult<Spec::Output>;
}

impl actix::Message for PreparedTransactionMessage {
    type Result = GatewayResult<PreparedTransactionResult>;
}

impl actix::Message for SubmitSignedTransactionMessage {
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

pub trait PlanWrite:
    MethodSpec<Output = WriteOperationResult, Input: HasIdempotencyKey + HasSignerAccountId>
    + Sized
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

    pub(crate) async fn prepare_planned_transaction(
        &self,
        transaction: PlannedTransaction,
    ) -> GatewayResult<PreparedTransactionResult> {
        let sender = self.sender_for(&transaction.signer_account_id)?;
        sender
            .send(PreparedTransactionMessage { transaction })
            .await
            .map_err(|error| map_mailbox_error(error, WRITE_ACTOR_NAME))?
    }

    pub(crate) async fn submit_signed_transaction(
        &self,
        signer_account_id: &ManagedAccountId,
        signed_transaction: SignedTransaction,
        wait_until: blockchain_gateway_core::rpc::common::TxExecutionStatus,
    ) -> GatewayResult<TransactionResult> {
        let sender = self.sender_for(signer_account_id)?;
        sender
            .send(SubmitSignedTransactionMessage {
                signed_transaction,
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

impl Handler<PreparedTransactionMessage> for AccountWriteActor {
    type Result = ResponseFuture<GatewayResult<PreparedTransactionResult>>;

    fn handle(
        &mut self,
        message: PreparedTransactionMessage,
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

            let presigned = near_api::Transaction::use_transaction(
                PrepopulateTransaction {
                    signer_id: message.transaction.signer_account_id.0.clone(),
                    receiver_id: message.transaction.receiver_id.clone(),
                    actions: message.transaction.actions.clone(),
                },
                signer,
            )
            .wait_until(message.transaction.wait_until.into())
            .presign_with(context.network())
            .await
            .map_err(|error| GatewayError::NearTransaction(error.to_string()))?;

            let Some(signed_transaction) = presigned.transaction.signed() else {
                return Err(GatewayError::NearTransaction(
                    "failed to extract presigned transaction".to_owned(),
                ));
            };
            let tx_hash = signed_transaction.get_hash().into();

            Ok(PreparedTransactionResult {
                transaction: message.transaction,
                tx_hash,
                signed_transaction,
            })
        })
    }
}

impl Handler<SubmitSignedTransactionMessage> for AccountWriteActor {
    type Result = ResponseFuture<GatewayResult<TransactionResult>>;

    fn handle(
        &mut self,
        message: SubmitSignedTransactionMessage,
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

            let prepopulated = PrepopulateTransaction {
                signer_id: message.signed_transaction.transaction.signer_id().clone(),
                receiver_id: message.signed_transaction.transaction.receiver_id().clone(),
                actions: message.signed_transaction.transaction.actions().to_vec(),
            };

            ExecuteSignedTransaction {
                transaction: TransactionableOrSigned::Signed((
                    message.signed_transaction,
                    Box::new(PrepopulatedTransactionCarrier(prepopulated)),
                )),
                signer,
                wait_until: message.wait_until.into(),
            }
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
