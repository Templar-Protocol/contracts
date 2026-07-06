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
    ops::Deref,
    sync::Arc,
};

use near_account_id::AccountId;
use near_api::{NetworkConfig, SecretKey, Signer};
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_core::{
    DispatchRead, GatewayContext, GatewayContextBuilder, GatewayError, GatewayResult,
    NearOperationExecutor, NearTransactionSigner, OperationDriver, OperationPlan, PlanWrite,
    SharedOperationStore,
};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_oracle_updates_dispatch::{
    Dispatch as OracleUpdatesDispatch, GatewayContextBuilderOracleExt, LazerSourceConfig,
    WithLazerSource, WithPythSource, WithRedStoneSource,
};
use templar_gateway_oracle_updates_spec::oracle::UpdatePrices;
use templar_gateway_store::MemoryStore;
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    IdempotencyKey, ManagedAccountId, MethodSpec, OperationRecord,
};

/// Re-exported so consumers can build an [`OracleSourceConfig`] without a direct
/// dependency on the oracle-updates dispatch crate.
pub use templar_gateway_oracle_updates_dispatch::LazerSubscriptionConfig;

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

    /// Build the base gateway context, signer set, executor, and store-backed
    /// operation driver shared by every client flavor.
    fn build_parts(
        self,
    ) -> GatewayResult<(GatewayContext, OperationDriver, HashSet<ManagedAccountId>)> {
        let context = GatewayContext::new(self.network.clone())?;
        let signer_account_ids = self.signers.keys().cloned().collect();
        let signer = NearTransactionSigner::new(self.network.clone(), self.signers);
        let executor = NearOperationExecutor::new(self.network);
        let driver = OperationDriver::new(self.store, Arc::new(signer), Arc::new(executor));
        Ok((context, driver, signer_account_ids))
    }

    /// Build the client, constructing the gateway context, signer, executor, and
    /// store-backed operation driver.
    pub fn build(self) -> GatewayResult<Client> {
        let (context, driver, signer_account_ids) = self.build_parts()?;
        Ok(Client {
            context,
            driver,
            signer_account_ids,
        })
    }

    /// Build both a methods [`Client`] and an [`OracleUpdatesClient`] that share one
    /// [`OperationDriver`] (store, signer pool, executor) and a base
    /// [`GatewayContext`] — so idempotency, replay, the startup-recovery sweep, and
    /// the NEAR read cache all span both. The oracle client additionally carries the
    /// in-process Pyth/RedStone/Lazer payload sources needed to plan `oracle.*`
    /// updates.
    pub fn build_with_oracle_updates(
        self,
        sources: OracleSourceConfig,
    ) -> GatewayResult<(Client, OracleUpdatesClient)> {
        let (base, driver, signer_account_ids) = self.build_parts()?;

        // Layer the sources onto a clone of the base context so both clients share
        // the same `Arc`-backed NEAR read cache (contract kinds, proxy definitions).
        let context = GatewayContextBuilder::new(base.clone())
            .with_pyth_source(sources.pyth_hermes_url)
            .with_redstone_source(&sources.redstone_node_path)?
            .with_lazer_source(sources.lazer)
            .build();

        let methods = Client {
            context: base,
            driver: driver.clone(),
            signer_account_ids: signer_account_ids.clone(),
        };
        let oracle_updates = OracleUpdatesClient {
            context,
            driver,
            signer_account_ids,
        };
        Ok((methods, oracle_updates))
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

impl std::fmt::Debug for Client {
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

impl std::fmt::Debug for SigningClient {
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

/// The layered gateway context carried by an [`OracleUpdatesClient`]: the base
/// context plus the in-process Pyth, RedStone, and Lazer payload sources.
pub type OracleUpdatesContext = WithLazerSource<WithRedStoneSource<WithPythSource<GatewayContext>>>;

/// Configuration for the in-process oracle payload sources an
/// [`OracleUpdatesClient`] fetches from when planning `oracle.*` updates.
#[derive(Debug, Clone)]
pub struct OracleSourceConfig {
    /// Pyth Hermes API URL.
    pub pyth_hermes_url: url::Url,
    /// Path to the Node.js interpreter (or equivalent) that runs the RedStone bridge.
    pub redstone_node_path: std::path::PathBuf,
    /// Pyth Pro/Lazer websocket source configuration.
    pub lazer: LazerSourceConfig,
}

/// A gateway client for the `oracle_updates` methods (e.g. `oracle.updatePrices`).
///
/// It shares its [`OperationDriver`] — and therefore the operation store,
/// signer pool, idempotency, replay, and startup recovery — with the methods
/// [`Client`] it was built alongside via
/// [`ClientBuilder::build_with_oracle_updates`]. Unlike the methods client it
/// carries the in-process payload sources, so it plans updates by fetching fresh
/// payloads server-side rather than having the caller supply them.
#[derive(Clone)]
pub struct OracleUpdatesClient {
    context: OracleUpdatesContext,
    driver: OperationDriver,
    signer_account_ids: HashSet<ManagedAccountId>,
}

impl std::fmt::Debug for OracleUpdatesClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OracleUpdatesClient")
            .field("signer_account_ids", &self.signer_account_ids)
            .finish_non_exhaustive()
    }
}

impl OracleUpdatesClient {
    /// Resolve and submit every oracle update `price_ids` require on `oracle_id`,
    /// signed by `signer_account_id`. The gateway resolves the oracle's
    /// Direct/Proxy/LST dependencies, fetches the underlying Pyth/RedStone/Lazer
    /// payloads in-process, and — for a proxy oracle — re-aggregates its cached
    /// prices, all as one store-backed operation.
    pub async fn execute_prices(
        &self,
        signer_account_id: impl Into<ManagedAccountId>,
        oracle_id: AccountId,
        price_ids: Vec<PriceIdentifier>,
    ) -> GatewayResult<WriteOperationResult> {
        self.driver
            .plan_and_complete::<UpdatePrices, OracleUpdatesDispatch, OracleUpdatesContext>(
                self.context.clone(),
                WriteRequest {
                    signer_account_id: signer_account_id.into(),
                    idempotency_key: None,
                    body: UpdatePrices {
                        oracle_id,
                        price_ids,
                    },
                },
            )
            .await
    }
}

#[cfg(feature = "clap")]
pub use source_args::OracleSourceArgs;

#[cfg(feature = "clap")]
mod source_args {
    use std::{path::PathBuf, time::Duration};

    use clap::Args;
    use templar_gateway_core::RedactedString;
    use url::Url;

    use super::{LazerSourceConfig, LazerSubscriptionConfig, OracleSourceConfig};

    /// Shared CLI surface for the gateway's in-process oracle payload sources (Pyth
    /// Hermes, RedStone bridge, Pyth Pro/Lazer websocket). Flatten it into a
    /// consumer's `clap` configuration and call [`OracleSourceArgs::build`].
    #[derive(Args, Debug, Clone)]
    pub struct OracleSourceArgs {
        /// Pyth Hermes API URL. See: <https://docs.pyth.network/price-feeds/core/api-reference>
        #[arg(
            long = "pyth-hermes-url",
            env = "PYTH_HERMES_URL",
            default_value = "https://hermes-beta.pyth.network"
        )]
        pub pyth_hermes_url: Url,

        /// Path to the Node.js interpreter (or equivalent) that runs the RedStone bridge.
        #[arg(
            long = "redstone-node-path",
            env = "REDSTONE_NODE_PATH",
            default_value = "node"
        )]
        pub redstone_node_path: PathBuf,

        /// Bearer token for Pyth Pro/Lazer websocket payload updates.
        #[arg(long = "pyth-lazer-api-key", env = "PYTH_LAZER_API_KEY")]
        pub pyth_lazer_api_key: RedactedString,

        /// Pyth Pro/Lazer websocket endpoint. Configures one endpoint only; automatic
        /// multi-endpoint failover is not implemented.
        #[arg(
            long = "pyth-lazer-ws-url",
            env = "PYTH_LAZER_WS_URL",
            default_value = "wss://pyth-lazer-0.dourolabs.app/v1/stream"
        )]
        pub pyth_lazer_ws_url: Url,

        /// Pyth Pro/Lazer websocket channel. One of: "real_time", "fixed_rate@50ms",
        /// "fixed_rate@200ms", "fixed_rate@1000ms". Validated when the source is built.
        #[arg(
            long = "pyth-lazer-channel",
            env = "PYTH_LAZER_CHANNEL",
            default_value = "fixed_rate@200ms"
        )]
        pub pyth_lazer_channel: String,

        /// Maximum age, in milliseconds, for cached Pyth Pro/Lazer payloads.
        #[arg(
            long = "pyth-lazer-max-payload-age-ms",
            env = "PYTH_LAZER_MAX_PAYLOAD_AGE_MS",
            default_value = "5000"
        )]
        pub pyth_lazer_max_payload_age_ms: u64,
    }

    impl OracleSourceArgs {
        /// Validate and assemble the runtime [`OracleSourceConfig`].
        ///
        /// # Errors
        /// Returns an error if the Lazer websocket configuration is invalid (empty
        /// token, non-`wss://` URL, unsupported channel, or zero payload age).
        pub fn build(&self) -> anyhow::Result<OracleSourceConfig> {
            let lazer = LazerSourceConfig::new(
                self.pyth_lazer_ws_url.clone(),
                self.pyth_lazer_api_key.clone(),
                LazerSubscriptionConfig {
                    channel: Some(self.pyth_lazer_channel.clone()),
                    max_payload_age: Duration::from_millis(self.pyth_lazer_max_payload_age_ms),
                },
            )
            .map_err(anyhow::Error::from)?;
            Ok(OracleSourceConfig {
                pyth_hermes_url: self.pyth_hermes_url.clone(),
                redstone_node_path: self.redstone_node_path.clone(),
                lazer,
            })
        }
    }
}
