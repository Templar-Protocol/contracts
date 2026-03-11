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
}
