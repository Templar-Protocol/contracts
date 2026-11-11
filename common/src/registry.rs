use near_sdk::{
    json_types::{Base58CryptoHash, Base64VecU8, U64},
    near, AccountId,
};

/// Delegated registry authority; Owner bypasses action guards without a synthetic membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, near_sdk::BorshStorageKey)]
#[near(serializers = [json, borsh(use_discriminant = true)])]
#[repr(u8)]
pub enum Role {
    Uploader = 0,
    Deployer = 1,
}

/// Code approved by the Owner without including WASM bytes in the approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[near(serializers = [json])]
pub enum ApprovedCode {
    Uploaded(Base58CryptoHash),
    Global(Base58CryptoHash),
}

/// Pending upload metadata; views never expose the stored WASM bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json])]
pub struct UploadInfo {
    pub uploader: AccountId,
    pub code_len: u32,
}

/// Store the wasm or publish it as a global contract — the only open question once the bytes are
/// in hand, which is why `registry.addArtifactVersion` still takes this.
///
/// [`VersionSource`] supersedes it on the `add_version` wire, where a third answer (a hash for code
/// already on chain) is possible and the bytes are not a given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[near(serializers = [json, borsh])]
pub enum DeployMode {
    Normal,
    GlobalHash,
}

/// Where a version's code comes from, as one value.
///
/// Supersedes the `(DeployMode, Vec<u8>)` pair, which admitted a combination that means nothing:
/// `Normal` alongside bytes already published as a global contract. Modelled on
/// [`crate::upgrade::UpgradeSource`], which draws the same distinction for the upgrade path.
///
/// Borsh tags are pinned via explicit discriminants (`use_discriminant`): `tmplrmgr` plan files
/// persist these args as opaque borsh, so the tag must not track declaration order. Discriminants
/// 0 and 1 match `DeployMode::Normal`/`GlobalHash`, and [`Base64VecU8`] is a transparent borsh
/// newtype over `Vec<u8>` — so `(version_key, Stored(code))` is byte-identical to the older
/// `(version_key, DeployMode::Normal, code)`. `borsh_is_wire_compatible_with_deploy_mode` pins this.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh(use_discriminant = true)])]
#[repr(u8)]
pub enum VersionSource {
    /// WASM held in registry state; `deploy` copies it onto each account.
    Stored(Base64VecU8) = 0,
    /// WASM published as a new global contract; the registry keeps only the hash.
    PublishGlobal(Base64VecU8) = 1,
    /// A global contract already on chain, by code hash. No publish cost.
    ExistingGlobal(Base58CryptoHash) = 2,
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
    #[case(Role::Uploader, 0, r#""Uploader""#)]
    #[case(Role::Deployer, 1, r#""Deployer""#)]
    fn role_wire_format(#[case] role: Role, #[case] tag: u8, #[case] expected: &str) {
        assert_eq!(near_sdk::borsh::to_vec(&role).unwrap(), [tag]);
        assert_eq!(near_sdk::borsh::from_slice::<Role>(&[tag]).unwrap(), role);
        assert_eq!(serde_json::to_string(&role).unwrap(), expected);
        assert_eq!(serde_json::from_str::<Role>(expected).unwrap(), role);
    }

