mod cli;
mod commands;
mod dispatch;
mod proxy;

#[cfg(test)]
mod tests;

use anyhow::Context as _;
use clap::Parser;
use near_api::SecretKey;
use std::sync::Arc;
use templar_gateway_client::{Client, NetworkConfigBuilder};
use templar_gateway_types::{IdempotencyKey, ManagedAccountId};

use cli::Cli;

struct CliContext {
    client: Client,
    signer_account_id: Option<ManagedAccountId>,
    idempotency_key: Option<IdempotencyKey>,
    has_operation_store: bool,
}

impl CliContext {
    fn signer_account(&self) -> anyhow::Result<ManagedAccountId> {
        self.signer_account_id
            .clone()
            .context("write methods require --signer-id and --secret-key")
    }
}

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let ctx = build_context(&cli).await?;
    dispatch::dispatch(ctx, cli.command).await
}

async fn build_context(cli: &Cli) -> anyhow::Result<CliContext> {
    let network = NetworkConfigBuilder::new(cli.network)
        .rpc_url(cli.rpc_url.as_deref())
        .context("invalid RPC URL")?
        .api_key(cli.rpc_api_key.clone())
        .build();

    let has_operation_store = cli.gateway_store_url.is_some();
    let builder = Client::builder(network);
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
    let builder = match (&signer_account_id, &cli.secret_key) {
        (Some(account_id), Some(secret_key)) => {
            let secret = secret_key
                .parse::<SecretKey>()
                .map_err(|_| anyhow::anyhow!("invalid --secret-key"))?;
            builder.secret_key(account_id.clone(), secret)?
        }
        (None, None) => builder,
        (Some(_), None) => anyhow::bail!("--secret-key is required with --signer-id"),
        (None, Some(_)) => anyhow::bail!("--signer-id is required with --secret-key"),
    };

    Ok(CliContext {
        client: builder.build()?,
        signer_account_id: signer_account_id.map(ManagedAccountId::from),
        idempotency_key: cli.idempotency_key.clone().map(IdempotencyKey),
        has_operation_store,
    })
}
