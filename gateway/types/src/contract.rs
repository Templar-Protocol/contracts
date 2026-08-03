use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a governance proposal's arguments reach the contract.
///
/// Borsh is cheaper and carries larger payloads, but leaves an operation explorers and indexers
/// cannot read — so it is opt-in, for proposals big enough to need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[cfg_attr(feature = "clap", value(rename_all = "snake_case"))]
pub enum ProposalEncoding {
    #[default]
    Json,
    Borsh,
}

impl ProposalEncoding {
    /// Serialization skips the default so a request that does not opt in is byte-identical to one
    /// written before this field existed — which keeps its idempotency fingerprint stable.
    #[must_use]
    pub fn is_json(&self) -> bool {
        matches!(self, Self::Json)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[cfg_attr(feature = "clap", value(rename_all = "snake_case"))]
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
    /// Pyth Lazer adapter. Answers the classic Pyth *view* ABI, so it is
    /// detected separately (and before `PythOracle`) via its Lazer feed-mapping views;
    /// its *write* ABI takes a base64 Lazer `payload` rather than a hex Pyth `data`.
    PythLazerOracle,
}

/// How the gateway resolves a price update/read for an oracle contract — the refined,
/// resolution-facing view of an oracle's [`ContractKind`]. A Pyth Lazer adapter is
/// deliberately absent: it is not a standalone oracle. It is used only as a proxy `Lazer`
/// source, so it never surfaces here as a resolvable top-level oracle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OracleContractKind {
    /// A classic Pyth (or RedStone) oracle updated with its native payload.
    Direct,
    /// A liquid-staking-token oracle wrapping an underlying Pyth oracle.
    Lst { pyth_id: near_account_id::AccountId },
    /// A proxy oracle that fans out to, and re-aggregates, underlying sources.
    Proxy,
}
