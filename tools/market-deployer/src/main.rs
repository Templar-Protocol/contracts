use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use market_deployer::commands::{
    add_version, deploy_from_registry, deploy_registry, recover_nep141, remove_all_markets,
    remove_all_versions, remove_market, remove_registry, remove_version, storage_deposit,
};
use near_crypto::{InMemorySigner, SecretKey, Signer};
use near_sdk::{AccountId, NearToken};
use templar_common::utils::Network;

// ── entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Context::parse();

    let rpc_url = cli
        .rpc_url
        .as_deref()
        .unwrap_or_else(|| cli.network.rpc_url())
        .to_owned();

    tracing::info!(network = %cli.network, rpc_url = %rpc_url, "Connecting");

    let near = near_fetch::Client::new(&rpc_url);
    let workspace = &cli.workspace_dir;

    match cli.command {
        Commands::DeployRegistry(a) => {
            deploy_registry::run(
                &near,
                workspace,
                a.signer.account_id,
                a.signer.secret_key,
                a.no_init,
            )
            .await?;
        }

        Commands::AddVersion(a) => {
            add_version::run(
                &near,
                workspace,
                a.signer.account_id,
                a.signer.secret_key,
                a.registry_id,
                &a.package,
                &a.version_key,
                a.deploy_mode.into(),
                a.deposit,
            )
            .await?;
        }

        Commands::AddMarketVersion(a) => {
            add_version::build_and_run(
                &near,
                workspace,
                a.signer.account_id,
                a.signer.secret_key,
                a.registry_id,
                "templar_market_contract",
                "contract/market",
                &a.version_key,
                a.deploy_mode.into(),
                a.deposit,
            )
            .await?;
        }

        Commands::AddUacVersion(a) => {
            let registry_id = a.registry_id.unwrap_or_else(|| a.signer.account_id.clone());
            add_version::build_and_run(
                &near,
                workspace,
                a.signer.account_id,
                a.signer.secret_key,
                registry_id,
                "templar_universal_account_contract",
                "contract/universal-account",
                &a.version_key,
                a.deploy_mode.into(),
                a.deposit,
            )
            .await?;
        }

        Commands::DeployFromRegistry(a) => {
            deploy_from_registry::run(
                &near,
                a.signer.account_id,
                a.signer.secret_key,
                a.registry_id,
                &a.version_key,
                &a.init_args,
                a.name.as_deref(),
                a.with_full_access_key.as_deref(),
                &a.method,
            )
            .await?;
        }

        Commands::RemoveMarket(a) => {
            remove_market::run(
                &near,
                a.signer.account_id,
                a.signer.secret_key,
                a.beneficiary_id,
            )
            .await?;
        }

        Commands::RemoveRegistry(a) => {
            remove_registry::run(
                &near,
                a.signer.account_id,
                a.signer.secret_key,
                a.beneficiary_id,
            )
            .await?;
        }

        Commands::RemoveAllMarkets(a) => {
            remove_all_markets::run(&near, a.secret_key, a.registry_id).await?;
        }

        Commands::RemoveAllVersions(a) => {
            remove_all_versions::run(
                &near,
                a.signer.account_id,
                a.signer.secret_key,
                a.registry_id,
            )
            .await?;
        }

        Commands::RemoveVersion(a) => {
            remove_version::run(
                &near,
                a.signer.account_id,
                a.signer.secret_key,
                a.registry_id,
                &a.version_key,
            )
            .await?;
        }

        Commands::StorageDeposit(a) => {
            storage_deposit::run(
                &near,
                a.signer.account_id,
                a.signer.secret_key,
                a.contract_id,
            )
            .await?;
        }

        Commands::RecoverNep141(a) => {
            recover_nep141::run(
                &near,
                a.signer.account_id,
                a.signer.secret_key,
                a.token_id,
                a.beneficiary_id,
            )
            .await?;
        }
    }

    tracing::info!("Done");
    Ok(())
}
