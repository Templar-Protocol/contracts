use std::ops::Deref;

use near_sdk::{near, AccountId, BorshStorageKey};

use super::{price_transformer::ProxyPriceTransformer, pyth::PriceIdentifier};

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Proxy(pub Vec<ProxyEntry>);

impl Proxy {
    /// Calculates a deterministic ID for this proxy.
    ///
    /// # Errors
    ///
    /// - Borsh encoding fails.
    pub fn id(&self) -> Result<PriceIdentifier, std::io::Error> {
        let encoding = near_sdk::borsh::to_vec(self)?;
        let hash;
        #[cfg(target_arch = "wasm32")]
        {
            hash = near_sdk::env::sha256_array(&encoding);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            hash = <sha2::Sha256 as sha2::Digest>::digest(encoding).into();
        }
        Ok(PriceIdentifier(hash))
    }
}

impl Deref for Proxy {
    type Target = [ProxyEntry];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Vec<ProxyEntry>> for Proxy {
    fn from(proxies: Vec<ProxyEntry>) -> Self {
        Self(proxies)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum ProxyEntry {
    Transformer(ProxyPriceTransformer),
    Pyth(super::pyth::PriceIdentifier),
    #[cfg(feature = "redstone")]
    RedStone(super::redstone::FeedId),
}

#[derive(Debug, Clone, BorshStorageKey)]
#[near(serializers = [json, borsh])]
pub enum Role {
    ModifyRole,
    SetOracleId,
    AddProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub enum Oracle {
    Pyth,
    #[cfg(feature = "redstone")]
    RedStone,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct OracleIds {
    pub pyth_id: AccountId,
    #[cfg(feature = "redstone")]
    pub redstone_id: AccountId,
}

#[near(event_json(standard = "templar-proxy-oracle"))]
pub enum ProxyOracleEvent {
    #[event_version("1.0.0")]
    ModifyRole {
        account_id: AccountId,
        role: Role,
        set: bool,
    },
    #[event_version("1.0.0")]
    SetOracleId {
        oracle: Oracle,
        oracle_id: AccountId,
    },
    #[event_version("1.0.0")]
    AddProxy { id: PriceIdentifier, proxy: Proxy },
}
