//! Signing keys held in the OS keychain, under the entries near-cli-rs writes.
//!
//! near-api's `KeystoreSigner` reads the keychain once per key while searching
//! and again on every signature, each a potential OS prompt. This reads one
//! secret, once, and leaves signing to an in-memory signer.

use anyhow::Context as _;
use near_account_id::AccountId;
use near_api::{signer::AccountKeyPair, NetworkConfig, PublicKey, SecretKey};
use templar_gateway_client::Client;
use templar_gateway_methods_spec::account::{AccessKeyPermission, ListAccessKeys};

/// The secret of the first of `account_id`'s full access keys, in on-chain
/// order, that the keychain holds for `network`.
pub(crate) async fn find_secret_key(
    account_id: &AccountId,
    network: &NetworkConfig,
) -> anyhow::Result<SecretKey> {
    let keys = Client::builder(network.clone())
        .build()
        .context("build a gateway client to list access keys")?
        .read(ListAccessKeys {
            account_id: account_id.clone(),
        })
        .await
        .with_context(|| format!("list {account_id}'s access keys"))?;
    let full_access_keys = keys
        .into_iter()
        .filter(|key| key.permission == AccessKeyPermission::FullAccess)
        .map(|key| PublicKey::from(key.public_key))
        .collect::<Vec<_>>();

    for public_key in &full_access_keys {
        if let Some(secret_key) =
            read_secret_key(&network.network_name, account_id, *public_key).await?
        {
            return Ok(secret_key);
        }
    }
    anyhow::bail!(
        "the keychain holds none of {account_id}'s {} full access key(s) for {}",
        full_access_keys.len(),
        network.network_name,
    )
}

/// The near-cli-rs entry for one key. near-api reads the same entries; the
/// tests pin the two together.
fn entry(
    network_name: &str,
    account_id: &AccountId,
    public_key: PublicKey,
) -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(
        &format!("near-{network_name}-{account_id}"),
        &format!("{account_id}:{public_key}"),
    )
}

/// `None` when no entry exists; any other keychain failure, such as a denied
/// prompt, is an error rather than a miss.
async fn read_secret_key(
    network_name: &str,
    account_id: &AccountId,
    public_key: PublicKey,
) -> anyhow::Result<Option<SecretKey>> {
    let entry = entry(network_name, account_id, public_key)
        .with_context(|| format!("open the keychain entry for {account_id}:{public_key}"))?;
    // Blocks on the OS prompt when the keychain is locked.
    let stored = tokio::task::spawn_blocking(move || entry.get_password())
        .await
        .context("join the keychain read")?;
    let stored = match stored {
        Ok(stored) => stored,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read the keychain entry for {account_id}:{public_key}"))
        }
    };

    // The serde error may quote the stored text, so it never reaches the message.
    let pair: AccountKeyPair = serde_json::from_str(&stored).map_err(|_| {
        anyhow::anyhow!("the keychain entry for {account_id}:{public_key} is not a NEAR key pair")
    })?;
    anyhow::ensure!(
        pair.private_key.public_key() == public_key,
        "the keychain entry for {account_id}:{public_key} holds a different key",
    );
    Ok(Some(pair.private_key))
}

#[cfg(test)]
mod tests {
    use std::{
        any::Any,
        collections::HashMap,
        sync::{Arc, Mutex, Once},
    };

