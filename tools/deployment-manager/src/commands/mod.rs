use clap::Args;
use near_crypto::{InMemorySigner, PublicKey, SecretKey, Signer};
use near_fetch::ops::Function;
use near_sdk::{json_types::Base64VecU8, AccountId, NearToken};
use serde_json::json;
use templar_tools_common::{
    build::{build_contract, load_contract, LoadedContract},
    near::contract_version,
    version::RegistryVersion,
};

pub mod market;
pub mod proxy_oracle;
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

/// Shared arguments for deploying a contract from a registry.
#[derive(Args, Debug)]
pub struct DeployFromRegistry {
    #[arg(long)]
    registry_id: AccountId,
    /// Version key to deploy from the registry
    #[arg(long)]
    version_key: String,
    /// Name of the contract that will be deployed
    ///
    /// This will be used as the prefix for the account ID.
    #[arg(long)]
    name: String,
    /// Additional public keys to add as full access keys to the new account.
    /// The signer's public key is included by default unless --no-signer-full-access-key is set.
    #[arg(long)]
    with_full_access_key: Vec<PublicKey>,
    /// Do not add the signer's public key as a full access key on the new account
    #[arg(long)]
    no_signer_full_access_key: bool,
    /// Deposit to send with the deployment
    #[arg(long)]
    deposit: Option<NearToken>,
}

impl DeployFromRegistry {
    pub async fn run(
        &self,
        ctx: &crate::CliContext,
        signer: &Signer,
        init_args: Vec<u8>,
    ) -> anyhow::Result<()> {
        let registry_version: RegistryVersion =
            contract_version(&ctx.near, &self.registry_id).await?;
        tracing::debug!(%registry_version);

        let deposit = self.deposit.unwrap_or_else(|| {
            if registry_version.supports_global_contracts() {
                NearToken::from_yoctonear(1)
            } else {
                NearToken::from_near(6)
            }
        });
        tracing::debug!(%deposit);

        let full_access_keys = {
            let mut keys = self.with_full_access_key.clone();
            if !self.no_signer_full_access_key {
                let signer_key = signer.public_key();
                if !keys.contains(&signer_key) {
                    keys.push(signer_key);
                }
            }
            keys
        };
        tracing::debug!(?full_access_keys);

        let method = registry_version.deploy_method_name();

        ctx.batch(signer, &self.registry_id)
            .call(
                Function::new(method)
                    .deposit(deposit)
                    .max_gas()
                    .args_json(json!({
                        "name": self.name,
                        "version_key": self.version_key,
                        "init_args": Base64VecU8(init_args),
                        "full_access_keys": full_access_keys,
                    })),
            )
            .transact()
            .await
    }
}
