use anyhow::{bail, Context, Result};
use serde::Serialize;
use templar_common::asset::{BorrowAsset, FungibleAsset};

/// Curated identities from deployments/profiles and deployments/alpha. An alias
/// is an explicit set, not a claim that differently wrapped assets are fungible.
#[derive(Debug, Serialize)]
pub(crate) struct AssetAlias {
    pub name: &'static str,
    pub members: &'static [&'static str],
}

const ALIASES: &[AssetAlias] = &[
    AssetAlias { name: "ADA", members: &["nep245:intents.near:nep141:cardano.omft.near"] },
    AssetAlias { name: "BTC", members: &["nep245:intents.near:nep141:btc.omft.near"] },
    AssetAlias { name: "CETES", members: &["nep245:intents.near:nep245:v2_1.omni.hot.tg:1100_111bzQBB5uBD3Wrr7pthp8XhJsreEcwTVnmjQ1wpbzkvHLEQf3ygS"] },
    AssetAlias { name: "DEJAAA", members: &["nep245:intents.near:nep245:v2_1.omni.hot.tg:1100_111bzQBB66Lr9d7WU1sDna78SqG5x1ZraFjkpPdiYXjHFRnZJUhuV"] },
    AssetAlias { name: "DEJTRSY", members: &["nep245:intents.near:nep245:v2_1.omni.hot.tg:1100_111bzQBB5y5yhcUCbDKaCx4zNjEHQbwLAdvwucCecVzC5Ub7uNKEb"] },
    AssetAlias { name: "DOGE", members: &["nep245:intents.near:nep141:doge.omft.near"] },
    AssetAlias { name: "ENA", members: &["nep245:intents.near:nep141:eth-0x57e114b691db790c35207b2e685d4a43181e6061.omft.near"] },
    AssetAlias { name: "FXRP", members: &["nep245:intents.near:nep141:eth-0xce6170ea245dc8d1f275a710a062b70f125f0110.omft.near"] },
    AssetAlias { name: "hemiBTC", members: &["nep245:intents.near:nep141:eth-0x06ea695b91700071b161a434fed42d1dcbad9f00.omft.near"] },
    AssetAlias { name: "LTC", members: &["nep245:intents.near:nep141:ltc.omft.near"] },
    AssetAlias { name: "PYUSD", members: &["nep245:intents.near:nep245:v2_1.omni.hot.tg:1100_111bzQBB62XZkuam1hPr5wsG54FvwhYaPvecKwgZo1ZoKMWEXcE2n"] },
    AssetAlias { name: "SolvBTC", members: &["nep245:intents.near:nep245:v2_1.omni.hot.tg:1100_111bzQBB5xzU1EsXby4ckez2qjWFTBiPoqHzZpPkq1Gr9gB7FQpeZ"] },
    AssetAlias { name: "stNEAR", members: &["nep141:meta-pool.near"] },
    AssetAlias { name: "sUSDe", members: &["nep245:intents.near:nep141:eth-0x9d39a5de30e57443bff2a8307a4256c8797a3497.omft.near"] },
    AssetAlias { name: "USDC", members: &[
        "nep141:17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1",
        "nep245:intents.near:nep141:eth-0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48.omft.near",
        "nep245:intents.near:nep141:sol-5ce3bf3a31af18be40ba30f721101b4341690186.omft.near",
        "nep245:intents.near:nep245:v2_1.omni.hot.tg:1100_111bzQBB65GxAPAVoxqmMcgYo5oS3txhqs1Uh1cgahKQUeTUq1TJu",
    ] },
    AssetAlias { name: "USDe", members: &["nep245:intents.near:nep141:eth-0x4c9edd5852cd905f086c759e8383e09bff1e68b3.omft.near"] },
    AssetAlias { name: "USTRY", members: &["nep245:intents.near:nep245:v2_1.omni.hot.tg:1100_111bzQBB5yT2A5maKJqJQsuNg7BA6VG4S4ZATpqmKYLwYBsfEfh6e"] },
    AssetAlias { name: "WBTC", members: &["nep245:intents.near:nep141:eth-0x2260fac5e5542a773aa44fbcfedf7c193bc2c599.omft.near"] },
    AssetAlias { name: "XLM", members: &["nep245:intents.near:nep245:v2_1.omni.hot.tg:1100_111bzQBB5v7AhLyPMDwS8uJgQV24KaAPXtwyVWu2KXbbfQU6NXRCz"] },
    AssetAlias { name: "XRP", members: &["nep245:intents.near:nep141:xrp.omft.near"] },
    AssetAlias { name: "ZEC", members: &["nep245:intents.near:nep141:zec.omft.near"] },
];

