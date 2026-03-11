use base64::{engine::general_purpose::STANDARD, Engine};
use near_sdk::json_types::Base64VecU8;
use near_sdk::{AccountId, NearToken, PublicKey};
use templar_tools_common::near::contract_version;
use templar_tools_common::version::RegistryVersion;

#[derive(serde::Serialize, Debug)]
struct DeployMethodArgs {
    name: String,
    version_key: String,
    init_args: Base64VecU8,
    full_access_keys: Option<Vec<near_sdk::PublicKey>>,
}

#[derive(clap::Args, Debug)]
pub struct DeployFromRegistry {
    #[command(flatten)]
    signer: super::SignerArgs,
    #[arg(long, env = "REGISTRY_ID")]
    registry_id: AccountId,
    /// Version key to deploy from the registry
    #[arg(long)]
    version_key: String,
    /// JSON-encoded init args to pass to the deployed contract
    #[arg(long)]
    init_args: serde_json::Value,
    /// Name of the contract that will be deployed
    ///
    /// This will be used as the prefix for the account ID.
    #[arg(long)]
    name: String,
    /// Public keys to add as full access keys to the new account
    #[arg(long)]
    with_full_access_key: Vec<PublicKey>,
}

impl DeployFromRegistry {
    #[tracing::instrument(skip(context))]
    pub async fn run(&self, context: &crate::CliContext) -> anyhow::Result<()> {
        let registry_version: RegistryVersion =
            contract_version(&context.near, &self.registry_id).await?;

        let deposit = if registry_version.supports_global_contracts() {
            NearToken::from_yoctonear(1)
        } else {
            NearToken::from_near(6)
        };

        let init_args = serde_json::to_vec(&self.init_args)?;
        let args = DeployMethodArgs {
            name: self.name.clone(),
            version_key: self.version_key.clone(),
            init_args: Base64VecU8::from(init_args),
            full_access_keys: Some(self.with_full_access_key.clone()),
        };

        tracing::info!(%deposit, "Deploying from registry");
        tracing::debug!(args = ?args);

        context
            .near
            .call(
                &self.signer.signer(),
                &self.registry_id,
                registry_version.deploy_method_name(),
            )
            .deposit(deposit)
            .max_gas()
            .args_json(args)
            .transact()
            .await?;

        tracing::info!("Deployed from registry");

        Ok(())
    }
}

/// Base64-encode `init_args` as required by the registry `deploy` / `deploy_market` methods.
pub fn encode_init_args(init_args: &str) -> String {
    STANDARD.encode(init_args.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_init_args_round_trips() {
        let input = r#"{"oracle_id":"pyth.near","borrow_asset":"usdt.near"}"#;
        let encoded = encode_init_args(input);
        let decoded = String::from_utf8(STANDARD.decode(&encoded).unwrap()).unwrap();
        assert_eq!(decoded, input);
    }

    #[test]
    fn encode_init_args_empty_json() {
        let encoded = encode_init_args("{}");
        let decoded = String::from_utf8(STANDARD.decode(&encoded).unwrap()).unwrap();
        assert_eq!(decoded, "{}");
    }

    #[test]
    fn encode_init_args_produces_valid_base64() {
        let encoded = encode_init_args(r#"{"key":"value"}"#);
        // Valid standard base64 uses only alphanumeric, +, /, and = padding.
        assert!(encoded
            .chars()
            .all(|c| c.is_alphanumeric() || c == '+' || c == '/' || c == '='));
    }

    #[test]
    fn encode_init_args_is_deterministic() {
        let input = r#"{"a":1}"#;
        assert_eq!(encode_init_args(input), encode_init_args(input));
    }
}
