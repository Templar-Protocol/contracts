#[derive(Debug)]
pub struct ProxyOracle;
pub type ProxyOracleVersion = super::Version<ProxyOracle>;

impl ProxyOracleVersion {
    /// Whether the proxy oracle is the kernelized contract (`>= 0.2.0`), which
    /// returns `Proxy<Source>` from `get_proxy` and delegates governance to a
    /// separate contract. Versions below `0.2.0` are the legacy contract whose
    /// `get_proxy` returns the pre-kernel `v0::Proxy` shape.
    pub fn proxy_is_kernelized(self) -> bool {
        self >= (0, 2, 0)
    }

    /// Whether `new` accepts an `owner_id` (`>= 0.3.0`).
    ///
    /// Check before sending one. An older `new` takes no arguments, and near-sdk
    /// only deserializes input for a method that declares some — so it does not
    /// reject an `owner_id`, it ignores it and seats the predecessor instead.
    pub fn new_accepts_owner_id(self) -> bool {
        self >= (0, 3, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::ProxyOracleVersion;

    #[rstest::rstest]
    #[case((0, 1, 0), false)]
    #[case((0, 2, 0), false)]
    #[case((0, 2, 9), false)]
    #[case((0, 3, 0), true)]
    #[case((0, 4, 0), true)]
    #[case((1, 0, 0), true)]
    fn new_accepts_owner_id_from_0_3_0(#[case] version: (u64, u64, u64), #[case] expected: bool) {
        assert_eq!(
            ProxyOracleVersion::from(version).new_accepts_owner_id(),
            expected,
        );
    }

    #[test]
    fn from_version_key_reads_the_version_segment() {
        let key = format!("templar-proxy-oracle-near-contract@0.3.0#{:0>64}", "ab");

        assert_eq!(
            ProxyOracleVersion::from_version_key(&key).unwrap(),
            (0, 3, 0)
        );
    }

    /// A pre-release is rejected rather than ordered: `sort -V` would place
    /// `0.3.0-rc1` *after* `0.3.0`, the opposite of semver.
    #[rstest::rstest]
    #[case::prerelease("templar-proxy-oracle-near-contract@0.3.0-rc1#abc")]
    #[case::not_a_version("templar-proxy-oracle-near-contract@latest#abc")]
    #[case::two_segments("templar-proxy-oracle-near-contract@0.3#abc")]
    #[case::no_at("templar-proxy-oracle-near-contract#abc")]
    #[case::no_hash("templar-proxy-oracle-near-contract@0.3.0")]
    #[case::empty_name("@0.3.0#abc")]
    #[case::empty_version("templar-proxy-oracle-near-contract@#abc")]
    #[case::empty_hash("templar-proxy-oracle-near-contract@0.3.0#")]
    fn from_version_key_rejects_malformed_keys(#[case] key: &str) {
        assert!(ProxyOracleVersion::from_version_key(key).is_err());
    }
}
