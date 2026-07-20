//! Direct, in-process gateway client for Rust consumers.
//!
//! Writes use the same [`OperationDriver`] as the RPC service, so direct clients
//! get the same idempotency, finalization, and replay behavior.
//!
//! [`Client`] owns the read context plus an [`OperationDriver`] (signer set,
//! executor, and an [`OperationStore`]). Its [`Client::read`] /
//! [`Client::execute_as`] helpers take the operation type directly (the
//! operation *is* its input), so call sites carry no turbofish, no request
//! wrappers, and no method-name repetition. A [`SigningClient`] binds a default
//! signing account:
//!
//! ```ignore
//! let client = SigningClient::connect(network, account_id, secret_key)?;
//!
//! let config = client.read(market::GetConfiguration::new(market_id)).await?;
//! let result = client
//!     .execute(market::WithdrawStaticYield::new(market_id, None))
//!     .await?;
//! ```
//!
//! The request structs keep their public fields, so explicit struct literals
//! remain available when they are clearer.
//!
//! [`OperationStore`]: templar_gateway_core::OperationStore

mod network;
pub use network::{Network, NetworkConfigBuilder};

mod pagination;
pub use pagination::collect_paginated;

use std::{
    collections::{HashMap, HashSet},
    marker::PhantomData,
    ops::Deref,
    sync::Arc,
};

use near_api::{NetworkConfig, SecretKey, Signer};
use templar_gateway_core::{
    DispatchRead, FinalityPolicy, GatewayContext, GatewayError, GatewayResult, NearClient,
    NearOperationExecutor, NearTransactionSigner, OperationDriver, OperationPlan, PlanWrite,
    SharedOperationStore,
};
use templar_gateway_store::MemoryStore;

/// The default dispatcher, covering the full in-process method surface. Rebind
/// with [`Client::via`] to reach a heavier, opt-in dispatcher (contract
/// artifacts, oracle updates) whose crate the consumer depends on explicitly.
pub use templar_gateway_methods_dispatch::Dispatch as MethodsDispatch;
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    IdempotencyKey, ManagedAccountId, MethodSpec, OperationId, OperationRecord,
};

/// Builder for [`Client`]. Takes the network once, accumulates signers, and
/// picks the [`OperationStore`](templar_gateway_core::OperationStore) backing
/// idempotency/replay (an in-process [`MemoryStore`] by default).
pub struct ClientBuilder {
    network: NetworkConfig,
    signers: HashMap<ManagedAccountId, Arc<Signer>>,
    store: SharedOperationStore,
    finality_policy: FinalityPolicy,
}

impl ClientBuilder {
    /// Register a pre-built signer for an account.
    #[must_use]
    pub fn with_signer(
        mut self,
        account_id: impl Into<ManagedAccountId>,
        signer: Arc<Signer>,
    ) -> Self {
        self.signers.insert(account_id.into(), signer);
        self
    }

    /// Register a signer for an account from its secret key.
    pub fn secret_key(
        self,
        account_id: impl Into<ManagedAccountId>,
        secret_key: SecretKey,
    ) -> GatewayResult<Self> {
        let signer = Signer::from_secret_key(secret_key)
            .map_err(|error| GatewayError::InvalidSignerKey(error.to_string()))?;
        Ok(self.with_signer(account_id, signer))
    }

    /// Register a rotating signer from one or more secret keys.
    ///
    /// Each key keeps its own nonce sequence. Errors if no keys are provided.
    pub async fn secret_keys(
        self,
        account_id: impl Into<ManagedAccountId>,
        secret_keys: impl IntoIterator<Item = SecretKey>,
    ) -> GatewayResult<Self> {
        let mut keys = secret_keys.into_iter();
        let first = keys.next().ok_or_else(|| {
            GatewayError::InvalidSignerKey("at least one secret key is required".to_owned())
        })?;
        let signer = Signer::from_secret_key(first)
            .map_err(|error| GatewayError::InvalidSignerKey(error.to_string()))?;
        for key in keys {
            signer
                .add_secret_key_to_pool(key)
                .await
                .map_err(|error| GatewayError::InvalidSignerKey(error.to_string()))?;
        }
        Ok(self.with_signer(account_id, signer))
    }

