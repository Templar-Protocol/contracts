use std::{collections::HashMap, sync::Arc};

use actix::{Actor, Addr, ArbiterHandle, Context, Handler, ResponseFuture};
use async_trait::async_trait;
use near_api::advanced::{ExecuteSignedTransaction, TransactionableOrSigned};
use near_api::types::transaction::{
    result::TransactionResult, PrepopulateTransaction, SignedTransaction,
};
use templar_gateway_core::GatewayContext;
use templar_gateway_core::{
    DispatchRead, GatewayError, GatewayResult, PlannedTransaction, PreparedTransactionResult,
};
use templar_gateway_types::{ManagedAccountId, MethodSpec};
use tokio::sync::Semaphore;

use crate::ManagedSigner;

const READ_ACTOR_NAME: &str = "read-actor";
const READ_ACTOR_MAX_CONCURRENCY: usize = 64;
const WRITE_ACTOR_NAME: &str = "write-actor";

pub struct RpcMessage<Spec: MethodSpec>(pub Spec::Input);

pub struct PreparedTransactionMessage {
    pub transaction: PlannedTransaction,
}

pub struct SubmitSignedTransactionMessage {
    pub signed_transaction: SignedTransaction,
    pub wait_until: templar_gateway_types::rpc::common::TxExecutionStatus,
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

pub fn map_mailbox_error(error: actix::MailboxError, actor_name: &'static str) -> GatewayError {
    GatewayError::ActorError {
        actor: actor_name,
        source: error,
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

    pub fn spawn(arbiter: &ArbiterHandle, context: GatewayContext) -> Addr<Self> {
        Self::start_in_arbiter(arbiter, move |_ctx| Self::new(context))
    }
}

impl<Spec> Handler<RpcMessage<Spec>> for ReadActor
where
    Spec: DispatchRead<GatewayContext>,
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
    pub fn spawn(
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
        self.senders
            .get(signer_account_id)
            .ok_or_else(|| GatewayError::UnsupportedSignerAccount(signer_account_id.0.to_string()))
    }

    pub async fn prepare_planned_transaction(
        &self,
        transaction: PlannedTransaction,
    ) -> GatewayResult<PreparedTransactionResult> {
        let sender = self.sender_for(&transaction.signer_account_id)?;
        sender
            .send(PreparedTransactionMessage { transaction })
            .await
            .map_err(|error| map_mailbox_error(error, WRITE_ACTOR_NAME))?
    }

    pub async fn submit_signed_transaction(
        &self,
        signer_account_id: &ManagedAccountId,
        signed_transaction: SignedTransaction,
        wait_until: templar_gateway_types::rpc::common::TxExecutionStatus,
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
