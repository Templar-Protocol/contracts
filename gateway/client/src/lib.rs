//! Direct, in-process gateway client for Rust consumers (CLIs, bots, services).
//!
//! This is the lean sibling of [`templar_gateway_runtime`], which provides the
//! actix actor frontend used by the long-running RPC service. Where the runtime
//! wraps dispatch in an actor for bounded concurrency, this crate offers a plain
//! call-it-yourself facade over the same [`templar_gateway_core`] kernel.
//!
//! [`Client`] owns the read/plan context, signer set, and transaction executor.
//! Its [`Client::read`] / [`Client::execute_as`] helpers take the operation type
//! directly (the operation *is* its input), so call sites carry no turbofish, no
//! request wrappers, and no method-name repetition. A [`SigningClient`] binds a
//! default signing account for the common single-signer case:
//!
//! ```ignore
//! let client = SigningClient::connect(network, account_id, secret_key)?;
//!
//! let config = client.read(market::GetConfiguration { market_id }).await?;
//! let tx_hashes = client
//!     .execute(market::WithdrawStaticYield { market_id, amount: None })
//!     .await?;
//! ```

use std::{collections::HashMap, ops::Deref, sync::Arc};

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
}

impl ClientBuilder {
    /// Register a pre-built signer for an account.
    #[must_use]
    pub fn signer(mut self, account_id: impl Into<ManagedAccountId>, signer: Arc<Signer>) -> Self {
        self.signers.insert(account_id.into(), signer);
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

    /// Build the client, constructing the gateway context, signer, and executor.
    pub fn build(self) -> GatewayResult<Client> {
        Ok(Client {
            context: GatewayContext::new(self.network.clone())?,
            transaction_signer: NearTransactionSigner::new(self.network.clone(), self.signers),
            operation_executor: NearOperationExecutor::new(self.network),
        })
    }
}

/// A direct, in-process gateway client over the concrete [`Dispatch`].
///
/// Reads need no signer; writes name the signing account explicitly via
/// [`Client::execute_as`]. For the common single-signer case, bind a default
/// account with [`Client::into_signing`] (or [`SigningClient::connect`]).
#[derive(Clone)]
pub struct Client {
    context: GatewayContext,
    transaction_signer: NearTransactionSigner,
    operation_executor: NearOperationExecutor,
}

impl Client {
    /// Start building a client for `network`.
    #[must_use]
    pub fn builder(network: NetworkConfig) -> ClientBuilder {
        ClientBuilder {
            network,
            signers: HashMap::new(),
        }
    }

    /// Build a read-only client (no signing capability).
    pub fn read_only(network: NetworkConfig) -> GatewayResult<Self> {
        Self::builder(network).build()
    }

    /// Bind a default signing account, yielding a [`SigningClient`] whose
    /// `execute` needs no account argument. Errors if no signer is registered
    /// for `account_id`.
    pub fn into_signing(
        self,
        account_id: impl Into<ManagedAccountId>,
    ) -> GatewayResult<SigningClient> {
        let signer_account_id = account_id.into();
        if !self.transaction_signer.has_signer(&signer_account_id) {
            return Err(GatewayError::UnsupportedSignerAccount(
                signer_account_id.0.to_string(),
            ));
        }
        Ok(SigningClient {
            client: self,
            signer_account_id,
        })
    }

    /// Dispatch a read operation, inferring the output type from the operation.
    pub async fn read<Op>(&self, op: Op) -> GatewayResult<Op::Output>
    where
        Op: MethodSpec,
        Dispatch: DispatchRead<Op, GatewayContext>,
    {
        <Dispatch as DispatchRead<Op, GatewayContext>>::dispatch(op, self.context.clone()).await
    }

    /// Plan, sign, and submit a write operation signed by a specific account.
    pub async fn execute_as<Op>(
        &self,
        signer_account_id: impl Into<ManagedAccountId>,
        op: Op,
    ) -> GatewayResult<Vec<CryptoHash>>
    where
        Op: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<Op, GatewayContext>,
    {
        self.execute_request(WriteRequest {
            signer_account_id: signer_account_id.into(),
            idempotency_key: None,
            body: op,
        })
        .await
    }

    /// Plan and execute a fully-specified write request (escape hatch for
    /// explicit idempotency keys or signer accounts).
    pub async fn execute_request<S>(
        &self,
        request: WriteRequest<S>,
    ) -> GatewayResult<Vec<CryptoHash>>
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
    /// hash of each submitted transaction. The result is empty when the plan is a
    /// no-op (e.g. an idempotent write that needs no transactions). Fails if any
    /// step does not succeed on-chain.
    pub async fn execute_plan(&self, plan: OperationPlan) -> GatewayResult<Vec<CryptoHash>> {
        let mut hashes = Vec::with_capacity(plan.steps.len());
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
            hashes.push(tx_hash);
        }
        Ok(hashes)
    }
}

/// A [`Client`] bound to a default signing account.
///
/// Constructing one guarantees a signer is registered for the account, so
/// [`SigningClient::execute`] needs no account argument and cannot fail for a
/// missing-signer reason. Derefs to the underlying [`Client`] for reads and
/// explicit-signer writes.
#[derive(Clone)]
pub struct SigningClient {
    client: Client,
    signer_account_id: ManagedAccountId,
}

impl SigningClient {
    /// Connect a single-signer client for `account_id` from its secret key.
    pub fn connect(
        network: NetworkConfig,
        account_id: impl Into<ManagedAccountId>,
        secret_key: SecretKey,
    ) -> GatewayResult<Self> {
        let account_id = account_id.into();
        Client::builder(network)
            .secret_key(account_id.clone(), secret_key)?
            .build()?
            .into_signing(account_id)
    }

    /// The bound signing account.
    #[must_use]
    pub fn account_id(&self) -> &ManagedAccountId {
        &self.signer_account_id
    }

    /// Plan, sign, and submit a write operation signed by the bound account.
    pub async fn execute<Op>(&self, op: Op) -> GatewayResult<Vec<CryptoHash>>
    where
        Op: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<Op, GatewayContext>,
    {
        self.client
            .execute_as(self.signer_account_id.clone(), op)
            .await
    }
}

impl Deref for SigningClient {
    type Target = Client;

    fn deref(&self) -> &Client {
        &self.client
    }
}