    /// Use a specific operation store (e.g. a durable `PostgresStore`) for
    /// idempotency and replay. Defaults to an in-process [`MemoryStore`].
    #[must_use]
    pub fn store(mut self, store: SharedOperationStore) -> Self {
        self.store = store;
        self
    }

    /// Set the transaction-wait and state-query policy. Defaults to final
    /// execution and finalized reads.
    #[must_use]
    pub fn finality_policy(mut self, finality_policy: FinalityPolicy) -> Self {
        self.finality_policy = finality_policy;
        self
    }

    /// Build the base gateway context, signer set, executor, and store-backed
    /// operation driver. Returns the raw parts so a consumer can layer a custom
    /// context (e.g. adding in-process oracle payload sources) and hand them to
    /// [`Client::from_parts`] — every client derived from one set of parts shares
    /// the same driver (store, signer pool, idempotency, replay, recovery).
    pub fn build_parts(
        self,
    ) -> GatewayResult<(GatewayContext, OperationDriver, HashSet<ManagedAccountId>)> {
        let context = GatewayContext::from_near_client(NearClient::with_finality_policy(
            self.network.clone(),
            self.finality_policy,
        ));
        let signer_account_ids = self.signers.keys().cloned().collect();
        let signer = NearTransactionSigner::new(self.network.clone(), self.signers);
        let executor =
            NearOperationExecutor::with_finality_policy(self.network, self.finality_policy);
        let driver = OperationDriver::new(self.store, Arc::new(signer), Arc::new(executor));
        Ok((context, driver, signer_account_ids))
    }

    /// Build the client, constructing the gateway context, signer, executor, and
    /// store-backed operation driver.
    pub fn build(self) -> GatewayResult<Client> {
        let (context, driver, signer_account_ids) = self.build_parts()?;
        Ok(Client::from_parts(context, driver, signer_account_ids))
    }
}

/// A direct, in-process gateway client, dispatching through `D` (defaults to
/// [`MethodsDispatch`]). Rebind to an opt-in dispatcher with [`Client::via`].
///
/// Reads need no signer; writes name the signing account explicitly via
/// [`Client::execute_as`] and run through the store-backed [`OperationDriver`].
/// For the common single-signer case, bind a default account with
/// [`Client::into_signing`] (or [`SigningClient::connect`]).
pub struct Client<D = MethodsDispatch, Ctx = GatewayContext> {
    context: Ctx,
    driver: OperationDriver,
    signer_account_ids: HashSet<ManagedAccountId>,
    // `fn() -> D` carries the dispatcher choice without owning it, so `Client`
    // stays `Send`/`Sync` for any `D` (which is only ever a ZST marker).
    _dispatch: PhantomData<fn() -> D>,
}

impl<D, Ctx: Clone> Clone for Client<D, Ctx> {
    fn clone(&self) -> Self {
        Self {
            context: self.context.clone(),
            driver: self.driver.clone(),
            signer_account_ids: self.signer_account_ids.clone(),
            _dispatch: PhantomData,
        }
    }
}

impl<D, Ctx> std::fmt::Debug for Client<D, Ctx> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("signer_account_ids", &self.signer_account_ids)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Start building a client for `network`.
    #[must_use]
    pub fn builder(network: NetworkConfig) -> ClientBuilder {
        ClientBuilder {
            network,
            signers: HashMap::new(),
            store: Arc::new(MemoryStore::new()),
            finality_policy: FinalityPolicy::default(),
        }
    }

    /// Build a read-only client (no signing capability).
    pub fn read_only(network: NetworkConfig) -> GatewayResult<Self> {
        Self::builder(network).build()
    }
}

impl<D, Ctx> Client<D, Ctx> {
    /// Construct a client from pre-built parts (see [`ClientBuilder::build_parts`]).
    /// Lets a consumer supply a custom `Ctx` — e.g. one carrying in-process oracle
    /// payload sources — while sharing the driver with sibling clients.
    #[must_use]
    pub fn from_parts(
        context: Ctx,
        driver: OperationDriver,
        signer_account_ids: HashSet<ManagedAccountId>,
    ) -> Self {
        Self {
            context,
            driver,
            signer_account_ids,
            _dispatch: PhantomData,
        }
    }

