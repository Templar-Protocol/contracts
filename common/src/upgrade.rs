use near_sdk::{
    env,
    json_types::{Base58CryptoHash, Base64VecU8},
    near, AccountId, Gas, NearToken, Promise,
};

/// The migrate method every contract exposes for post-deploy state migration. The
/// [`UpgradeSource::deploy_and_migrate`] helper takes the name as a parameter, but all call sites
/// pass this constant so the method name stays uniform across contracts.
pub const MIGRATE_METHOD: &str = "migrate";

/// Where an upgrade's new code comes from. Deploying from a raw blob, a global contract by code
/// hash, or a global contract by account id are the three forms NEAR supports; all three batch
/// identically with a follow-up `migrate` call.
///
/// In JSON, `Code` is **untagged** — it serializes as a bare base64 string (matching the
/// pre-`UpgradeSource` wire, where the input was a bare `Base64VecU8`), while the global variants are
/// externally tagged (`{"GlobalHash": …}` / `{"GlobalAccountId": …}`). There is no ambiguity: a bare
/// string can only be `Code`, an object only a global variant. Borsh is unaffected by `untagged` and
/// tags every variant as usual.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum UpgradeSource {
    /// A NEAR global contract referenced by its code hash.
    GlobalHash(Base58CryptoHash),
    /// A NEAR global contract referenced by the account that published it.
    GlobalAccountId(AccountId),
    /// A raw WASM blob deployed onto the account. Untagged in JSON: a bare base64 string. serde
    /// requires untagged variants to be declared last.
    #[serde(untagged)]
    Code(Base64VecU8),
}

/// A compact, loggable summary of an [`UpgradeSource`], mirroring its three variants: for a raw
/// blob, the blob's sha256 (the blob itself is never logged); otherwise the global-contract
/// reference. Emitted by upgrade events in place of the (potentially multi-hundred-KB) source.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json])]
pub enum UpgradeSummary {
    /// sha256 of the deployed wasm blob.
    CodeHash(Base58CryptoHash),
    /// A global contract referenced by its code hash.
    GlobalHash(Base58CryptoHash),
    /// A global contract referenced by the account that published it.
    GlobalAccountId(AccountId),
}

impl UpgradeSource {
    /// A `Code` variant carrying an empty blob is never a valid deploy; the global variants have no
    /// blob to be empty.
    pub fn is_empty_code(&self) -> bool {
        matches!(self, UpgradeSource::Code(code) if code.0.is_empty())
    }

    /// A compact [`UpgradeSummary`] for event logging — for a `Code` blob, its sha256 (via the NEAR
    /// host `sha256`, so contract-only) rather than the blob itself.
    pub fn summary(&self) -> UpgradeSummary {
        match self {
            UpgradeSource::Code(blob) => {
                UpgradeSummary::CodeHash(env::sha256_array(&blob.0).into())
            }
            UpgradeSource::GlobalHash(hash) => UpgradeSummary::GlobalHash(*hash),
            UpgradeSource::GlobalAccountId(account_id) => {
                UpgradeSummary::GlobalAccountId(account_id.clone())
            }
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
            UpgradeSource::GlobalAccountId(account_id) => {
                promise.use_global_contract_by_account_id(account_id)
            }
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
    fn global_variants_stay_externally_tagged_in_json() {
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

        let account = UpgradeSource::GlobalAccountId("global.near".parse().unwrap());
        let value = serde_json::to_value(&account).unwrap();
        assert_eq!(value, json!({ "GlobalAccountId": "global.near" }));
        assert_eq!(
            serde_json::from_value::<UpgradeSource>(value).unwrap(),
            account
        );
    }

    #[test]
    fn all_variants_borsh_roundtrip() {
        for source in [
            UpgradeSource::Code(Base64VecU8(vec![1, 2, 3])),
            UpgradeSource::GlobalHash(Base58CryptoHash::from([7u8; 32])),
            UpgradeSource::GlobalAccountId("global.near".parse().unwrap()),
        ] {
            let bytes = near_sdk::borsh::to_vec(&source).unwrap();
            assert_eq!(
                near_sdk::borsh::from_slice::<UpgradeSource>(&bytes).unwrap(),
                source
            );
        }
    }
}
