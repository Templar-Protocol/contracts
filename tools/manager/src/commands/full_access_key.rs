use clap::Args;
use near_api::PublicKey as CliPublicKey;
use templar_gateway_types::primitive::PublicKey;

/// The shared full-access-key options for from-registry create/deploy commands,
/// flattened into each so every new account is provisioned the same way: the
/// signer's key is granted by default (so the operator retains control), plus any
/// explicit extras, unless the signer key is suppressed.
#[derive(Args, Debug)]
pub struct FullAccessKeyArgs {
    /// Additional full access key for the new account (repeatable).
    #[arg(long, value_name = "PUBLIC_KEY")]
    with_full_access_key: Vec<CliPublicKey>,
    /// Do not grant the signer's public key a full access key on the new account.
    #[arg(long)]
    no_signer_full_access_key: bool,
}

impl FullAccessKeyArgs {
    /// Resolve the keys granted to the new account: `signer_public_key` by default
    /// (unless suppressed), plus the explicit extras, de-duplicated.
    pub fn resolve(&self, signer_public_key: PublicKey) -> Vec<PublicKey> {
        let mut keys = Vec::with_capacity(self.with_full_access_key.len() + 1);
        if !self.no_signer_full_access_key {
            keys.push(signer_public_key);
        }
        for key in &self.with_full_access_key {
            let key = PublicKey::from(*key);
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_B: &str = "ed25519:5TMKtTtD5uuMF28ovo7vVge7oAu58eXjySJWTrwcEB5w";
    // A throwaway ed25519 secret; only its public half is used.
    const SIGNER_SECRET: &str = "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q";

    fn signer_key() -> PublicKey {
        let secret: near_api::SecretKey = SIGNER_SECRET.parse().expect("valid signer secret");
        PublicKey::from(secret.public_key())
    }

    fn args(no_signer: bool, extra: &[&str]) -> FullAccessKeyArgs {
        FullAccessKeyArgs {
            no_signer_full_access_key: no_signer,
            with_full_access_key: extra
                .iter()
                .map(|key| key.parse().expect("valid ed25519 public key"))
                .collect(),
        }
    }

    #[test]
    fn grants_signer_key_by_default() {
        // The signer's key is granted on every deploy — the operator must retain
        // control of the new account.
        let signer = signer_key();
        assert_eq!(args(false, &[]).resolve(signer.clone()), vec![signer]);
    }

    #[test]
    fn no_signer_flag_drops_signer_key() {
        assert!(args(true, &[]).resolve(signer_key()).is_empty());
        // With extras and --no-signer, only the extras are granted.
        let key_b = PublicKey::from(KEY_B.parse::<CliPublicKey>().unwrap());
        assert_eq!(args(true, &[KEY_B]).resolve(signer_key()), vec![key_b]);
    }

    #[test]
    fn appends_and_dedups_extra_keys() {
        let signer = signer_key();
        let signer_str = signer.0.to_string();
        // Extras that repeat the signer key or each other are dropped.
        let keys = args(false, &[&signer_str, KEY_B, KEY_B]).resolve(signer.clone());
        let key_b = PublicKey::from(KEY_B.parse::<CliPublicKey>().unwrap());
        assert_eq!(keys, vec![signer, key_b]);
    }
}
