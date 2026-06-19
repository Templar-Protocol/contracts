//! Direct, in-process gateway client for Rust consumers (CLIs, bots, services).
//!
//! This is the lean sibling of [`templar_gateway_runtime`], which provides the
//! actix actor frontend used by the long-running RPC service. Where the runtime
//! wraps dispatch in an actor for bounded concurrency, this crate offers a plain
//! call-it-yourself facade over the same [`templar_gateway_core`] kernel.
//!
//! [`Client`] owns the whole "boilerplate triangle" — the read/plan context, the
//! signer set, and the transaction executor — behind a single builder, and its
//! [`Client::read`] / [`Client::execute`] helpers take the operation type
//! directly (the operation *is* its input), so call sites carry no turbofish, no
//! request wrappers, and no method-name repetition:
//!
//! ```ignore
//! let client = Client::builder(network)
//!     .secret_key(account_id, secret_key)?
//!     .build()?;
//!
//! let config = client.read(market::GetConfiguration { market_id }).await?;
//! let tx_hash = client
//!     .execute(market::WithdrawStaticYield { market_id, amount: None })
//!     .await?;
//! ```

use std::{collections::HashMap, sync::Arc};

use near_api::{NetworkConfig, SecretKey, Signer};
use templar_gateway_core::{
    DispatchRead, ExecuteOperation, GatewayContext, GatewayError, GatewayResult,
    NearOperationExecutor, NearTransactionSigner, OperationPlan, PlanWrite, SignTransaction,
};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    CryptoHash, ManagedAccountId, MethodSpec,
};

/// Builder for [`Client`]. Takes the network once and accumulates signers,
/// constructing the context, signer, and executor on [`build`](ClientBuilder::build).
pub struct ClientBuilder {
    network: NetworkConfig,
    signers: HashMap<ManagedAccountId, Arc<Signer>>,
    default_signer: Option<ManagedAccountId>,
}

impl ClientBuilder {
    /// Register a pre-built signer for an account. The first signer registered
    /// becomes the default used by [`Client::execute`] unless overridden with
    /// [`default_signer`](Self::default_signer).
    #[must_use]
    pub fn signer(mut self, account_id: impl Into<ManagedAccountId>, signer: Arc<Signer>) -> Self {
        let account_id = account_id.into();
        self.default_signer
            .get_or_insert_with(|| account_id.clone());
        self.signers.insert(account_id, signer);
        self
    }

    /// Register a signer for an account from its secret key.
    pub fn secret_key(
        self,
        account_id: impl Into<ManagedAccountId>,
        secret_key: SecretKey,
    ) -> GatewayResult<Self> {
        let signer = Signer::from_secret_key(secret_key).map_err(|error| {
            GatewayError::NearTransaction(format!("invalid signer secret key: {error}"))
        })?;
        Ok(self.signer(account_id, signer))
    }

    /// Override which registered account signs [`Client::execute`] writes.
    #[must_use]
    pub fn default_signer(mut self, account_id: impl Into<ManagedAccountId>) -> Self {
        self.default_signer = Some(account_id.into());
        self
    }

    /// Build the client, constructing the gateway context, signer, and executor.
    pub fn build(self) -> GatewayResult<Client> {
        Ok(Client {
            context: GatewayContext::new(self.network.clone())?,
            transaction_signer: NearTransactionSigner::new(self.network.clone(), self.signers),
            operation_executor: NearOperationExecutor::new(self.network),
            default_signer: self.default_signer,
        })
    }
}

/// A direct, in-process gateway client over the concrete [`Dispatch`].
///
/// Bundles everything an operation needs — the read/plan context plus the signer
/// and executor used to submit planned transactions — and an optional default
/// signing account for ergonomic writes.
#[derive(Clone)]
pub struct Client {
    context: GatewayContext,
    transaction_signer: NearTransactionSigner,
    operation_executor: NearOperationExecutor,
    default_signer: Option<ManagedAccountId>,
}

impl Client {
    /// Start building a client for `network`.
    #[must_use]
    pub fn builder(network: NetworkConfig) -> ClientBuilder {
        ClientBuilder {
            network,
            signers: HashMap::new(),
            default_signer: None,
        }
    }

    /// Build a read-only client (no signing capability).
    pub fn read_only(network: NetworkConfig) -> GatewayResult<Self> {
        Self::builder(network).build()
    }

    /// Dispatch a read operation, inferring the output type from the operation.
    pub async fn read<Op>(&self, op: Op) -> GatewayResult<Op::Output>
    where
        Op: MethodSpec,
        Dispatch: DispatchRead<Op, GatewayContext>,
    {
        <Dispatch as DispatchRead<Op, GatewayContext>>::dispatch(op, self.context.clone()).await
    }

    /// Plan, sign, and submit a write operation signed by the default signer.
    /// Errors if no default signer is configured.
    pub async fn execute<Op>(&self, op: Op) -> GatewayResult<CryptoHash>
    where
        Op: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<Op, GatewayContext>,
    {
        let signer_account_id = self.default_signer.clone().ok_or_else(|| {
            GatewayError::UnsupportedSignerAccount("no default signer configured".to_owned())
        })?;
        self.execute_as::<Op>(signer_account_id, op).await
    }

    /// Plan, sign, and submit a write operation signed by a specific account.
    pub async fn execute_as<Op>(
        &self,
        signer_account_id: impl Into<ManagedAccountId>,
        op: Op,
    ) -> GatewayResult<CryptoHash>
    where
        Op: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<Op, GatewayContext>,
    {
        self.execute_request::<Op>(WriteRequest {
            signer_account_id: signer_account_id.into(),
            idempotency_key: None,
            body: op,
        })
        .await
    }

    /// Plan and execute a fully-specified write request (escape hatch for
    /// explicit idempotency keys or signer accounts).
    pub async fn execute_request<S>(&self, request: WriteRequest<S>) -> GatewayResult<CryptoHash>
    where
        S: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<S, GatewayContext>,
    {
        let plan =
            <Dispatch as PlanWrite<S, GatewayContext>>::plan(request, self.context.clone()).await?;
        self.execute_plan(plan).await
    }

    /// Plan a write request into the transactions required to fulfil it, without
    /// executing them.
    pub async fn plan_request<S>(&self, request: WriteRequest<S>) -> GatewayResult<OperationPlan>
    where
        S: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<S, GatewayContext>,
    {
        <Dispatch as PlanWrite<S, GatewayContext>>::plan(request, self.context.clone()).await
    }

    /// Sign and submit every step of an operation plan in order, returning the
    /// hash of the final step. Fails if any step does not succeed on-chain, or
    /// if the plan is empty.
    pub async fn execute_plan(&self, plan: OperationPlan) -> GatewayResult<CryptoHash> {
        let mut last_hash = None;
        for step in plan.steps {
            let prepared = self.transaction_signer.sign_transaction(step).await?;
            let tx_hash = prepared.tx_hash;
            let result = self
                .operation_executor
                .submit_transaction(prepared.signed_transaction, prepared.transaction.wait_until)
                .await?;
            if !result.is_success() {
                return Err(GatewayError::NearTransaction(format!(
                    "transaction {tx_hash} did not succeed"
                )));
            }
            last_hash = Some(tx_hash);
        }
        last_hash.ok_or_else(|| {
            GatewayError::NearTransaction("operation plan contained no steps".to_owned())
        })
    }
}
