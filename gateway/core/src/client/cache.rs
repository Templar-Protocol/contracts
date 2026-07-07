use std::{future::Future, hash::Hash, sync::Arc, time::Duration};

use moka::sync::Cache;

use crate::GatewayResult;

use super::{
    contract::ContractClientCaches, lst_oracle::LstOracleClientCaches, market::MarketClientCaches,
    proxy_oracle::ProxyOracleClientCaches, storage::StorageClientCaches, vault::VaultClientCaches,
};

#[derive(Clone)]
pub(crate) struct NearClientCache {
    pub(crate) contract: ContractClientCaches,
    pub(crate) storage: StorageClientCaches,
    pub(crate) market: MarketClientCaches,
    pub(crate) vault: VaultClientCaches,
    pub(crate) lst_oracle: LstOracleClientCaches,
    pub(crate) proxy_oracle: ProxyOracleClientCaches,
}

impl NearClientCache {
    pub(crate) fn new() -> Self {
        Self {
            contract: ContractClientCaches::new(),
            storage: StorageClientCaches::new(),
            market: MarketClientCaches::new(),
            vault: VaultClientCaches::new(),
            lst_oracle: LstOracleClientCaches::new(),
            proxy_oracle: ProxyOracleClientCaches::new(),
        }
    }
}

pub(crate) const IMMUTABLE_CACHE_TTL: Duration = Duration::from_secs(60 * 60);
pub(crate) const CONFIG_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

pub(crate) fn immutable_cache<K, V>(capacity: u64) -> Cache<K, Arc<V>>
where
    K: Eq + Hash + Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    Cache::builder()
        .max_capacity(capacity)
        .time_to_live(IMMUTABLE_CACHE_TTL)
        .build()
}

pub(crate) fn config_cache<K, V>(capacity: u64) -> Cache<K, Arc<V>>
where
    K: Eq + Hash + Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    Cache::builder()
        .max_capacity(capacity)
        .time_to_live(CONFIG_CACHE_TTL)
        .build()
}

pub(crate) async fn load_cached<K, V, F, Fut>(
    cache: &Cache<K, Arc<V>>,
    key: K,
    load: F,
) -> GatewayResult<V>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = GatewayResult<V>> + Send + 'static,
{
    if let Some(value) = cache.get(&key) {
        return Ok((*value).clone());
    }

    let value = Arc::new(load().await?);
    let copied = (*value).clone();
    cache.insert(key, value);
    Ok(copied)
}

pub(crate) fn is_method_not_found(error: &crate::GatewayError) -> bool {
    matches!(error, crate::GatewayError::NearQuery(message) if message.contains("MethodNotFound"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn load_cached_reuses_existing_value() {
        let cache = Cache::builder().max_capacity(16).build();
        let load_count = Arc::new(AtomicUsize::new(0));

        let first = load_cached(&cache, "key", {
            let load_count = Arc::clone(&load_count);
            move || async move {
                load_count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, crate::GatewayError>(41_u32)
            }
        })
        .await
        .unwrap();

        let second = load_cached(&cache, "key", {
            let load_count = Arc::clone(&load_count);
            move || async move {
                load_count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, crate::GatewayError>(99_u32)
            }
        })
        .await
        .unwrap();

        assert_eq!(first, 41);
        assert_eq!(second, 41);
        assert_eq!(load_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn load_cached_reuses_none_values() {
        let cache = Cache::builder().max_capacity(16).build();
        let load_count = Arc::new(AtomicUsize::new(0));

        let first = load_cached(&cache, "missing", {
            let load_count = Arc::clone(&load_count);
            move || async move {
                load_count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, crate::GatewayError>(Option::<u32>::None)
            }
        })
        .await
        .unwrap();

        let second = load_cached(&cache, "missing", {
            let load_count = Arc::clone(&load_count);
            move || async move {
                load_count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, crate::GatewayError>(Some(7_u32))
            }
        })
        .await
        .unwrap();

        assert_eq!(first, None);
        assert_eq!(second, None);
        assert_eq!(load_count.load(Ordering::SeqCst), 1);
    }
}
