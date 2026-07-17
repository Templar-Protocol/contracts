#[derive(Debug)]
pub struct RedstoneAdapter;
pub type RedstoneAdapterVersion = super::Version<RedstoneAdapter>;

impl RedstoneAdapterVersion {
    /// Whether `new` accepts an optional `admin_id` (`>= 0.1.1`).
    pub fn new_accepts_admin_id(self) -> bool {
        self >= (0, 1, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::RedstoneAdapterVersion;

    #[rstest::rstest]
    #[case((0, 1, 0), false)]
    #[case((0, 1, 1), true)]
    #[case((0, 2, 0), true)]
    fn new_accepts_admin_id_from_0_1_1(#[case] version: (u64, u64, u64), #[case] expected: bool) {
        assert_eq!(
            RedstoneAdapterVersion::from(version).new_accepts_admin_id(),
            expected
        );
    }

    #[test]
    fn from_version_key_reads_the_version_segment() {
        let key = format!("templar-redstone-adapter-contract@0.1.1#{:0>64}", "ab");

        assert_eq!(
            RedstoneAdapterVersion::from_version_key(&key).unwrap(),
            (0, 1, 1)
        );
    }
}
