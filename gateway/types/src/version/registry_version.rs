use templar_common::registry::VersionSource;

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
        self >= (2, 0, 0)
    }

    pub fn deploy_method_name(self) -> &'static str {
        if self >= (1, 1, 0) {
            "deploy"
        } else {
            "deploy_market"
        }
    }

    /// Whether `add_version` accepts [`VersionSource::ExistingGlobal`] — pointing a version key at
    /// a global contract already on chain instead of paying to publish those bytes again.
    pub fn supports_existing_global(self) -> bool {
        self >= (2, 0, 0)
    }

    /// Lower a [`VersionSource`] onto the encoding the target registry actually parses.
    ///
    /// 1.1.0+ takes the tagged source as-is; its `Stored`/`PublishGlobal` bytes are identical to
    /// the `(version_key, DeployMode, code)` tuple those releases were built against. Pre-1.1.0
    /// predates the tag entirely and takes a bare `(version_key, code)`.
    pub fn encode_add_version_args(
        &self,
        version_key: &str,
        source: &VersionSource,
    ) -> std::io::Result<Vec<u8>> {
        if self.supports_global_contracts() {
            return borsh::to_vec(&(version_key, source));
        }

        match source {
            VersionSource::Stored(code) | VersionSource::PublishGlobal(code) => {
                borsh::to_vec(&(version_key, &code.0))
            }
            VersionSource::ExistingGlobal(_) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Registry version {self} has no add_version encoding for a code hash"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RegistryVersion;
    use near_sdk::json_types::{Base58CryptoHash, Base64VecU8};
    use templar_common::registry::{DeployMode, VersionSource};

    const KEY: &str = "market@1.5.0";
    const CODE: [u8; 3] = [0xde, 0xad, 0xbe];

    /// Every release before the one carrying `ExistingGlobal` must read as unsupported, so a
    /// caller is rejected up front rather than sending an encoding the registry cannot parse.
    #[rstest::rstest]
    #[case((0, 1, 0), false)]
    #[case((1, 1, 0), false)]
    #[case((1, 2, 4), false)]
    #[case((1, 3, 0), false)]
    #[case((2, 0, 0), true)]
    fn existing_global_from_2_0_0(#[case] version: (u64, u64, u64), #[case] expected: bool) {
        assert_eq!(
            RegistryVersion::from(version).supports_existing_global(),
            expected,
        );
    }

    /// The whole point of the fold: a 1.1.0+ registry sees exactly the bytes it saw before.
    #[rstest::rstest]
    #[case(DeployMode::Normal, VersionSource::Stored(Base64VecU8(CODE.to_vec())))]
    #[case(
        DeployMode::GlobalHash,
        VersionSource::PublishGlobal(Base64VecU8(CODE.to_vec()))
    )]
    fn encoding_is_unchanged_for_deployed_registries(
        #[case] mode: DeployMode,
        #[case] source: VersionSource,
    ) {
        assert_eq!(
            RegistryVersion::from((1, 1, 0))
                .encode_add_version_args(KEY, &source)
                .unwrap(),
            borsh::to_vec(&(KEY, mode, CODE.as_slice())).unwrap(),
        );
    }

    /// Pre-1.1.0 predates the tag: the blob goes over bare.
    #[test]
    fn pre_1_1_0_drops_the_source_tag() {
        assert_eq!(
            RegistryVersion::from((1, 0, 0))
                .encode_add_version_args(KEY, &VersionSource::Stored(Base64VecU8(CODE.to_vec())))
                .unwrap(),
            borsh::to_vec(&(KEY, CODE.as_slice())).unwrap(),
        );
    }

    /// A code hash has no pre-1.1.0 representation at all, so it must fail rather than encode into
    /// something that registry would misread as a wasm blob.
    #[test]
    fn pre_1_1_0_cannot_encode_a_code_hash() {
        assert!(RegistryVersion::from((1, 0, 0))
            .encode_add_version_args(
                KEY,
                &VersionSource::ExistingGlobal(Base58CryptoHash::from([7u8; 32])),
            )
            .is_err());
    }

    /// The three releases actually deployed — `templar-alpha.near`, `v1.tmplr.near` and
    /// `user0.tmplr.near` — must all read as unsupported, or every one of them fails preflight
    /// instead of degrading to the checks they can answer.
    #[rstest::rstest]
    #[case::alpha_near((0, 1, 0), false)]
    #[case::v1_tmplr_near((1, 0, 0), false)]
    #[case::user0_tmplr_near((1, 1, 0), false)]
    #[case((1, 2, 4), false)]
    #[case((1, 3, 0), false)]
    #[case((2, 0, 0), true)]
    fn entry_and_version_views_from_2_0_0(
        #[case] version: (u64, u64, u64),
        #[case] expected: bool,
    ) {
        assert_eq!(
            RegistryVersion::from(version).supports_entry_and_version_views(),
            expected,
        );
    }
}
