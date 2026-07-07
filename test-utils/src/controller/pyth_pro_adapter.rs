use tokio::sync::OnceCell;

use crate::{get_contract, ArtifactId};

/// Test controller for the Pyth Pro (Pyth Lazer) adapter contract
/// (`contract/pyth-pro/contract`). The gateway sandbox harness deploys it via its own
/// `deploy_contract`, so this only needs to supply the compiled wasm.
pub struct PythProAdapterController;

impl PythProAdapterController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        WASM.get_or_init(|| get_contract(ArtifactId::PythProAdapter))
            .await
    }
}
