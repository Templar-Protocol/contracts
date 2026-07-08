//! Signer credentials as structural, per-operation arguments. Flattening these
//! into a write command's args makes the invalid states unrepresentable at the
//! parser: a write with no credentials can't parse, and credentials on a read are
//! an unexpected argument — enforcement lives in clap, not a runtime check.

use clap::Args;
use near_account_id::AccountId;
use near_api::SecretKey;
use templar_gateway_types::{primitive::PublicKey, ManagedAccountId};

/// The account that signs a write and its secret key — both required. Flatten
/// into every write command's args so the pairing is structural: neither half
/// can be supplied without the other.
#[derive(Args, Debug, Clone)]
pub struct SignerArgs {
    /// Account that signs the transaction.
    #[arg(long, env = "SIGNER_ID", value_name = "ACCOUNT_ID")]
    signer_id: AccountId,
    /// Private key for `--signer-id`, in `ed25519:…` form.
    #[arg(
        long,
        env = "SECRET_KEY",
        hide_env_values = true,
        value_name = "SECRET_KEY"
    )]
    secret_key: String,
}

impl SignerArgs {
    /// The signing account.
    pub fn account_id(&self) -> ManagedAccountId {
        ManagedAccountId::from(self.signer_id.clone())
    }

    /// The signer's public key, used by from-registry deploys that grant it a
    /// full access key on the new account.
    pub fn public_key(&self) -> anyhow::Result<PublicKey> {
        Ok(PublicKey::from(self.secret()?.public_key()))
    }

    /// The signing account together with its secret key.
    pub fn resolve(&self) -> anyhow::Result<(ManagedAccountId, SecretKey)> {
        Ok((self.account_id(), self.secret()?))
    }

    fn secret(&self) -> anyhow::Result<SecretKey> {
        parse_secret_key(&self.secret_key)
    }
}

/// A secret key with no bound account — for teardown flows (e.g. `registry
/// clear-deployments`) that sign many discovered accounts with one authorized
/// key, so there is no single `--signer-id`.
#[derive(Args, Debug, Clone)]
pub struct SecretKeyArgs {
    /// Private key that signs each discovered account, in `ed25519:…` form.
    #[arg(
        long,
        env = "SECRET_KEY",
        hide_env_values = true,
        value_name = "SECRET_KEY"
    )]
    secret_key: String,
}

impl SecretKeyArgs {
    /// The parsed secret key.
    pub fn secret(&self) -> anyhow::Result<SecretKey> {
        parse_secret_key(&self.secret_key)
    }
}

/// Parse a secret key without echoing the input in the error (it is a secret).
fn parse_secret_key(secret_key: &str) -> anyhow::Result<SecretKey> {
    secret_key
        .parse::<SecretKey>()
        .map_err(|_| anyhow::anyhow!("invalid --secret-key"))
}
