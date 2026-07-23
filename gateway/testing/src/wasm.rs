//! Contract WASM loading for the sandbox harness.
//!
//! Each helper yields the bytes of a workspace contract, either read from a
//! prebuilt artifact (when `TEST_CONTRACTS_PREBUILT` is set) or freshly built
//! via `cargo near build`. Bytes are cached per-artifact for the process. The
//! three legacy blobs (embedded `include_bytes!`) are exposed as consts for
//! migration tests.

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
/// Backed by the immutable release list in
/// [`templar_contract_artifacts`] — see `contract/artifacts/README.md`. These
/// blobs used to be hand-maintained `include_bytes!` consts in this module,
/// outside the catalog and outside the drift check; they are now catalogued
/// releases like any other, so a corrupted or silently-swapped historical blob
/// fails `embedded_drift_check`.
///
/// # Panics
/// If `version` is not a catalogued release of `artifact`. That is a test bug:
/// the available versions are listed in `contract/artifacts/src/ids.rs`.
pub fn released(artifact: ArtifactId, version: &str) -> &'static [u8] {
    artifact
        .embedded_bytes_for_version(version)
        .unwrap_or_else(|| {
            panic!(
                "{artifact}@{version} is not a catalogued release; \
                 see contract/artifacts/src/ids.rs",
            )
        })
}
