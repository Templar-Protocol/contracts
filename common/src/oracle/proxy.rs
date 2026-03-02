use std::ops::Deref;

use near_sdk::{near, AccountId, BorshStorageKey};

use super::{price_transformer::ProxyPriceTransformer, pyth::PriceIdentifier, OracleRequest};

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
    Request(OracleRequest),
    Transformer(ProxyPriceTransformer),
}

impl From<ProxyPriceTransformer> for ProxyEntry {
    fn from(transformer: ProxyPriceTransformer) -> Self {
        Self::Transformer(transformer)
    }
}

impl From<OracleRequest> for ProxyEntry {
    fn from(oracle_price: OracleRequest) -> Self {
        Self::Request(oracle_price)
    }
}

#[derive(Debug, Clone, BorshStorageKey)]
#[near(serializers = [json, borsh])]
pub enum Role {
    ModifyRole,
    AddProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub enum Oracle {
    Pyth,
    RedStone,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub enum OracleType {
    Pyth(AccountId),
    RedStone(AccountId),
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
    AddProxy { id: PriceIdentifier, proxy: Proxy },
}
