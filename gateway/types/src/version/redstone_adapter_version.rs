#[derive(Debug)]
pub struct RedstoneAdapter;
pub type RedstoneAdapterVersion = super::Version<RedstoneAdapter>;

impl RedstoneAdapterVersion {
    /// Whether `new` requires `admin_id` (`>= 0.2.0`).
    pub fn new_requires_admin_id(self) -> bool {
        self >= (0, 2, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::RedstoneAdapterVersion;

    #[rstest::rstest]
    #[case((0, 1, 0), false)]
    #[case((0, 2, 0), true)]
    #[case((1, 0, 0), true)]
    fn new_requires_admin_id_from_0_2_0(#[case] version: (u64, u64, u64), #[case] expected: bool) {
        assert_eq!(
            RedstoneAdapterVersion::from(version).new_requires_admin_id(),
            expected
        );
    }

    #[test]
    fn from_version_key_reads_the_version_segment() {
        let key = format!("templar-redstone-adapter-contract@0.2.0#{:0>64}", "ab");

        assert_eq!(
            RedstoneAdapterVersion::from_version_key(&key).unwrap(),
            (0, 2, 0)
        );
    }
}
