mod command;
mod dispatch;
mod params;
mod typed;

#[cfg(test)]
mod tests;

use anyhow::Context as _;
use clap::ArgMatches;
use near_account_id::AccountId;
use near_api::SecretKey;
use serde_json::Value;
use std::sync::Arc;
use templar_gateway_client::{Client, Network, NetworkConfigBuilder};
use templar_gateway_types::{IdempotencyKey, ManagedAccountId};

pub(super) struct GatewayCli {
    network: Network,
    rpc_url: Option<String>,
    rpc_api_key: Option<String>,
    signer_account_id: Option<AccountId>,
    secret_key: Option<SecretKey>,
    gateway_store_url: Option<String>,
    migrate_gateway_store: bool,
    idempotency_key: Option<String>,
    rpc_method: String,
    params: Value,
}

pub async fn run() -> anyhow::Result<()> {
    let matches = command::command().get_matches();
    let cli = GatewayCli::from_matches(&matches)?;
    let client = cli.client().await?;
    dispatch::dispatch(&client, &cli).await
}

impl GatewayCli {
    fn from_matches(matches: &ArgMatches) -> anyhow::Result<Self> {
        let (namespace, namespace_matches) = matches
            .subcommand()
            .context("missing gateway method namespace")?;
        let (method, method_matches) = namespace_matches
            .subcommand()
            .context("missing gateway method name")?;
        let rpc_method = format!("{namespace}.{method}");
        let params = params::load_params(method_matches, &rpc_method)?;
        let gateway_store_url = matches.get_one::<String>("gateway-store-url").cloned();
        if rpc_method == "op.get" && gateway_store_url.is_none() {
            anyhow::bail!("op.get requires --gateway-store-url");
        }

        let secret_key = matches
            .get_one::<String>("secret-key")
            .map(|value| {
                value
                    .parse::<SecretKey>()
                    .map_err(|_| anyhow::anyhow!("invalid --secret-key"))
            })
            .transpose()?;

        Ok(Self {
            network: *matches
                .get_one::<Network>("network")
                .context("missing network")?,
            rpc_url: matches.get_one::<String>("rpc-url").cloned(),
            rpc_api_key: matches.get_one::<String>("rpc-api-key").cloned(),
            signer_account_id: matches.get_one::<AccountId>("signer-id").cloned(),
            secret_key,
            gateway_store_url,
            migrate_gateway_store: matches.get_flag("migrate-gateway-store"),
            idempotency_key: matches.get_one::<String>("idempotency-key").cloned(),
            rpc_method,
            params,
        })
    }

    async fn client(&self) -> anyhow::Result<Client> {
        let network = NetworkConfigBuilder::new(self.network)
            .rpc_url(self.rpc_url.as_deref())
            .context("invalid RPC URL")?
            .api_key(self.rpc_api_key.clone())
            .build();

        let builder = Client::builder(network);
        let builder = if let Some(database_url) = self.gateway_store_url.as_deref() {
            let store = templar_gateway_store::PostgresStore::new(database_url)
                .context("connect gateway operation store")?;
            if self.migrate_gateway_store {
                store
                    .migrate()
                    .await
                    .context("migrate gateway operation store")?;
            }
            builder.store(Arc::new(store))
        } else {
            if self.migrate_gateway_store {
                anyhow::bail!("--migrate-gateway-store requires --gateway-store-url");
            }
            builder
        };
        let builder = match (&self.signer_account_id, &self.secret_key) {
            (Some(account_id), Some(secret_key)) => {
                builder.secret_key(account_id.clone(), secret_key.clone())?
            }
            (None, None) => builder,
            (Some(_), None) => anyhow::bail!("--secret-key is required with --signer-id"),
            (None, Some(_)) => anyhow::bail!("--signer-id is required with --secret-key"),
        };

        Ok(builder.build()?)
    }

    pub(super) fn rpc_method(&self) -> &str {
        &self.rpc_method
    }

    pub(super) fn params(&self) -> &Value {
        &self.params
    }

    pub(super) fn signer_account(&self) -> anyhow::Result<ManagedAccountId> {
        self.signer_account_id
            .clone()
            .map(ManagedAccountId::from)
            .context("write methods require --signer-id and --secret-key")
    }

    pub(super) fn idempotency_key(&self) -> Option<IdempotencyKey> {
        self.idempotency_key.clone().map(IdempotencyKey)
    }
}
