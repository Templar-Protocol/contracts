//! Direct, in-process gateway client for Rust consumers (CLIs, bots, services).
//!
//! This is the lean sibling of [`templar_gateway_runtime`], which provides the
//! actix actor frontend used by the long-running RPC service. Where the runtime
//! wraps dispatch in an actor for bounded concurrency, this crate offers a plain
//! call-it-yourself facade over the same [`templar_gateway_core`] kernel — and,
//! crucially, **reuses** the kernel's [`OperationDriver`] for writes rather than
//! re-implementing signing/submission, so direct-client writes get the same
//! idempotency, multi-step finalization, and replay semantics as the RPC service.
//!
//! [`Client`] owns the read context plus an [`OperationDriver`] (signer set,
//! executor, and an [`OperationStore`]). Its [`Client::read`] /
//! [`Client::execute_as`] helpers take the operation type directly (the
//! operation *is* its input), so call sites carry no turbofish, no request
//! wrappers, and no method-name repetition. A [`SigningClient`] binds a default
//! signing account for the common single-signer case:
//!
//! ```ignore
//! let client = SigningClient::connect(network, account_id, secret_key)?;
//!
//! let config = client.read(market::GetConfiguration { market_id }).await?;
//! let result = client
//!     .execute(market::WithdrawStaticYield { market_id, amount: None })
//!     .await?;
//! ```
//!
//! [`OperationStore`]: templar_gateway_core::OperationStore

use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
    sync::Arc,
};

use near_api::{NetworkConfig, SecretKey, Signer};
use templar_gateway_core::{
    DispatchRead, GatewayContext, GatewayError, GatewayResult, NearOperationExecutor,
    NearTransactionSigner, OperationDriver, OperationPlan, PlanWrite, SharedOperationStore,
};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_store::MemoryStore;
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    ManagedAccountId, MethodSpec,
};

/// Builder for [`Client`]. Takes the network once, accumulates signers, and
/// picks the [`OperationStore`](templar_gateway_core::OperationStore) backing
/// idempotency/replay (an in-process [`MemoryStore`] by default).
pub struct ClientBuilder {
    network: NetworkConfig,
    signers: HashMap<ManagedAccountId, Arc<Signer>>,
    store: SharedOperationStore,
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

    /// Use a specific operation store (e.g. a durable `PostgresStore`) for
    /// idempotency and replay. Defaults to an in-process [`MemoryStore`].
    #[must_use]
    pub fn store(mut self, store: SharedOperationStore) -> Self {
        self.store = store;
        self
    }

    /// Build the client, constructing the gateway context, signer, executor, and
    /// store-backed operation driver.
    pub fn build(self) -> GatewayResult<Client> {
        let context = GatewayContext::new(self.network.clone())?;
        let signer_account_ids = self.signers.keys().cloned().collect();
        let signer = NearTransactionSigner::new(self.network.clone(), self.signers);
        let executor = NearOperationExecutor::new(self.network);
        let driver = OperationDriver::new(self.store, Arc::new(signer), Arc::new(executor));
        Ok(Client {
            context,
            driver,
            signer_account_ids,
        })
    }
}

/// A direct, in-process gateway client over the concrete [`Dispatch`].
///
/// Reads need no signer; writes name the signing account explicitly via
/// [`Client::execute_as`] and run through the store-backed [`OperationDriver`].
/// For the common single-signer case, bind a default account with
/// [`Client::into_signing`] (or [`SigningClient::connect`]).
#[derive(Clone)]
pub struct Client {
    context: GatewayContext,
    driver: OperationDriver,
    signer_account_ids: HashSet<ManagedAccountId>,
}

impl Client {
    /// Start building a client for `network`.
    #[must_use]
    pub fn builder(network: NetworkConfig) -> ClientBuilder {
        ClientBuilder {
            network,
            signers: HashMap::new(),
            store: Arc::new(MemoryStore::new()),
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
        if !self.signer_account_ids.contains(&signer_account_id) {
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

    /// Plan and execute a write operation signed by a specific account, through
    /// the store-backed driver (idempotency + finalization + replay).
    pub async fn execute_as<Op>(
        &self,
        signer_account_id: impl Into<ManagedAccountId>,
        op: Op,
    ) -> GatewayResult<WriteOperationResult>
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

    /// Plan and execute a fully-specified write request (escape hatch for an
    /// explicit signer account or idempotency key).
    pub async fn execute_request<S>(
        &self,
        request: WriteRequest<S>,
    ) -> GatewayResult<WriteOperationResult>
    where
        S: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<S, GatewayContext>,
    {
        self.driver
            .plan_and_complete::<S, Dispatch, GatewayContext>(self.context.clone(), request)
            .await
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
    /// Connect a single-signer client for `account_id` from its secret key,
    /// backed by an in-process [`MemoryStore`].
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

    /// Plan and execute a write operation signed by the bound account.
    pub async fn execute<Op>(&self, op: Op) -> GatewayResult<WriteOperationResult>
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
