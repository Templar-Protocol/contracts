pub mod account;
pub mod contract;
pub mod deploy_common;
pub mod duration;
pub mod ft;
pub mod full_access_key;
pub mod market;
pub mod oracle;
pub mod owner;
pub mod pagination;
pub mod proxy_oracle;
pub mod pyth;
pub mod recover;
pub mod redstone;
pub mod registry;
pub mod signer;
pub mod storage;

pub use account::AccountNs;
pub use contract::ContractNs;
pub use ft::FtNs;
pub use market::MarketNs;
pub use oracle::OracleNs;
pub use owner::OwnerNs;
pub use proxy_oracle::ProxyOracleNs;
pub use pyth::PythNs;
pub use recover::RecoverNep141;
pub use redstone::RedstoneNs;
pub use registry::RegistryNs;
pub use storage::StorageNs;

use anyhow::Context as _;
use std::path::PathBuf;
use templar_gateway_types::Base64Bytes;

/// Resolve a base64-encoded binary argument supplied either inline or by file path.
/// `what` names the payload in error messages (e.g. "Pyth VAA").
///
/// Callers pair the two options in a required [`clap::ArgGroup`], so "neither" is a
/// parse error rather than a runtime one.
pub(crate) fn resolve_base64_arg(
    inline: Option<String>,
    file: Option<PathBuf>,
    what: &str,
) -> anyhow::Result<Base64Bytes> {
    let encoded = match (inline, file) {
        (Some(inline), _) => inline,
        (None, Some(path)) => std::fs::read_to_string(&path)
            .with_context(|| format!("read {what} from {}", path.display()))?,
        (None, None) => anyhow::bail!("missing {what}"),
    };

    // Decode via `Base64Bytes`' own base64 deserialization to avoid a bespoke decoder.
    serde_json::from_value(serde_json::Value::String(encoded.trim().to_owned()))
        .with_context(|| format!("invalid base64 {what}"))
}
