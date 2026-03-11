use anyhow::Context;
use clap::Args;
use near_sdk::{AccountId, NearToken};
use templar_common::registry::DeployMode;

use crate::CliContext;

use super::{ContractWasm, FixedContractWasm, SignerArgs};

const MARKET_PACKAGE: &str = "templar-market-contract";
const UAC_PACKAGE: &str = "templar-universal-account-contract";

#[derive(Args, Debug)]
pub struct AddVersion {
    #[command(flatten)]
    signer: SignerArgs,
    #[command(flatten)]
    contract_wasm: ContractWasm,
    /// Registry contract account ID
    #[arg(long, env = "REGISTRY_ID")]
    registry_id: AccountId,
    /// Version key to store in the registry
    #[arg(long)]
    version_key: String,
    /// Deployment mode
    #[arg(long, default_value_t = DeployMode::Normal)]
    deploy_mode: DeployMode,
    /// Deposit to attach in NEAR (defaults to 1 yoctoNEAR for `normal` mode)
    #[arg(long)]
    deposit: Option<NearToken>,
}

impl AddVersion {
    #[tracing::instrument(skip(ctx))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let wasm = self.contract_wasm.wasm(ctx)?;
        send_add_version(
            ctx,
            &self.signer,
            &self.registry_id,
            &self.version_key,
            self.deploy_mode,
            self.deposit,
            &wasm,
        )
        .await
    }
}

/// Build the market contract and register it as a new version.
#[derive(Args, Debug)]
pub struct AddMarketVersion {
    #[command(flatten)]
    signer: SignerArgs,
    #[command(flatten)]
    contract: FixedContractWasm,
    #[arg(long, env = "REGISTRY_ID")]
    registry_id: AccountId,
    #[arg(long)]
    version_key: String,
    #[arg(long, default_value_t = DeployMode::Normal)]
    deploy_mode: DeployMode,
    #[arg(long)]
    deposit: Option<NearToken>,
}

impl AddMarketVersion {
    #[tracing::instrument(skip(ctx))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let wasm = self.contract.wasm(ctx, MARKET_PACKAGE)?;
        send_add_version(
            ctx,
            &self.signer,
            &self.registry_id,
            &self.version_key,
            self.deploy_mode,
            self.deposit,
            &wasm,
        )
        .await
    }
}

/// Build the universal-account contract and register it as a new version.
#[derive(Args, Debug)]
pub struct AddUacVersion {
    #[command(flatten)]
    signer: SignerArgs,
    #[command(flatten)]
    contract: FixedContractWasm,
    #[arg(long, env = "REGISTRY_ID")]
    registry_id: AccountId,
    #[arg(long)]
    version_key: String,
    #[arg(long, default_value_t = DeployMode::GlobalHash)]
    deploy_mode: DeployMode,
    #[arg(long)]
    deposit: Option<NearToken>,
}

impl AddUacVersion {
    #[tracing::instrument(skip(ctx))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let wasm = self.contract.wasm(ctx, UAC_PACKAGE)?;
        send_add_version(
            ctx,
            &self.signer,
            &self.registry_id,
            &self.version_key,
            self.deploy_mode,
            self.deposit,
            &wasm,
        )
        .await
    }
}

async fn send_add_version(
    ctx: &CliContext,
    signer: &SignerArgs,
    registry_id: &AccountId,
    version_key: &str,
    deploy_mode: DeployMode,
    deposit: Option<NearToken>,
    wasm: &[u8],
) -> anyhow::Result<()> {
    let borsh_args = encode_add_version_args(version_key, deploy_mode, wasm)?;
    let deposit = deposit.unwrap_or(NearToken::from_yoctonear(1));

    tracing::info!(wasm_bytes = wasm.len(), "Calling add_version on registry");

    ctx.near
        .call(&signer.signer(), registry_id, "add_version")
        .args(borsh_args)
        .deposit(deposit)
        .max_gas()
        .transact()
        .await
        .context("add_version")?;

    tracing::info!("Version registered");
    Ok(())
}

/// Borsh-encode the `add_version` arguments, matching the layout produced by
/// `test-utils/examples/registry_add_version_args.rs`.
///
/// The encoding is `(version_key: String, deploy_mode: DeployMode, wasm: Vec<u8>)`.
pub fn encode_add_version_args(
    version_key: &str,
    deploy_mode: DeployMode,
    wasm: &[u8],
) -> anyhow::Result<Vec<u8>> {
    borsh::to_vec(&(version_key, deploy_mode, wasm)).context("borsh-encode add_version args")
}

#[cfg(test)]
mod tests {
    use super::*;
    use templar_common::registry::DeployMode;

    #[test]
    fn encode_add_version_args_is_deterministic() {
        let wasm = b"fake_wasm";
        let a = encode_add_version_args("v1.0.0", DeployMode::Normal, wasm).unwrap();
        let b = encode_add_version_args("v1.0.0", DeployMode::Normal, wasm).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn encode_add_version_args_differs_by_deploy_mode() {
        let wasm = b"fake_wasm";
        let normal = encode_add_version_args("v1", DeployMode::Normal, wasm).unwrap();
        let global_hash = encode_add_version_args("v1", DeployMode::GlobalHash, wasm).unwrap();
        assert_ne!(normal, global_hash, "deploy mode must affect the encoding");
    }

    #[test]
    fn encode_add_version_args_differs_by_version_key() {
        let wasm = b"fake_wasm";
        let a = encode_add_version_args("v1", DeployMode::Normal, wasm).unwrap();
        let b = encode_add_version_args("v2", DeployMode::Normal, wasm).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn encode_add_version_args_round_trips_via_borsh() {
        let wasm: &[u8] = b"some_contract_bytes";
        let version_key = "v1.2.3";
        let deploy_mode = DeployMode::Normal;

        let encoded = encode_add_version_args(version_key, deploy_mode, wasm).unwrap();

        // Decode and verify each field via borsh reader
        let mut reader = encoded.as_slice();
        let decoded_key: String = borsh::BorshDeserialize::deserialize(&mut reader).unwrap();
        let decoded_mode: DeployMode = borsh::BorshDeserialize::deserialize(&mut reader).unwrap();
        let decoded_wasm: Vec<u8> = borsh::BorshDeserialize::deserialize(&mut reader).unwrap();

        assert_eq!(decoded_key, version_key);
        assert!(matches!(decoded_mode, DeployMode::Normal));
        assert_eq!(decoded_wasm, wasm);
    }
}
