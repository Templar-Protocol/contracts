use near_sdk::{near, AccountId, BorshStorageKey};

use super::{price_transformer::ProxyPriceTransformer, pyth::PriceIdentifier, OraclePriceId};

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Proxy {
    Transformer(ProxyPriceTransformer),
    List(Vec<OraclePriceId>),
}

impl Proxy {
    pub fn list(list: impl IntoIterator<Item = OraclePriceId>) -> Self {
        Self::List(list.into_iter().collect())
    }

    /// Calculates a deterministic ID for this proxy.
    ///
    /// # Errors
    ///
    /// - Borsh encoding fails.
    #[cfg(feature = "redstone")]
    pub fn id(&self) -> Result<PriceIdentifier, std::io::Error> {
        let encoding = near_sdk::borsh::to_vec(self)?;
        let hash;
        #[cfg(target_family = "wasm")]
        {
            hash = near_sdk::env::sha256_array(&encoding);
        }
        #[cfg(not(target_family = "wasm"))]
        {
            use k256::sha2::Digest;
            hash = k256::sha2::Sha256::digest(encoding).into();
        }
        Ok(PriceIdentifier(hash))
    }
}

#[derive(Debug, Clone, BorshStorageKey)]
#[near(serializers = [json, borsh])]
pub enum Role {
    ModifyRole,
    SetOracleId,
    AddProxy,
    Upgrade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub enum Oracle {
    Pyth,
    RedStone,
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