    #[rstest]
    #[case(
        ApprovedCode::Uploaded(Base58CryptoHash::from([7u8; 32])),
        r#"{"version_key":"market@1.5.0","code":{"Uploaded":"US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx"}}"#,
    )]
    #[case(
        ApprovedCode::Global(Base58CryptoHash::from([7u8; 32])),
        r#"{"version_key":"market@1.5.0","code":{"Global":"US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx"}}"#,
    )]
    fn approved_code_wire_format(#[case] code: ApprovedCode, #[case] expected: &str) {
        #[derive(Debug, PartialEq)]
        #[near(serializers = [json])]
        struct Approval {
            version_key: String,
            code: ApprovedCode,
        }
        let approval = Approval {
            version_key: "market@1.5.0".to_string(),
            code,
        };
        assert_eq!(serde_json::to_string(&approval).unwrap(), expected);
        assert_eq!(
            serde_json::from_str::<Approval>(expected).unwrap(),
            approval
        );
    }

    #[test]
    fn upload_info_wire_format() {
        let info = UploadInfo {
            uploader: "uploader.near".parse().unwrap(),
            code_len: 4,
        };
        let expected = r#"{"uploader":"uploader.near","code_len":4}"#;
        assert_eq!(serde_json::to_string(&info).unwrap(), expected);
        assert_eq!(serde_json::from_str::<UploadInfo>(expected).unwrap(), info);
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

    /// The fold from `(DeployMode, Vec<u8>)` to one [`VersionSource`] must not move a byte, or
    /// every 1.1.0+ registry already on chain stops accepting `add_version`.
    #[rstest]
    #[case(DeployMode::Normal, VersionSource::Stored(Base64VecU8(vec![0xde, 0xad])))]
    #[case(DeployMode::GlobalHash, VersionSource::PublishGlobal(Base64VecU8(vec![0xde, 0xad])))]
    fn borsh_is_wire_compatible_with_deploy_mode(
        #[case] mode: DeployMode,
        #[case] source: VersionSource,
    ) {
        let version_key = "market@1.5.0";
        assert_eq!(
            near_sdk::borsh::to_vec(&(version_key, &source)).unwrap(),
            near_sdk::borsh::to_vec(&(version_key, mode, vec![0xdeu8, 0xad])).unwrap(),
        );
    }

    /// Golden bytes for the persisted tags. Plan files hold these opaquely, so a discriminant that
    /// shifted with declaration order would silently repoint an already-written plan.
    #[rstest]
    #[case(VersionSource::Stored(Base64VecU8(vec![0xaa])), vec![0, 1, 0, 0, 0, 0xaa])]
    #[case(VersionSource::PublishGlobal(Base64VecU8(vec![0xaa])), vec![1, 1, 0, 0, 0, 0xaa])]
    #[case(
        VersionSource::ExistingGlobal(Base58CryptoHash::from([7u8; 32])),
        [&[2u8][..], &[7u8; 32][..]].concat(),
    )]
    fn borsh_discriminants_are_stable(#[case] source: VersionSource, #[case] expected: Vec<u8>) {
        assert_eq!(near_sdk::borsh::to_vec(&source).unwrap(), expected);
    }

    #[rstest]
    #[case(VersionSource::Stored(Base64VecU8(vec![1, 2, 3])))]
    #[case(VersionSource::PublishGlobal(Base64VecU8(vec![1, 2, 3])))]
    #[case(VersionSource::ExistingGlobal(Base58CryptoHash::from([9u8; 32])))]
    fn version_source_round_trips(#[case] source: VersionSource) {
        let bytes = near_sdk::borsh::to_vec(&source).unwrap();
        assert_eq!(
            near_sdk::borsh::from_slice::<VersionSource>(&bytes).unwrap(),
            source,
        );
        let value = serde_json::to_value(&source).unwrap();
        assert_eq!(
            serde_json::from_value::<VersionSource>(value).unwrap(),
            source,
        );
    }

    /// The exact log line an indexer parses. Flattening puts the payload's fields beside
    /// `version_key`/`account_id`; a nested object would be a different wire format.
    #[rstest]
    #[case::version_added(
        RegistryEvent::VersionAdded {
            version_key: "market@1.5.0".to_string(),
            info: VersionInfo {
                code_hash: Base58CryptoHash::from([7u8; 32]),
                availability: VersionAvailability::Stored { code_len: 300 },
            },
        },
        r#"EVENT_JSON:{"standard":"templar-registry","version":"1.0.0","event":"version_added","data":{"version_key":"market@1.5.0","code_hash":"US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx","availability":{"Stored":{"code_len":300}}}}"#
    )]
    #[case::deployed(
        RegistryEvent::Deployed {
            account_id: "one.registry.near".parse().unwrap(),
            deployment: deployment(),
            deployer: Some("deployer.near".parse().unwrap()),
            full_access_keys: Some(vec!["ed25519:5BGSaf6YjVm7565VzWQHNxoyEjwr3jUpRJSGjREvU9dB".parse().unwrap()]),
        },
        r#"EVENT_JSON:{"standard":"templar-registry","version":"1.1.0","event":"deployed","data":{"account_id":"one.registry.near","version_key":"market@1.5.0","code_hash":"US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx","block_height":"42","deployer":"deployer.near","full_access_keys":["ed25519:5BGSaf6YjVm7565VzWQHNxoyEjwr3jUpRJSGjREvU9dB"]}}"#
    )]
    #[case::deployed_without_keys(
        RegistryEvent::Deployed {
            account_id: "one.registry.near".parse().unwrap(),
            deployment: deployment(),
            deployer: Some("deployer.near".parse().unwrap()),
            full_access_keys: Some(Vec::new()),
        },
        r#"EVENT_JSON:{"standard":"templar-registry","version":"1.1.0","event":"deployed","data":{"account_id":"one.registry.near","version_key":"market@1.5.0","code_hash":"US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx","block_height":"42","deployer":"deployer.near","full_access_keys":[]}}"#
    )]
    #[case::deployed_legacy_callback(
        RegistryEvent::Deployed {
            account_id: "one.registry.near".parse().unwrap(),
            deployment: deployment(),
            deployer: None,
            full_access_keys: None,
        },
        r#"EVENT_JSON:{"standard":"templar-registry","version":"1.1.0","event":"deployed","data":{"account_id":"one.registry.near","version_key":"market@1.5.0","code_hash":"US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx","block_height":"42","deployer":null,"full_access_keys":null}}"#
    )]
    #[case::code_uploaded(
        RegistryEvent::CodeUploaded {
            code_hash: Base58CryptoHash::from([7u8; 32]),
            uploader: "uploader.near".parse().unwrap(),
            code_len: 4,
        },
        r#"EVENT_JSON:{"standard":"templar-registry","version":"1.0.0","event":"code_uploaded","data":{"code_hash":"US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx","uploader":"uploader.near","code_len":4}}"#
    )]
    #[case::upload_removed(
        RegistryEvent::UploadRemoved {
            code_hash: Base58CryptoHash::from([7u8; 32]),
            uploader: "uploader.near".parse().unwrap(),
        },
        r#"EVENT_JSON:{"standard":"templar-registry","version":"1.0.0","event":"upload_removed","data":{"code_hash":"US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx","uploader":"uploader.near"}}"#
    )]
    #[case::role_granted(
        RegistryEvent::RoleSet {
            account_id: "uploader.near".parse().unwrap(),
            role: Role::Uploader,
            is_granted: true,
        },
        r#"EVENT_JSON:{"standard":"templar-registry","version":"1.0.0","event":"role_set","data":{"account_id":"uploader.near","role":"Uploader","is_granted":true}}"#
    )]
    #[case::role_revoked(
        RegistryEvent::RoleSet {
            account_id: "deployer.near".parse().unwrap(),
            role: Role::Deployer,
            is_granted: false,
        },
        r#"EVENT_JSON:{"standard":"templar-registry","version":"1.0.0","event":"role_set","data":{"account_id":"deployer.near","role":"Deployer","is_granted":false}}"#
    )]
    fn event_wire_format(#[case] event: RegistryEvent, #[case] expected: &str) {
        near_sdk::testing_env!(near_sdk::test_utils::VMContextBuilder::new().build());
        event.emit();
        assert_eq!(near_sdk::test_utils::get_logs(), [expected]);
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

#[near(event_json(standard = "templar-registry"))]
pub enum RegistryEvent {
    /// A version became deployable: logged by `add_version` for stored code, and by its finalize
    /// callback once a global contract is confirmed, never for one that turns out not to exist.
    #[event_version("1.0.0")]
    VersionAdded {
        version_key: String,
        #[serde(flatten)]
        info: VersionInfo,
    },
    /// `remove_version` cleared a stored version's code. The key stays registered but `deploy`
    /// refuses it.
    #[event_version("1.0.0")]
    VersionRemoved {
        version_key: String,
        code_hash: Base58CryptoHash,
    },
    /// A deployment's account was created and initialized, and the name now resolves to it.
    /// Legacy in-flight finalizers emit null provenance because they did not carry it.
    #[event_version("1.1.0")]
    Deployed {
        account_id: AccountId,
        #[serde(flatten)]
        deployment: Deployment,
        deployer: Option<AccountId>,
        full_access_keys: Option<Vec<near_sdk::PublicKey>>,
    },
    /// Code was stored pending Owner approval.
    #[event_version("1.0.0")]
    CodeUploaded {
        code_hash: Base58CryptoHash,
        uploader: AccountId,
        code_len: u32,
    },
    /// A pending upload was removed and its storage credit released to the payer.
    #[event_version("1.0.0")]
    UploadRemoved {
        code_hash: Base58CryptoHash,
        uploader: AccountId,
    },
    /// A successful role setter, following the proxy oracle's historical role-set payload.
    #[event_version("1.0.0")]
    RoleSet {
        account_id: AccountId,
        role: Role,
        is_granted: bool,
    },
}