    /// Bind a default signing account, yielding a [`SigningClient`] whose
    /// `execute` needs no account argument. Errors if no signer is registered
    /// for `account_id`.
    pub fn into_signing(
        self,
        account_id: impl Into<ManagedAccountId>,
    ) -> GatewayResult<SigningClient<D, Ctx>> {
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

    /// Look up a stored operation by idempotency key.
    ///
    /// Used by callers that need to recover after submitting work but before
    /// recording the result locally.
    pub async fn operation_by_idempotency_key(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> GatewayResult<Option<OperationRecord>> {
        Ok(self
            .driver
            .get_by_idempotency_key(idempotency_key)
            .await?
            .map(|operation| operation.record()))
    }

    /// Look up a stored gateway operation by operation ID.
    pub async fn operation(
        &self,
        operation_id: &OperationId,
    ) -> GatewayResult<Option<OperationRecord>> {
        self.driver.get_operation(operation_id).await
    }

    /// Drive every operation left mid-flight (e.g. by a crash) to a terminal
    /// outcome — submitting prepared steps and reconciling submitted ones against
    /// the chain. Lets a consumer rely on the gateway to finish its own work
    /// before reading back terminal results (see the relayer's broom).
    pub async fn resume_incomplete_operations(&self) -> GatewayResult<()> {
        self.driver.resume_incomplete_operations().await
    }

    /// Reconcile a single in-flight operation by idempotency key against the
    /// chain and return its current record. The broom calls this to drive a stuck
    /// operation toward terminal between startups; an unknown submitted
    /// transaction remains submitted because the chain's absence of a record is
    /// not proof that the transaction never landed.
    pub async fn reconcile_operation(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> GatewayResult<Option<OperationRecord>> {
        self.driver.reconcile_operation(idempotency_key).await
    }
}

impl<D, Ctx: Clone> Client<D, Ctx> {
    /// Rebind this client to dispatcher `D2`, keeping the same connection,
    /// context, signer set, and operation store. Cheap: clones the context, two
    /// `Arc`s, and the signer-id set. Use it to reach an opt-in dispatcher, e.g.
    /// `client.via::<ArtifactsDispatch>().read(GetArtifact { .. })`.
    #[must_use]
    pub fn via<D2>(&self) -> Client<D2, Ctx> {
        Client {
            context: self.context.clone(),
            driver: self.driver.clone(),
            signer_account_ids: self.signer_account_ids.clone(),
            _dispatch: PhantomData,
        }
    }

    /// Dispatch a read operation, inferring the output type from the operation.
    pub async fn read<Op>(&self, op: Op) -> GatewayResult<Op::Output>
    where
        Op: MethodSpec,
        D: DispatchRead<Op, Ctx>,
    {
        <D as DispatchRead<Op, Ctx>>::dispatch(op, self.context.clone()).await
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
        D: PlanWrite<Op, Ctx>,
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
        D: PlanWrite<S, Ctx>,
    {
        self.driver
            .plan_and_complete::<S, D, Ctx>(self.context.clone(), request)
            .await
    }

    /// Plan a write request into the transactions required to fulfil it, without
    /// executing them.
    pub async fn plan_request<S>(&self, request: WriteRequest<S>) -> GatewayResult<OperationPlan>
    where
        S: MethodSpec<Output = WriteOperationResult>,
        D: PlanWrite<S, Ctx>,
    {
        <D as PlanWrite<S, Ctx>>::plan(request, self.context.clone()).await
    }
}

/// A [`Client`] bound to a default signing account.
///
/// Constructing one guarantees a signer is registered for the account, so
/// [`SigningClient::execute`] needs no account argument and cannot fail for a
/// missing-signer reason. Derefs to the underlying [`Client`] for reads and
/// explicit-signer writes.
pub struct SigningClient<D = MethodsDispatch, Ctx = GatewayContext> {
    client: Client<D, Ctx>,
    signer_account_id: ManagedAccountId,
}

impl<D, Ctx: Clone> Clone for SigningClient<D, Ctx> {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            signer_account_id: self.signer_account_id.clone(),
        }
    }
}

impl<D, Ctx> std::fmt::Debug for SigningClient<D, Ctx> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningClient")
            .field("signer_account_id", &self.signer_account_id)
            .finish_non_exhaustive()
    }
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
}

