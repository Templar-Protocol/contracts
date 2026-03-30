use near_crypto::InMemorySigner;

use near_crypto::Signer;

use near_crypto::SecretKey;

use near_sdk::AccountId;

/// Arguments common to every command that signs a transaction.
#[derive(clap::Args, Clone)]
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