pub(crate) fn catalog() -> &'static [AssetAlias] {
    ALIASES
}

/// Parse the complete canonical identity, retaining nested NEP-245 token IDs.
pub(crate) fn canonical_id(text: &str) -> Result<String> {
    text.parse::<FungibleAsset<BorrowAsset>>()
        .map(|asset| asset.to_string())
        .with_context(|| format!("invalid canonical asset identity {text:?}"))
}

#[derive(Debug, Clone)]
pub(crate) enum AssetSelector {
    Exact(String),
    Alias(&'static AssetAlias),
}

impl AssetSelector {
    pub(crate) fn matches(&self, canonical: &str) -> bool {
        match self {
            Self::Exact(expected) => expected == canonical,
            Self::Alias(alias) => alias.members.contains(&canonical),
        }
    }
}

pub(crate) fn resolve(text: &str) -> Result<AssetSelector> {
    if text.contains(':') {
        return canonical_id(text).map(AssetSelector::Exact);
    }
    if let Some(alias) = ALIASES
        .iter()
        .find(|alias| alias.name.eq_ignore_ascii_case(text))
    {
        return Ok(AssetSelector::Alias(alias));
    }
    bail!("unknown asset alias {text:?}; use query assets or an exact nep141:/nep245: identity")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_identity_is_valid_and_unchanged() {
        for alias in catalog() {
            for member in alias.members {
                assert_eq!(canonical_id(member).unwrap(), *member);
                assert!(resolve(alias.name).unwrap().matches(member));
            }
        }
    }

    #[test]
    fn wrappers_are_distinct_exact_identities() {
        for name in ["BTC", "WBTC", "hemiBTC", "SolvBTC"] {
            let selector = resolve(name).unwrap();
            for alias in catalog()
                .iter()
                .filter(|alias| ["BTC", "WBTC", "hemiBTC", "SolvBTC"].contains(&alias.name))
            {
                assert_eq!(selector.matches(alias.members[0]), alias.name == name);
            }
        }
        assert!(!resolve("BTC").unwrap().matches("nep141:btc.omft.near"));
    }

    #[test]
    fn aliases_are_case_insensitive_but_ids_remain_exact() {
        let usdc = resolve("usdc").unwrap();
        let members = catalog()
            .iter()
            .find(|alias| alias.name == "USDC")
            .unwrap()
            .members;
        assert!(members.iter().all(|member| usdc.matches(member)));
        let exact = resolve(members[0]).unwrap();
        assert!(!exact.matches(members[1]));
        assert!(resolve("UNKNOWN").is_err());
    }

    #[test]
    fn nested_token_ids_are_not_split_or_rewritten() {
        let identity = "nep245:intents.near:nep245:v2_1.omni.hot.tg:Token:with:colons";
        assert_eq!(canonical_id(identity).unwrap(), identity);
        assert!(!resolve(identity)
            .unwrap()
            .matches(&identity.to_ascii_lowercase()));
        for invalid in [
            "btc.omft.near",
            "nep141:",
            "nep141:UPPER.near",
            "nep245:intents.near:",
            "nep141:btc.omft.near:extra",
        ] {
            assert!(canonical_id(invalid).is_err(), "{invalid}");
        }
    }
}
