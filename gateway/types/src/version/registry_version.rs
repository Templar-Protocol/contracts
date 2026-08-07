use templar_common::registry::DeployMode;

pub struct Registry;
pub type RegistryVersion = super::Version<Registry>;

impl RegistryVersion {
    pub fn supports_global_contracts(self) -> bool {
        self >= (1, 1, 0)
    }

    /// Whether `get_version` and `get_registry_entry` exist.
    ///
    /// Without them a reader can only test membership, which cannot see a soft-deleted version or
    /// a reserved name. Every registry deployed so far predates them — the live ones run 0.1.0,
    /// 1.0.0 and 1.1.0 — so a caller must degrade to the older checks rather than fail closed.
    pub fn supports_entry_and_version_views(self) -> bool {
        self >= (1, 3, 0)
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
            borsh::to_vec(&(version_key, deploy_mode, wasm))
        } else {
            borsh::to_vec(&(version_key, wasm))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RegistryVersion;

    /// The three releases actually deployed — `templar-alpha.near`, `v1.tmplr.near` and
    /// `user0.tmplr.near` — must all read as unsupported, or every one of them fails preflight
    /// instead of degrading to the checks they can answer.
    #[rstest::rstest]
    #[case::alpha_near((0, 1, 0), false)]
    #[case::v1_tmplr_near((1, 0, 0), false)]
    #[case::user0_tmplr_near((1, 1, 0), false)]
    #[case((1, 2, 4), false)]
    #[case((1, 3, 0), true)]
    #[case((2, 0, 0), true)]
    fn entry_and_version_views_from_1_3_0(
        #[case] version: (u64, u64, u64),
        #[case] expected: bool,
    ) {
        assert_eq!(
            RegistryVersion::from(version).supports_entry_and_version_views(),
            expected,
        );
    }
}
