use clap::Args;
use near_crypto::{InMemorySigner, SecretKey, Signer};
use near_sdk::AccountId;
use templar_tools_common::build::{build_contract, load_contract};

pub mod add_version;
pub mod deploy_from_registry;
pub mod deploy_registry;
pub mod recover_nep141;
pub mod remove_all_markets;
pub mod remove_all_versions;
pub mod remove_market;
pub mod remove_registry;
pub mod remove_version;
pub mod storage_deposit;

#[derive(Args, Debug)]
pub struct FixedContractWasm {
    #[arg(long)]
    pub no_build: bool,
}

impl FixedContractWasm {
    pub fn wasm(&self, context: &crate::CliContext, package: &str) -> anyhow::Result<Vec<u8>> {
        if self.no_build {
            load_contract(&context.workspace_path, package)
        } else {
            build_contract(&context.workspace_path, package)
        }
    }
}

#[derive(Args, Debug)]
pub struct ContractWasm {
    #[command(flatten)]
    pub fixed: FixedContractWasm,
    #[arg(long)]
    pub package: String,
}

impl ContractWasm {
    pub fn wasm(&self, context: &crate::CliContext) -> anyhow::Result<Vec<u8>> {
        self.fixed.wasm(context, &self.package)
    }
}

/// Arguments common to every command that signs a transaction.
#[derive(Args, Clone)]
pub struct SignerArgs {
    /// Account ID to sign transactions as
    #[arg(long, env = "ACCOUNT_ID")]
    account_id: AccountId,

    /// Ed25519 private key for signing (ed25519:...)
    #[arg(long, env = "SECRET_KEY")]
    secret_key: SecretKey,
}

impl std::fmt::Debug for SignerArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignerArgs")
            .field("account_id", &self.account_id)
            .field("secret_key", &"***")
            .finish()
    }
}

impl std::fmt::Display for SignerArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.account_id.as_str())
    }
}

impl SignerArgs {
    pub fn signer(&self) -> Signer {
        InMemorySigner::from_secret_key(self.account_id.clone(), self.secret_key.clone())
    }
}
