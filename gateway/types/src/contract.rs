use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContractKind {
    Unknown,
    Registry,
    Market,
    Vault,
    ProxyOracle,
    ProxyGovernance,
    LstOracle,
    UniversalAccount,
    RedstoneOracle,
    PythOracle,
    /// Pyth Pro (Pyth Lazer) adapter. Answers the classic Pyth *view* ABI, so it is
    /// detected separately (and before `PythOracle`) via its Lazer feed-mapping views;
    /// its *write* ABI takes a base64 Lazer `payload` rather than a hex Pyth `data`.
    PythProOracle,
}

/// How the gateway resolves a price update/read for an oracle contract — the refined,
/// resolution-facing view of an oracle's [`ContractKind`]. Reads treat `Direct` and
/// `PythPro` alike (both serve the classic Pyth view ABI); writes/dependencies do not
/// (a `PythPro` adapter is updated with a Lazer payload, not a Pyth VAA).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OracleContractKind {
    /// A classic Pyth (or RedStone) oracle updated with its native payload.
    Direct,
    /// A Pyth Pro (Pyth Lazer) adapter: reads like Pyth, but resolves to a Lazer feed.
    PythPro,
    /// A liquid-staking-token oracle wrapping an underlying Pyth oracle.
    Lst { pyth_id: near_account_id::AccountId },
    /// A proxy oracle that fans out to, and re-aggregates, underlying sources.
    Proxy,
}
