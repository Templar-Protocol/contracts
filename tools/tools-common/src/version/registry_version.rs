use templar_common::registry::DeployMode;

pub struct Registry;
pub type RegistryVersion = super::Version<Registry>;

impl RegistryVersion {
    pub fn supports_global_contracts(self) -> bool {
        self >= (1, 1, 0)
    }

    pub fn deploy_method_name(self) -> &'static str {
        if self >= (1, 1, 0) {
            "deploy"
        } else {
            "deploy_market"
        }
    }

    pub fn encode_add_version_args(
        &self,
        version_key: &str,
        deploy_mode: DeployMode,
        wasm: &[u8],
    ) -> std::io::Result<Vec<u8>> {
        if self.supports_global_contracts() {
            near_sdk::borsh::to_vec(&(version_key, deploy_mode, wasm))
        } else {
            near_sdk::borsh::to_vec(&(version_key, wasm))
        }
    }
}
