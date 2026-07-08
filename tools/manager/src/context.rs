//! The CLI execution context: a gateway [`Client`] plus the resolved signer and
//! presentation settings, and the small set of helpers every dispatch path uses
//! to read, write, report, and print. Keeping this in one place means the
//! context's whole surface lives together rather than scattered across dispatch.

use anyhow::Context as _;
use near_api::{NetworkConfig, SecretKey};
use serde::Serialize;
use std::io::Write as _;
use templar_gateway_client::{Client, NetworkConfigBuilder};
use templar_gateway_core::{DispatchRead, GatewayContext, PlanWrite};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_types::{
    common::WriteOperationResult, primitive::PublicKey, ManagedAccountId, MethodSpec,
};

use crate::cli::Cli;

pub(crate) struct CliContext {
    pub(crate) client: Client,
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

    /// The signer's public key, used by commands that grant it a full access key
    /// on a newly created account.
    pub(crate) fn signer_public_key(&self) -> anyhow::Result<PublicKey> {
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

    /// Dispatch a read and print its JSON result.
    pub(crate) async fn read<S>(&self, request: S) -> anyhow::Result<()>
    where
        S: MethodSpec,
        Dispatch: DispatchRead<S, GatewayContext>,
    {
        let output = self.client.read(request).await?;
        print_json(&output)
    }

    /// Execute a write signed by the default signer, report the tx link, and
    /// print the JSON result.
    pub(crate) async fn write<S>(&self, body: S) -> anyhow::Result<()>
    where
        S: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<S, GatewayContext>,
    {
        let output = self.client.execute_as(self.signer_account()?, body).await?;
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
/// client (backed by a transient in-memory operation store), and the resolved
/// signer.
pub(crate) fn build_context(cli: &Cli) -> anyhow::Result<CliContext> {
    let network = NetworkConfigBuilder::new(cli.network)
        .rpc_url(cli.rpc_url.as_deref())
        .context("invalid RPC URL")?
        .api_key(cli.rpc_api_key.clone())
        .build();

    let builder = Client::builder(network.clone());
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
        transaction_url_prefix: cli.transaction_url_prefix(),
    })
}

#[cfg(test)]
impl CliContext {
    /// A signer-configured context for tests: an offline client plus a fixed
    /// signer key, so FAK-resolving conversions can read [`signer_public_key`].
    ///
    /// [`signer_public_key`]: CliContext::signer_public_key
    pub(crate) fn for_test() -> Self {
        use templar_gateway_client::Network;

        // A throwaway ed25519 key; tests never submit, only read its public half.
        const TEST_SECRET_KEY: &str = "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q";
        let secret: SecretKey = TEST_SECRET_KEY.parse().expect("valid test secret key");
        let public_key = PublicKey::from(secret.public_key());
        let network = NetworkConfigBuilder::new(Network::Testnet).build();
        let account = ManagedAccountId::from(
            "signer.testnet"
                .parse::<near_account_id::AccountId>()
                .expect("valid account id"),
        );
        let client = Client::builder(network.clone())
            .secret_key(account.clone(), secret.clone())
            .expect("configure test signer")
            .build()
            .expect("build test client");
        Self {
            client,
            network,
            signer_account_id: Some(account),
            signer_secret_key: Some(secret),
            signer_public_key: Some(public_key),
            transaction_url_prefix: String::new(),
        }
    }
}
