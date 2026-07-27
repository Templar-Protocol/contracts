#![allow(clippy::expect_used, clippy::unwrap_used)]

pub mod bench;
pub mod controller;
pub mod ops;
pub mod sandbox;
mod sandbox_ext;
pub mod wasm;

pub use controller::TestController;
pub use ops::{failed_receipts, DeployedMarket, DeployedVault};
pub use sandbox::{sandbox_config, test_secret_key, test_signer, SandboxHarness};
pub use sandbox_ext::node_is_serving;
pub use templar_gateway_types::ManagedAccountId;
pub use test_utils::test_signer::TestSigner;

/// Sandbox transactions wait only for complete optimistic execution, and all
/// sandbox reads use the matching optimistic state.
pub const TEST_FINALITY_POLICY: templar_gateway_core::FinalityPolicy =
    templar_gateway_core::FinalityPolicy::ExecutedOptimistic;

/// An [`rstest`] fixture yielding a started [`SandboxHarness`], so tests keep
/// the familiar `#[rstest] ... #[future(awt)] harness: SandboxHarness` shape.
///
/// The harness connects via [`SandboxHarness::start`], so the same fixture
/// attaches to an out-of-band `neard` (when `NEAR_SANDBOX_RPC_URL` is set) or
/// launches an owned one — no test changes either way.
#[rstest::fixture]
pub async fn harness() -> SandboxHarness {
    SandboxHarness::start()
        .await
        .expect("failed to start sandbox harness")
}

/// Like [`harness()`], but always on a dedicated `neard`. Use only when the suite
/// genuinely cannot share a pooled node — see [`SandboxHarness::start_owned`]
/// for the one situation that requires it. It costs a node boot per test.
#[rstest::fixture]
pub async fn owned_harness() -> SandboxHarness {
    SandboxHarness::start_owned()
        .await
        .expect("failed to start owned sandbox harness")
}
