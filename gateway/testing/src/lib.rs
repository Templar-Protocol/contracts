#![allow(clippy::expect_used, clippy::unwrap_used)]

pub mod controller;
pub mod ops;
pub mod sandbox;

pub use controller::TestController;
pub use ops::DeployedMarket;
pub use sandbox::SandboxHarness;
pub use test_utils::test_signer::TestSigner;

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
