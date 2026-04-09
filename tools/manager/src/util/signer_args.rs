use near_crypto::InMemorySigner;

use near_crypto::Signer;

use near_crypto::SecretKey;

use near_sdk::AccountId;

/// Arguments common to every command that signs a transaction.
#[derive(clap::Args, Clone)]
pub struct SignerArgs {
    /// Account ID used to sign transactions; some commands also act on this same account
    #[arg(long, env = "SIGNER_ID")]
    pub signer_id: AccountId,

    /// Ed25519 private key for signing (ed25519:...)
    #[arg(long, env = "SECRET_KEY")]
    pub secret_key: SecretKey,
}

impl std::fmt::Debug for SignerArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignerArgs")
            .field("signer_id", &self.signer_id)
            .field("secret_key", &"***")
            .finish()
    }
}

impl std::fmt::Display for SignerArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.signer_id.as_str())
    }
}

impl SignerArgs {
    pub fn new(signer_id: AccountId, secret_key: SecretKey) -> Self {
        Self {
            signer_id,
            secret_key,
        }
    }

    pub fn signer(&self) -> Signer {
        InMemorySigner::from_secret_key(self.signer_id.clone(), self.secret_key.clone())
    }
}
