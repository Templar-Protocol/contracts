#[derive(Debug)]
pub struct ProxyGovernance;
pub type ProxyGovernanceVersion = super::Version<ProxyGovernance>;

impl ProxyGovernanceVersion {
    /// First version exposing `create_proposal_borsh`.
    pub const BORSH_CREATE_PROPOSAL: (u64, u64, u64) = (0, 3, 0);

    /// Check before sending one: an older contract has no such method, and the call would fail on
    /// chain after paying for it.
    pub fn supports_borsh_create_proposal(self) -> bool {
        self >= Self::BORSH_CREATE_PROPOSAL
    }
}

#[cfg(test)]
mod tests {
    use super::ProxyGovernanceVersion;

    #[rstest::rstest]
    #[case((0, 1, 0), false)]
    #[case((0, 2, 0), false)]
    #[case((0, 2, 9), false)]
    #[case((0, 3, 0), true)]
    #[case((0, 4, 0), true)]
    #[case((1, 0, 0), true)]
    fn supports_borsh_create_proposal_from_0_3_0(
        #[case] version: (u64, u64, u64),
        #[case] expected: bool,
    ) {
        assert_eq!(
            ProxyGovernanceVersion::from(version).supports_borsh_create_proposal(),
            expected,
        );
    }
}