impl<D, Ctx> SigningClient<D, Ctx> {
    /// The bound signing account.
    #[must_use]
    pub fn account_id(&self) -> &ManagedAccountId {
        &self.signer_account_id
    }
}

impl<D, Ctx: Clone> SigningClient<D, Ctx> {
    /// Rebind the dispatcher, keeping the bound signing account.
    #[must_use]
    pub fn via<D2>(&self) -> SigningClient<D2, Ctx> {
        SigningClient {
            client: self.client.via::<D2>(),
            signer_account_id: self.signer_account_id.clone(),
        }
    }

    /// Plan and execute a write operation signed by the bound account.
    pub async fn execute<Op>(&self, op: Op) -> GatewayResult<WriteOperationResult>
    where
        Op: MethodSpec<Output = WriteOperationResult>,
        D: PlanWrite<Op, Ctx>,
    {
        self.client
            .execute_as(self.signer_account_id.clone(), op)
            .await
    }
}

impl<D, Ctx> Deref for SigningClient<D, Ctx> {
    type Target = Client<D, Ctx>;

    fn deref(&self) -> &Client<D, Ctx> {
        &self.client
    }
}

#[cfg(test)]
mod tests {
    use templar_common::registry::DeployMode;
    use templar_contract_artifacts::ArtifactId;
    use templar_gateway_artifacts_dispatch::Dispatch as ArtifactsDispatch;
    use templar_gateway_artifacts_spec::artifact::{
        AddArtifactVersion, GetArtifact, ListArtifacts,
    };
    use templar_gateway_core::{FinalityPolicy, GatewayContext, HasNearClient, PlanWrite};
    use templar_gateway_types::NearToken;

    use super::{Client, Network, NetworkConfigBuilder};

    #[test]
    fn client_builder_defaults_to_final_policy() {
        let (context, _, _) = Client::builder(NetworkConfigBuilder::new(Network::Testnet).build())
            .build_parts()
            .expect("testnet network config is valid");

        assert_eq!(
            context.near_client().finality_policy(),
            FinalityPolicy::Final
        );
    }

    #[test]
    fn client_builder_propagates_optimistic_policy() {
        let (context, _, _) = Client::builder(NetworkConfigBuilder::new(Network::Testnet).build())
            .finality_policy(FinalityPolicy::ExecutedOptimistic)
            .build_parts()
            .expect("testnet network config is valid");

        assert_eq!(
            context.near_client().finality_policy(),
            FinalityPolicy::ExecutedOptimistic
        );
    }

    #[tokio::test]
    async fn client_reads_artifact_list() {
        let client = Client::read_only(NetworkConfigBuilder::new(Network::Testnet).build())
            .expect("testnet network config is valid");

        let result = client
            .via::<ArtifactsDispatch>()
            .read(ListArtifacts {})
            .await
            .expect("artifact.list dispatch succeeds");

        assert!(result
            .artifacts
            .iter()
            .any(|metadata| metadata.artifact == ArtifactId::Market));
    }

    #[tokio::test]
    async fn client_reads_artifact_bytes() {
        let client = Client::read_only(NetworkConfigBuilder::new(Network::Testnet).build())
            .expect("testnet network config is valid");

        let result = client
            .via::<ArtifactsDispatch>()
            .read(GetArtifact {
                artifact: ArtifactId::Market,
            })
            .await
            .expect("artifact.get dispatch succeeds");

        assert_eq!(&result.code.0[0..4], b"\0asm");
    }

    #[test]
    fn client_dispatch_accepts_add_artifact_version() {
        fn assert_plan_write<Spec>()
        where
            Spec: templar_gateway_types::MethodSpec<
                Output = templar_gateway_types::common::WriteOperationResult,
            >,
            ArtifactsDispatch: PlanWrite<Spec, GatewayContext>,
        {
        }

        let _ = AddArtifactVersion {
            registry_id: "registry.near".parse().expect("valid account id"),
            artifact: ArtifactId::MockFt,
            deploy_mode: DeployMode::Normal,
            deposit: NearToken::from_yoctonear(1),
        };
        assert_plan_write::<AddArtifactVersion>();
    }
}
