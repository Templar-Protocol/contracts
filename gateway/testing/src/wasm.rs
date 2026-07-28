//! Contract WASM loading for the sandbox harness.
//!
//! Each helper yields the bytes of a workspace contract, either read from a
//! prebuilt artifact (when `TEST_CONTRACTS_PREBUILT` is set) or freshly built
//! via `cargo near build`. Bytes are cached per-artifact for the process.
//!
//! [`released`] is the other source: the exact bytes a *past* version shipped
//! as, downloaded from that version's GitHub Release rather than built here.

use std::path::Path;

use serde::{Deserialize, Serialize};
use templar_contract_artifacts::{build_artifact, load_artifact_bytes, ArtifactId};
use tokio::sync::OnceCell;

/// Deploy-argument shape for the mock Ref Finance exchange's `new` call and
/// `get_pools` view (mirrors `mock-ref`'s `PoolInfo`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolInfo {
    pub token_account_ids: Vec<near_api::types::AccountId>,
    pub shares_total_supply: near_sdk::json_types::U128,
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_WORKSPACE_DIR"))
}

/// Load `artifact`'s WASM bytes: read the prebuilt artifact when
/// `TEST_CONTRACTS_PREBUILT` is set, otherwise build it fresh.
fn get(artifact: ArtifactId) -> Vec<u8> {
    let metadata = artifact.metadata();
    if std::env::var("TEST_CONTRACTS_PREBUILT").is_ok() {
        load_artifact_bytes(workspace_root(), metadata)
            .expect("failed to read prebuilt contract artifact")
    } else {
        build_artifact(workspace_root(), metadata.package_name, false)
            .expect("failed to build contract artifact")
            .0
    }
}

macro_rules! wasm_fns {
    ($($(#[$meta:meta])* $name:ident => $artifact:ident),* $(,)?) => {
        $(
            $(#[$meta])*
            pub async fn $name() -> &'static [u8] {
                static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();
                WASM.get_or_init(|| async { get(ArtifactId::$artifact) })
                    .await
                    .as_slice()
            }
        )*
    };
}

wasm_fns! {
    market => Market,
    ft => MockFt,
    mt => MockMt,
    mock_oracle => MockOracle,
    registry => Registry,
    universal_account => UniversalAccount,
    proxy_oracle => ProxyOracle,
    proxy_governance => ProxyGovernance,
    redstone_adapter => RedstoneAdapter,
    lst_oracle => LstOracle,
    receiver => MockReceiver,
    ref_finance => MockRefFinance,
    pyth_lazer_adapter => PythLazerAdapter,
    vault => Vault,
}

/// Bytes of a specific *released* version of a contract, for migration and
/// upgrade tests that must deploy the real historical binary.
///
/// Downloaded from that version's GitHub Release into a shared on-disk cache
/// and verified against the catalog's SHA-256 pin, so a swapped asset fails here
/// rather than silently changing what a migration test deploys. A cold cache
/// needs network; `just artifacts-fetch` warms it (CI does this first).
///
/// # Panics
/// If `version` is not a catalogued release of `artifact` (a test bug — the
/// available versions are in `contract/artifacts/releases/`), or if the bytes
/// can be neither found in the cache nor downloaded.
pub async fn released(artifact: ArtifactId, version: &str) -> Vec<u8> {
    templar_contract_artifacts::fetch::released_bytes(artifact, version)
        .await
        .unwrap_or_else(|error| panic!("could not load {artifact}@{version}: {error}"))
}
