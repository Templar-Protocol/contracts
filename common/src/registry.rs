use near_sdk::{
    json_types::{Base58CryptoHash, U64},
    near,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "rpc", derive(clap::ValueEnum))]
#[near(serializers = [json, borsh])]
pub enum DeployMode {
    Normal,
    GlobalHash,
}

impl std::fmt::Display for DeployMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeployMode::Normal => write!(f, "Normal"),
            DeployMode::GlobalHash => write!(f, "GlobalHash"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct Deployment {
    pub version_key: String,
    pub code_hash: Base58CryptoHash,
    pub block_height: U64,
}

/// Where a registered version's code lives, and whether `deploy` can still use it.
///
/// `remove_version` soft-deletes by clearing the stored blob but keeping the key, and a
/// `GlobalHash` version never stores one — so "has code" cannot tell the two apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub enum VersionAvailability {
    /// Held in registry state. `code_len` sizes a chunked read of it.
    Stored { code_len: u32 },
    /// A NEAR global contract, resolvable by [`VersionInfo::code_hash`].
    Global,
    /// `remove_version` cleared the blob; the key remains but `deploy` panics.
    Removed,
}

impl VersionAvailability {
    pub fn is_deployable(self) -> bool {
        matches!(self, Self::Stored { .. } | Self::Global)
    }
}

/// A registered version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub struct VersionInfo {
    /// `sha256` of the code, computed by the registry when the version was added — unlike the
    /// digest embedded in a version key, which is a convention the registry does not enforce.
    pub code_hash: Base58CryptoHash,
    pub availability: VersionAvailability,
}

/// A name's entry in the deployment map.
///
/// `deploy` refuses any name already present, so `Reserved` blocks a deployment just as
/// `Deployed` does — a distinction [`Deployment`] alone cannot carry.
#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub enum RegistryEntryView {
    /// Claimed by an in-flight deploy that has not finalized.
    Reserved,
    Deployed(Deployment),
}

impl RegistryEntryView {
    pub fn deployment(&self) -> Option<&Deployment> {
        match self {
            Self::Reserved => None,
            Self::Deployed(deployment) => Some(deployment),
        }
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::serde_json::{self, json};
    use rstest::rstest;

    use super::*;

    fn deployment() -> Deployment {
        Deployment {
            version_key: "market@1.5.0".to_string(),
            code_hash: Base58CryptoHash::from([7u8; 32]),
            block_height: 42.into(),
        }
    }

    #[rstest]
    #[case(VersionAvailability::Stored { code_len: 521_039 }, json!({ "Stored": { "code_len": 521_039 } }))]
    #[case(VersionAvailability::Global, json!("Global"))]
    #[case(VersionAvailability::Removed, json!("Removed"))]
    fn availability_wire_format(
        #[case] availability: VersionAvailability,
        #[case] expected: serde_json::Value,
    ) {
        assert_eq!(serde_json::to_value(availability).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<VersionAvailability>(expected).unwrap(),
            availability,
        );
    }

    /// A `GlobalHash` version stores no code yet deploys fine, so "has code" would report it
    /// alongside a soft-deleted one.
    #[rstest]
    #[case(VersionAvailability::Stored { code_len: 1 }, true)]
    #[case(VersionAvailability::Global, true)]
    #[case(VersionAvailability::Removed, false)]
    fn deployability(#[case] availability: VersionAvailability, #[case] expected: bool) {
        assert_eq!(availability.is_deployable(), expected);
    }

    #[test]
    fn version_info_round_trips() {
        let info = VersionInfo {
            code_hash: Base58CryptoHash::from([3u8; 32]),
            availability: VersionAvailability::Stored { code_len: 128 },
        };
        let value = serde_json::to_value(info).unwrap();
        assert_eq!(serde_json::from_value::<VersionInfo>(value).unwrap(), info);
    }

    #[test]
    fn reserved_is_distinguishable_from_deployed() {
        let reserved = RegistryEntryView::Reserved;
        assert_eq!(serde_json::to_value(&reserved).unwrap(), json!("Reserved"));
        assert_eq!(reserved.deployment(), None);

        let deployed = RegistryEntryView::Deployed(deployment());
        assert_eq!(deployed.deployment(), Some(&deployment()));
        assert_eq!(
            serde_json::from_value::<RegistryEntryView>(serde_json::to_value(&deployed).unwrap())
                .unwrap(),
            deployed,
        );
    }
}
