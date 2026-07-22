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

/// Legacy `0.2.0` universal-account WASM (pinned blob), for migration tests.
pub const UNIVERSAL_ACCOUNT_0_2_0: &[u8] = include_bytes!("wasm/uac_0_2_0.wasm");
/// Legacy `0.4.0` universal-account WASM (pinned blob), for migration tests.
pub const UNIVERSAL_ACCOUNT_0_4_0: &[u8] = include_bytes!("wasm/uac_0_4_0.wasm");
/// Legacy (`0.1.0`, pre-kernelization) proxy-oracle WASM (pinned blob).
pub const PROXY_ORACLE_V0: &[u8] = include_bytes!("wasm/proxy_oracle_v0.wasm");
/// Currently-deployed proxy-oracle WASM (`0.3.0`, on-chain state version 1), pinned from
/// `proxy-oracle-iethhemibtc-iethusdc.v1.tmplr.near`. The pre-standardized-upgrade blob, for
/// cross-version upgrade tests.
pub const PROXY_ORACLE_0_3_0: &[u8] = include_bytes!("wasm/proxy_oracle_0_3_0.wasm");
/// Currently-deployed proxy-oracle-governance WASM (`0.1.0`, no versioned state / no `migrate`),
/// pinned from `proxy-gov-iethhemibtc-iethusdc.v1.tmplr.near`. The pre-standardized-upgrade blob,
/// for cross-version upgrade tests.
pub const PROXY_GOVERNANCE_0_1_0: &[u8] = include_bytes!("wasm/proxy_governance_0_1_0.wasm");
