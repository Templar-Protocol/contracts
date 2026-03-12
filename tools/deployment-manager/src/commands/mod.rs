use clap::Args;
use near_crypto::{InMemorySigner, SecretKey, Signer};
use near_sdk::AccountId;
use templar_tools_common::build::{build_contract, load_contract, LoadedContract};

pub mod market;
pub mod recover_nep141;
pub mod registry;
pub mod storage_deposit;

#[derive(Args, Debug)]
pub struct FixedContractWasm {
    /// Skip the build step and use an existing WASM file. Warning: it may be stale!
    #[arg(long)]
    pub no_build: bool,
}

impl FixedContractWasm {
    pub fn load_contract<T>(
        &self,
        context: &crate::CliContext,
        package: &str,
    ) -> anyhow::Result<LoadedContract<T>> {
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
    pub fn load_contract<T>(
        &self,
        context: &crate::CliContext,
    ) -> anyhow::Result<LoadedContract<T>> {
        self.fixed.load_contract(context, &self.package)
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
