use tokio::sync::OnceCell;

use crate::get_contract;

/// Test controller for the Pyth Lazer adapter contract
/// (`contract/pyth-lazer/contract`). The gateway sandbox harness deploys it via its own
/// `deploy_contract`, so this only needs to supply the compiled wasm.
pub struct PythLazerAdapterController;

impl PythLazerAdapterController {
    pub async fn wasm() -> &'static [u8] {
        static WASM: OnceCell<Vec<u8>> = OnceCell::const_new();

        WASM.get_or_init(|| {
            get_contract(
                "templar_pyth_lazer_adapter_contract",
                "contract/pyth-lazer/contract",
            )
        })
        .await
    }
}