    use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi};
    use near_api::signer::{keystore::KeystoreSigner, SignerTrait as _};

    use super::*;

    type Entries = Arc<Mutex<HashMap<(String, String), Vec<u8>>>>;

    /// keyring's own mock gives every `Entry` private storage, so nothing
    /// written through one entry is visible to another of the same name.
    #[derive(Default)]
    struct SharedStore(Entries);

    struct SharedCredential {
        entries: Entries,
        name: (String, String),
    }

    impl CredentialBuilderApi for SharedStore {
        fn build(
            &self,
            _target: Option<&str>,
            service: &str,
            user: &str,
        ) -> keyring::Result<Box<Credential>> {
            Ok(Box::new(SharedCredential {
                entries: Arc::clone(&self.0),
                name: (service.to_owned(), user.to_owned()),
            }))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    impl CredentialApi for SharedCredential {
        fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
            self.entries
                .lock()
                .expect("store lock")
                .insert(self.name.clone(), secret.to_vec());
            Ok(())
        }

        fn get_secret(&self) -> keyring::Result<Vec<u8>> {
            self.entries
                .lock()
                .expect("store lock")
                .get(&self.name)
                .cloned()
                .ok_or(keyring::Error::NoEntry)
        }

        fn delete_credential(&self) -> keyring::Result<()> {
            self.entries
                .lock()
                .expect("store lock")
                .remove(&self.name)
                .map(drop)
                .ok_or(keyring::Error::NoEntry)
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// Process-wide, so each test stores under its own account id.
    fn use_shared_store() {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            keyring::set_default_credential_builder(Box::new(SharedStore::default()));
        });
    }

    fn generated_secret_key() -> SecretKey {
        near_api::signer::generate_secret_key().expect("generate a key")
    }

    fn store(network_name: &str, account_id: &AccountId, secret_key: &SecretKey) {
        let pair = AccountKeyPair {
            public_key: secret_key.public_key(),
            private_key: secret_key.clone(),
        };
        entry(network_name, account_id, secret_key.public_key())
            .expect("open entry")
            .set_password(&serde_json::to_string(&pair).expect("serialize key pair"))
            .expect("store key pair");
    }

    /// Breaks when near-api stops reading the entries this module reads, which
    /// would leave keys stored by near-cli-rs unreachable here.
    #[rstest::rstest]
    #[case::mainnet("mainnet")]
    #[case::testnet("testnet")]
    #[tokio::test]
    async fn near_api_reads_the_entry_this_module_reads(#[case] network_name: &str) {
        use_shared_store();
        let account_id: AccountId = format!("convention.{network_name}")
            .parse()
            .expect("valid account");
        let secret_key = generated_secret_key();
        store(network_name, &account_id, &secret_key);

        let read_by_near_api = KeystoreSigner::new_with_pubkey(secret_key.public_key())
            .get_secret_key(&account_id, secret_key.public_key())
            .await
            .expect("near-api finds the entry");

        assert_eq!(read_by_near_api, secret_key);
    }

    #[tokio::test]
    async fn reads_a_stored_key() {
        use_shared_store();
        let account_id: AccountId = "stored.testnet".parse().expect("valid account");
        let secret_key = generated_secret_key();
        store("testnet", &account_id, &secret_key);

        let read = read_secret_key("testnet", &account_id, secret_key.public_key())
            .await
            .expect("readable");

        assert_eq!(read, Some(secret_key));
    }

    #[tokio::test]
    async fn a_missing_entry_is_a_miss_not_an_error() {
        use_shared_store();
        let account_id: AccountId = "missing.testnet".parse().expect("valid account");
        let secret_key = generated_secret_key();
        store("mainnet", &account_id, &secret_key);

        let read = read_secret_key("testnet", &account_id, secret_key.public_key())
            .await
            .expect("a miss is not an error");

        assert_eq!(read, None, "another network's entry must not match");
    }

    #[tokio::test]
    async fn an_entry_holding_another_key_is_rejected() {
        use_shared_store();
        let account_id: AccountId = "mismatch.testnet".parse().expect("valid account");
        let expected = generated_secret_key();
        let held = AccountKeyPair {
            public_key: expected.public_key(),
            private_key: generated_secret_key(),
        };
        entry("testnet", &account_id, expected.public_key())
            .expect("open entry")
            .set_password(&serde_json::to_string(&held).expect("serialize key pair"))
            .expect("store key pair");

        let error = read_secret_key("testnet", &account_id, expected.public_key())
            .await
            .expect_err("signing with it would fail on chain")
            .to_string();

        assert!(error.contains("holds a different key"), "{error}");
    }

    #[tokio::test]
    async fn a_malformed_entry_is_not_echoed() {
        use_shared_store();
        let account_id: AccountId = "malformed.testnet".parse().expect("valid account");
        let public_key = generated_secret_key().public_key();
        let stored = r#"{"public_key":"x","private_key":"ed25519:leaked"}"#;
        entry("testnet", &account_id, public_key)
            .expect("open entry")
            .set_password(stored)
            .expect("store garbage");

        let error = format!(
            "{:#}",
            read_secret_key("testnet", &account_id, public_key)
                .await
                .expect_err("not a key pair")
        );

        assert!(error.contains("not a NEAR key pair"), "{error}");
        assert!(!error.contains("leaked"), "{error}");
    }
}
