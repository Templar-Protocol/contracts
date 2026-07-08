mod cli;
mod commands;
mod dispatch;
mod proxy;

#[cfg(test)]
mod tests;

use anyhow::Context as _;
use clap::Parser;
use near_api::{NetworkConfig, SecretKey};
use std::sync::Arc;
use templar_gateway_client::{Client, NetworkConfigBuilder};
use templar_gateway_types::{
    common::WriteOperationResult, primitive::PublicKey, IdempotencyKey, ManagedAccountId,
};

use cli::Cli;

struct CliContext {
    client: Client,
    network: NetworkConfig,
    signer_account_id: Option<ManagedAccountId>,
    signer_secret_key: Option<SecretKey>,
    signer_public_key: Option<PublicKey>,
    idempotency_key: Option<IdempotencyKey>,
    has_operation_store: bool,
    transaction_url_prefix: String,
}

impl CliContext {
    fn signer_account(&self) -> anyhow::Result<ManagedAccountId> {
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
    /// `--secret-key`. Used by teardown flows (e.g. `registry deployment clear`)
    /// that must sign as many discovered accounts with one authorized key.
    fn signing_client_for(
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

    /// Log the explorer link for a completed write to stderr (the JSON result,
    /// carrying every step's hash, still goes to stdout).
    fn report_tx(&self, result: &WriteOperationResult) {
        if let Some(tx_hash) = result.operation.latest_tx_hash() {
            tracing::info!("tx: {}{}", self.transaction_url_prefix, tx_hash);
        }
    }
}

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.console_level());
    tracing::info!(network = %cli.network, "Connecting");
    let ctx = build_context(&cli).await?;
    dispatch::dispatch(ctx, cli.command).await
}

fn init_tracing(console_default: tracing::level_filters::LevelFilter) {
    use tracing::level_filters::LevelFilter;
    use tracing_subscriber::{
        fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
    };

    let console_filter = EnvFilter::builder()
        .with_default_directive(console_default.into())
        .from_env_lossy();
    // Logs are diagnostics; keep stdout clean for machine-readable JSON results.
    let console_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(console_filter);
    let registry = tracing_subscriber::registry().with(console_layer);

    // Best-effort daily-rotating file log under the OS state dir; console-only
    // if it can't be set up.
    let file_layer = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|dir| dir.join(env!("CARGO_PKG_NAME")).join("logs"))
        .and_then(|log_dir| {
            tracing_appender::rolling::RollingFileAppender::builder()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("log")
                .build(&log_dir)
                .ok()
        })
        .map(|file_appender| {
            fmt::layer()
                .with_ansi(false)
                .with_writer(file_appender)
                .with_filter(LevelFilter::DEBUG)
        });

    if let Some(file_layer) = file_layer {
        registry.with(file_layer).init();
    } else {
        registry.init();
    }
}

async fn build_context(cli: &Cli) -> anyhow::Result<CliContext> {
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
        network,
        signer_account_id: signer_account_id.map(ManagedAccountId::from),
        signer_secret_key,
        signer_public_key,
        idempotency_key: cli.idempotency_key.clone().map(IdempotencyKey),
        has_operation_store,
        transaction_url_prefix: cli.transaction_url_prefix(),
    })
}
