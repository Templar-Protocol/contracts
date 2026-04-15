use std::sync::Arc;

#[derive(Clone)]
pub struct ManagedSigner {
    pub signer: Arc<near_api::Signer>,
    pub key_count: usize,
}

impl ManagedSigner {
    pub async fn new(secret_keys: impl IntoIterator<Item = near_api::SecretKey>) -> Option<Self> {
        let mut secret_keys = secret_keys.into_iter();
        let signer = near_api::Signer::from_secret_key(secret_keys.next()?).ok()?;
        let mut key_count = 1;

        for secret_key in secret_keys {
            signer.add_secret_key_to_pool(secret_key).await.ok()?;
            key_count += 1;
        }

        Some(Self { signer, key_count })
    }
}
