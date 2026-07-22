use near_sdk::{
    env,
    json_types::{Base58CryptoHash, Base64VecU8},
    near, Gas, NearToken, Promise,
};

/// The post-deploy state-migration method every contract exposes.
pub const MIGRATE_METHOD: &str = "migrate";

/// Where an upgrade's new code comes from — a raw blob, or a global contract pinned by code hash.
/// Both are immutable: exactly this code is deployed, then `migrate` runs. (Global-*by-account-id* is
/// deliberately excluded: it is a live delegation to the publisher, so a later republish would swap
/// the code with no proposal, timelock, or `migrate` — bypassing governance and bricking versioned
/// state.)
///
/// JSON: `Code` is untagged (a bare base64 string, matching the pre-`UpgradeSource` wire), so serde
/// requires it declared last; `GlobalHash` is externally tagged. Borsh tags are pinned via explicit
/// discriminants (`use_discriminant`), decoupling the persisted tag from declaration order —
/// `UpgradeSource` lives in pending governance proposals, so a future variant just takes its own
/// discriminant without disturbing the stored ones.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh(use_discriminant = true)])]
#[repr(u8)]
pub enum UpgradeSource {
    /// A NEAR global contract referenced by its (immutable) code hash.
    GlobalHash(Base58CryptoHash) = 1,
    /// A raw WASM blob deployed onto the account.
    #[serde(untagged)]
    Code(Base64VecU8) = 0,
}

/// A compact, loggable stand-in for an [`UpgradeSource`] — a hash, never the (potentially large) blob.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json])]
pub enum UpgradeSummary {
    /// sha256 of the deployed wasm blob.
    CodeHash(Base58CryptoHash),
    /// A global contract referenced by its code hash.
    GlobalHash(Base58CryptoHash),
}

impl UpgradeSource {
    /// A `Code` variant carrying an empty blob is never a valid deploy.
    pub fn is_empty_code(&self) -> bool {
        matches!(self, UpgradeSource::Code(code) if code.0.is_empty())
    }

    /// A compact [`UpgradeSummary`] for event logging (`Code` → its sha256, via the on-chain host).
    pub fn summary(&self) -> UpgradeSummary {
        match self {
            UpgradeSource::Code(blob) => {
                UpgradeSummary::CodeHash(env::sha256_array(&blob.0).into())
            }
            UpgradeSource::GlobalHash(hash) => UpgradeSummary::GlobalHash(*hash),
        }
    }

    /// Atomically deploy the new code and run `migrate_method` on it in a single receipt: a failed
    /// migration reverts the code change too. The deploy always targets the current account, so this
    /// is a self-upgrade primitive; the caller owns access control.
    pub fn deploy_and_migrate(
        self,
        migrate_method: impl Into<String>,
        migrate_args: Base64VecU8,
        gas: Gas,
    ) -> Promise {
        let promise = Promise::new(env::current_account_id());
        let deployed = match self {
            UpgradeSource::Code(code) => promise.deploy_contract(code.0),
            UpgradeSource::GlobalHash(hash) => promise.use_global_contract(hash),
        };
        deployed.function_call(
            migrate_method.into(),
            migrate_args.0,
            NearToken::from_yoctonear(0),
            gas,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::serde_json::{self, json};

    #[test]
    fn code_is_untagged_bare_base64_in_json() {
        let code = UpgradeSource::Code(Base64VecU8(vec![0xde, 0xad, 0xbe, 0xef]));
        let value = serde_json::to_value(&code).unwrap();
        // Bare base64 string, exactly as the pre-`UpgradeSource` `code` field serialized.
        assert_eq!(value, json!("3q2+7w=="));
        assert_eq!(
            serde_json::from_value::<UpgradeSource>(value).unwrap(),
            code
        );
    }

    #[test]
    fn global_hash_stays_externally_tagged_in_json() {
        let hash = UpgradeSource::GlobalHash(Base58CryptoHash::from([0u8; 32]));
        let value = serde_json::to_value(&hash).unwrap();
        assert_eq!(
            value,
            json!({ "GlobalHash": "11111111111111111111111111111111" })
        );
        assert_eq!(
            serde_json::from_value::<UpgradeSource>(value).unwrap(),
            hash
        );
    }

    #[test]
    fn both_variants_borsh_roundtrip() {
        for source in [
            UpgradeSource::Code(Base64VecU8(vec![1, 2, 3])),
            UpgradeSource::GlobalHash(Base58CryptoHash::from([7u8; 32])),
        ] {
            let bytes = near_sdk::borsh::to_vec(&source).unwrap();
            assert_eq!(
                near_sdk::borsh::from_slice::<UpgradeSource>(&bytes).unwrap(),
                source
            );
        }
    }

    /// Golden bytes for the persisted borsh format — `UpgradeSource` lives inside pending governance
    /// proposals, so these explicit discriminants must not shift.
    #[test]
    fn borsh_discriminants_are_stable() {
        // Code = tag 0, then a Vec<u8> (u32 LE length + bytes).
        assert_eq!(
            near_sdk::borsh::to_vec(&UpgradeSource::Code(Base64VecU8(vec![0xaa, 0xbb]))).unwrap(),
            vec![0, 2, 0, 0, 0, 0xaa, 0xbb],
        );
        // GlobalHash = tag 1, then the 32-byte hash.
        assert_eq!(
            near_sdk::borsh::to_vec(&UpgradeSource::GlobalHash(Base58CryptoHash::from(
                [0u8; 32]
            )))
            .unwrap(),
            [&[1u8][..], &[0u8; 32][..]].concat(),
        );
    }
}
