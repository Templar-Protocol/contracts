pub mod account;
pub mod contract;
pub mod deploy_common;
pub mod duration;
pub mod ft;
pub mod full_access_key;
pub mod market;
pub mod mpc;
pub mod oracle;
pub mod owner;
pub mod pagination;
pub mod patch;
pub mod proxy_oracle;
pub mod pyth;
pub mod recover;
pub mod redstone;
pub mod registry;
pub mod signer;
pub mod spec;
pub mod storage;

/// Read and parse a JSON file named by a `--*-file` flag; `-` reads stdin.
pub fn load_json_file<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> anyhow::Result<T> {
    use anyhow::Context as _;

    if path == std::path::Path::new("-") {
        return serde_json::from_reader(std::io::stdin().lock()).context("parse JSON from stdin");
    }
    let contents =
        std::fs::read(path).with_context(|| format!("read JSON from {}", path.display()))?;
    serde_json::from_slice(&contents).with_context(|| format!("parse JSON from {}", path.display()))
}

/// Write `bytes` to `path` through a fresh sibling temp file, fsync, and
/// rename, so a crash mid-write cannot leave a truncated file where a reader
/// will look, and nothing that already exists is truncated on the way. The
/// directory fsync is best-effort: some filesystems refuse it, and by then the
/// file is already in place.
pub fn write_atomically(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::io::Write as _;

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("{} is not a file path", path.display()))?;
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("flush {}", temporary.display()))?;
    }
    std::fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

pub use account::AccountNs;
pub use contract::ContractNs;
pub use ft::FtNs;
pub use market::MarketNs;
pub use mpc::MpcNs;
pub use oracle::OracleNs;
pub use owner::OwnerNs;
pub use patch::PatchNs;
pub use proxy_oracle::ProxyOracleNs;
pub use pyth::PythNs;
pub use recover::RecoverNep141;
pub use redstone::RedstoneNs;
pub use registry::RegistryNs;
pub use spec::SpecNs;
pub use storage::StorageNs;

use anyhow::Context as _;
use std::collections::BTreeSet;
use std::path::PathBuf;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_types::Base64Bytes;

/// Drop repeats from a repeatable `--price-id`, which would otherwise widen a Hermes
/// query or resolve the same dependency twice.
pub(crate) fn dedup_price_ids(price_ids: Vec<PriceIdentifier>) -> Vec<PriceIdentifier> {
    price_ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Resolve a base64-encoded binary argument supplied either inline or by file path.
/// `what` names the payload in error messages (e.g. "Pyth update data").
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
