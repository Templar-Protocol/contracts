use clap::Args;
use near_crypto::{InMemorySigner, SecretKey, Signer};
use near_sdk::AccountId;
use templar_tools_common::build::{build_contract, load_contract, LoadedContract};

pub mod deployment;
pub mod json_input;
pub mod market;
pub mod proxy_oracle;
pub mod recover_nep141;
pub mod redstone_adapter;
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
    pub package: Option<String>,
}

impl ContractWasm {
    pub fn load_contract<T>(
        &self,
        context: &crate::CliContext,
        default_package: &str,
    ) -> anyhow::Result<LoadedContract<T>> {
        self.fixed
            .load_contract(context, self.package.as_deref().unwrap_or(default_package))
    }
}

/// Arguments common to every command that signs a transaction.
#[derive(Args, Clone)]
pub struct SignerArgs {
    /// Account ID to sign transactions as
    #[arg(long, env = "ACCOUNT_ID")]
    pub account_id: AccountId,

    /// Ed25519 private key for signing (ed25519:...)
    #[arg(long, env = "SECRET_KEY")]
    pub secret_key: SecretKey,
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
    pub fn new(account_id: AccountId, secret_key: SecretKey) -> Self {
        Self {
            account_id,
            secret_key,
        }
    }

    pub fn signer(&self) -> Signer {
        InMemorySigner::from_secret_key(self.account_id.clone(), self.secret_key.clone())
    }
}

/// Check if the account exists and, if so, delete it and send remaining funds
/// to `beneficiary_id`. Returns `Ok(false)` if the account did not exist.
pub async fn delete_account(
    ctx: &crate::CliContext,
    signer: &SignerArgs,
    beneficiary_id: &AccountId,
) -> anyhow::Result<bool> {
    if !crate::near::account_exists(&ctx.near, &signer.account_id).await? {
        tracing::info!(account_id = %signer.account_id, "Account does not exist, nothing to do");
        return Ok(false);
    }

    let s = signer.signer();
    ctx.batch(&s, &signer.account_id)
        .delete_account(beneficiary_id)
        .transact()
        .await?;

    Ok(true)
}
