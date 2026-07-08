//! The CLI execution context: a gateway [`Client`] plus the resolved signer and
//! presentation settings, and the small set of helpers every dispatch path uses
//! to read, write, report, and print. Keeping this in one place means the
//! context's whole surface lives together rather than scattered across dispatch.

use std::sync::Arc;

use anyhow::Context as _;
use near_api::{NetworkConfig, SecretKey};
use serde::Serialize;
use std::io::Write as _;
use templar_gateway_client::{Client, NetworkConfigBuilder};
use templar_gateway_core::{DispatchRead, GatewayContext, PlanWrite};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    primitive::PublicKey,
    IdempotencyKey, ManagedAccountId, MethodSpec,
};

use crate::cli::Cli;
use crate::commands::registry;

pub(crate) struct CliContext {
    pub(crate) client: Client,
    pub(crate) idempotency_key: Option<IdempotencyKey>,
    pub(crate) has_operation_store: bool,
    network: NetworkConfig,
    signer_account_id: Option<ManagedAccountId>,
    signer_secret_key: Option<SecretKey>,
    signer_public_key: Option<PublicKey>,
    transaction_url_prefix: String,
}

impl CliContext {
    /// The signing account, or an error if no `--signer-id`/`--secret-key` was given.
    pub(crate) fn signer_account(&self) -> anyhow::Result<ManagedAccountId> {
        self.signer_account_id
            .clone()
            .context("write methods require --signer-id and --secret-key")
    }

    /// The signer's public key, used to grant it a full access key on accounts
    /// deployed from a registry.
    fn signer_public_key(&self) -> anyhow::Result<PublicKey> {
        self.signer_public_key
            .clone()
            .context("this operation requires --signer-id and --secret-key")
    }

    /// Build a single-signer client for an arbitrary account using the shared
    /// `--secret-key`. Used by teardown flows (e.g. `registry clear-deployments`)
    /// that must sign as many discovered accounts with one authorized key.
    pub(crate) fn signing_client_for(
        &self,
        account_id: impl Into<ManagedAccountId>,
    ) -> anyhow::Result<Client> {
        let secret_key = self
            .signer_secret_key
            .clone()
            .context("this operation requires --secret-key")?;
        Client::builder(self.network.clone())
            .secret_key(account_id, secret_key)?
            .build()
            .context("build signing client")
    }

    /// Resolve the full access keys for a from-registry deploy from the CLI
    /// flags and the signer's public key.
    pub(crate) fn resolve_full_access_keys(
        &self,
        no_signer: bool,
        extra: &[near_api::PublicKey],
    ) -> anyhow::Result<Vec<PublicKey>> {
        Ok(registry::resolve_full_access_keys(
            self.signer_public_key()?,
            no_signer,
            extra,
        ))
    }

    /// The default full access keys for a deploy: just the signer's key.
    pub(crate) fn default_full_access_keys(&self) -> anyhow::Result<Vec<PublicKey>> {
        self.resolve_full_access_keys(false, &[])
    }

    /// Dispatch a read and print its JSON result.
    pub(crate) async fn read<S>(&self, request: S) -> anyhow::Result<()>
    where
        S: MethodSpec,
        Dispatch: DispatchRead<S, GatewayContext>,
    {
        let output = self.client.read(request).await?;
        print_json(&output)
    }

    /// Execute a write signed by the default signer (carrying its idempotency
    /// key), report the tx link, and print the JSON result.
    pub(crate) async fn write<S>(&self, body: S) -> anyhow::Result<()>
    where
        S: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<S, GatewayContext>,
    {
        let output = self
            .client
            .execute_request(WriteRequest {
                signer_account_id: self.signer_account()?,
                idempotency_key: self.idempotency_key.clone(),
                body,
            })
            .await?;
        self.report_tx(&output);
        print_json(&output)
    }

    /// Log the explorer link for a completed write to stderr (the JSON result,
    /// carrying every step's hash, still goes to stdout).
    pub(crate) fn report_tx(&self, result: &WriteOperationResult) {
        if let Some(tx_hash) = result.operation.latest_tx_hash() {
            tracing::info!("tx: {}{}", self.transaction_url_prefix, tx_hash);
        }
    }
}

/// Serialize `output` as a single line of JSON to stdout — the machine-readable
/// result channel (diagnostics go to stderr via tracing).
pub(crate) fn print_json(output: &impl Serialize) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer(&mut lock, output)?;
    writeln!(lock)?;
    Ok(())
}

/// Build the execution context from parsed CLI args: the network, the gateway
/// client (optionally backed by a durable operation store), and the resolved
/// signer.
pub(crate) async fn build_context(cli: &Cli) -> anyhow::Result<CliContext> {
    let network = NetworkConfigBuilder::new(cli.network)
        .rpc_url(cli.rpc_url.as_deref())
        .context("invalid RPC URL")?
        .api_key(cli.rpc_api_key.clone())
        .build();

    let has_operation_store = cli.gateway_store_url.is_some();
    let builder = Client::builder(network.clone());
    let builder = if let Some(database_url) = cli.gateway_store_url.as_deref() {
        let store = templar_gateway_store::PostgresStore::new(database_url)
            .context("connect gateway operation store")?;
        if cli.migrate_gateway_store {
            store
                .migrate()
                .await
                .context("migrate gateway operation store")?;
        }
        builder.store(Arc::new(store))
    } else if cli.migrate_gateway_store {
        anyhow::bail!("--migrate-gateway-store requires --gateway-store-url");
    } else {
        builder
    };

    let signer_account_id = cli.signer_id.clone();
    let (builder, signer_secret_key, signer_public_key) =
        match (&signer_account_id, &cli.secret_key) {
            (Some(account_id), Some(secret_key)) => {
                let secret = secret_key
                    .parse::<SecretKey>()
                    .map_err(|_| anyhow::anyhow!("invalid --secret-key"))?;
                let public_key = PublicKey::from(secret.public_key());
                let builder = builder.secret_key(account_id.clone(), secret.clone())?;
                (builder, Some(secret), Some(public_key))
            }
            (None, None) => (builder, None, None),
            (Some(_), None) => anyhow::bail!("--secret-key is required with --signer-id"),
            (None, Some(_)) => anyhow::bail!("--signer-id is required with --secret-key"),
        };

    Ok(CliContext {
        client: builder.build()?,
        idempotency_key: cli.idempotency_key.clone().map(IdempotencyKey),
        has_operation_store,
        network,
        signer_account_id: signer_account_id.map(ManagedAccountId::from),
        signer_secret_key,
        signer_public_key,
        transaction_url_prefix: cli.transaction_url_prefix(),
    })
}
