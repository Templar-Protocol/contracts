#![allow(clippy::needless_pass_by_value)]

mod state;

use std::ops::{Deref, DerefMut};

use near_sdk::{
    assert_one_yocto, env,
    json_types::{Base58CryptoHash, Base64VecU8},
    near, require, AccountId, CryptoHash, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue,
};
use near_sdk_contract_tools::{owner::Owner, Owner};
use templar_common::{
    contract::list,
    registry::{DeployMode, Deployment, RegistryEntryView, VersionAvailability, VersionInfo},
    self_ext,
    upgrade::{UpgradeSource, MIGRATE_METHOD},
    versioned_state::{impl_versioned_state, StateVersion, VersionedState},
};

type State = state::V1;

#[derive(Debug, Clone)]
#[near(serializers = [json, borsh])]
pub enum VersionEntry {
    Code {
        hash: CryptoHash,
        code: Option<Vec<u8>>,
    },
    GlobalHash(CryptoHash),
}

impl VersionEntry {
    pub fn code_hash(&self) -> CryptoHash {
        match self {
            Self::Code { hash, .. } | Self::GlobalHash(hash) => *hash,
        }
    }

    fn availability(&self) -> VersionAvailability {
        match self {
            Self::Code {
                code: Some(code), ..
            } => VersionAvailability::Stored {
                // Unreachable: a stored value cannot exceed the 4 MiB storage-value limit.
                code_len: u32::try_from(code.len()).unwrap_or(u32::MAX),
            },
            Self::Code { code: None, .. } => VersionAvailability::Removed,
            Self::GlobalHash(_) => VersionAvailability::Global,
        }
    }
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh])]
pub enum RegistryEntry {
    Reserved,
    Deployed(Deployment),
}

#[derive(PanicOnDefault, Owner)]
#[near(contract_state)]
pub struct Contract {
    pub state: VersionedState<State>,
}

// Generates the private `migrate()` entrypoint plus the `get_stored_state_version` /
// `get_target_state_version` / `needs_migration` views.
impl_versioned_state!(Contract, State, crate::state::Migration);

// `versions` and `registry` live on `State`; deref so the methods below keep reaching them
// directly.
impl Deref for Contract {
    type Target = State;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for Contract {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

#[near]
impl Contract {
    /// Gas reserved for the batched `migrate` in [`Self::upgrade`].
    ///
    /// Enough for any migration this method can be asked to run. The expensive one — rewriting
    /// every stored version blob out of a pre-1.1.0 layout, several MB on the live registries —
    /// is not among them: those releases have no `upgrade`, so they are migrated by a batch their
    /// key holder signs, where the gas is on the transaction rather than reserved here.
    pub const GAS_FOR_MIGRATE: Gas = Gas::from_tgas(250);

    /// Most a single [`Self::get_version_code_chunk`] will return.
    ///
    /// A view's return value reaches the caller as a JSON array of one integer per byte, so the
    /// response body runs several times the payload; this keeps it inside what RPC providers serve.
    pub const MAX_CODE_CHUNK_LEN: u32 = 128 * 1024;

    #[init]
    pub fn new() -> Self {
        let mut self_ = Self {
            state: State::new(()),
        };

        self_.init(&env::predecessor_account_id());

        self_
    }

    /// Atomically deploy new code and run its `migrate` in one receipt, so a failed migration
    /// reverts the deploy with it. `migrate_args` selects the `state::Migration` matching the
    /// layout this registry actually holds.
    ///
    /// The only way to replace the code of a registry whose full-access keys have been removed.
    #[payable]
    pub fn upgrade(&mut self, code: UpgradeSource, migrate_args: Base64VecU8) -> Promise {
        assert_one_yocto();
        self.assert_owner();

        require!(!code.is_empty_code(), "Upgrade code must not be empty");

        near_sdk::log!("Upgrading registry to {:?}", code.summary());

        code.deploy_and_migrate(MIGRATE_METHOD, migrate_args, Self::GAS_FOR_MIGRATE)
    }

    pub fn list_versions(&self, count: Option<u32>, offset: Option<u32>) -> Vec<&String> {
        list(self.versions.keys(), offset, count)
    }

    pub fn get_version_code_hash(&self, version_key: String) -> Option<Base58CryptoHash> {
        self.versions
            .get(&version_key)
            .map(VersionEntry::code_hash)
            .map(Into::into)
    }

    /// A registered version's hash and whether [`Self::deploy`] can still use it.
    ///
    /// `list_versions` keeps listing a version whose code `remove_version` cleared, and
    /// `get_version_code_hash` keeps answering for it, so membership alone cannot tell a
    /// deployable version from one that will panic partway through a deploy.
    pub fn get_version(&self, version_key: String) -> Option<VersionInfo> {
        self.versions.get(&version_key).map(|entry| VersionInfo {
            code_hash: entry.code_hash().into(),
            availability: entry.availability(),
        })
    }

    /// Whether a name is taken, and by what.
    ///
    /// [`Self::deploy`] refuses any name already present, so a `Reserved` name is as unusable as a
    /// deployed one — a state [`Self::get_deployment`] reports as absent.
    pub fn get_registry_entry(&self, account_id: AccountId) -> Option<RegistryEntryView> {
        self.registry.get(&account_id).map(|entry| match entry {
            RegistryEntry::Reserved => RegistryEntryView::Reserved,
            RegistryEntry::Deployed(deployment) => RegistryEntryView::Deployed(deployment.clone()),
        })
    }

    /// Bytes `[offset, offset + len)` of a version's stored code, or `None` if it has none.
    ///
    /// Borsh-serialized and chunked: the JSON encoding would base64 the blob and the RPC would
    /// then re-expand it to one integer per byte. Reading a whole contract back is the last
    /// resort anyway — [`Self::get_version`] reports the hash, which usually resolves against a
    /// released artifact without touching this at all.
    ///
    /// An `offset` past the end reads empty, so a caller can loop until it stops making progress.
    #[result_serializer(borsh)]
    pub fn get_version_code_chunk(
        &self,
        version_key: String,
        offset: u32,
        len: u32,
    ) -> Option<Vec<u8>> {
        require!(
            len <= Self::MAX_CODE_CHUNK_LEN,
            format!("len exceeds maximum of {}", Self::MAX_CODE_CHUNK_LEN),
        );

        let code = match self.versions.get(&version_key)? {
            VersionEntry::Code { code, .. } => code.as_ref()?,
            VersionEntry::GlobalHash(_) => return None,
        };

        let start = (offset as usize).min(code.len());
        let end = start.saturating_add(len as usize).min(code.len());

        Some(code[start..end].to_vec())
    }

    pub fn list_deployments(&self, count: Option<u32>, offset: Option<u32>) -> Vec<&AccountId> {
        list(
            self.registry
                .iter()
                .filter(|(_, e)| matches!(e, RegistryEntry::Deployed(_)))
                .map(|(a, _)| a),
            offset,
            count,
        )
    }

    pub fn get_deployment(&self, account_id: AccountId) -> Option<&Deployment> {
        self.registry.get(&account_id).and_then(|e| match e {
            RegistryEntry::Reserved => None,
            RegistryEntry::Deployed(deployment) => Some(deployment),
        })
    }

    #[payable]
    pub fn add_version(
        &mut self,
        #[serializer(borsh)] version_key: String,
        #[serializer(borsh)] mode: DeployMode,
        #[serializer(borsh)] code: Vec<u8>,
    ) -> PromiseOrValue<()> {
        self.assert_owner();
        require!(
            !self.versions.contains_key(&version_key),
            "Version key already exists",
        );

        let hash = env::sha256_array(&code);

        match mode {
            DeployMode::Normal => {
                assert_one_yocto();
                let version_entry = VersionEntry::Code {
                    hash,
                    code: Some(code.clone()),
                };
                self.versions.insert(version_key, version_entry);
                PromiseOrValue::Value(())
            }
            DeployMode::GlobalHash => {
                let deposit = env::attached_deposit();
                require!(
                    !deposit.is_zero(),
                    "Deposit required to pay for global contract deployment",
                );
                let version_entry = VersionEntry::GlobalHash(hash);
                self.versions.insert(version_key.clone(), version_entry);
                let dummy_id: AccountId = format!("deploy.{}", env::current_account_id())
                    .parse()
                    .unwrap_or_else(|_| {
                        templar_common::panic_with_message(
                            "Failed to construct deployment account ID.",
                        )
                    });
                PromiseOrValue::Promise(
                    Promise::new(dummy_id)
                        .create_account()
                        .transfer(deposit)
                        .deploy_global_contract(code)
                        .delete_account(env::current_account_id())
                        .then(self_ext!(Gas::from_tgas(6)).add_version_01_finalize(version_key)),
                )
            }
        }
    }

    #[private]
    pub fn add_version_01_finalize(&mut self, version_key: String) -> PromiseOrValue<()> {
        let result = env::promise_result_checked(0, 0x1000);
        if result.is_ok() {
            PromiseOrValue::Value(())
        } else {
            self.versions.remove(&version_key);
            PromiseOrValue::Promise(
                self_ext!(Gas::from_tgas(1)).fail("Failed to deploy global contract".to_string()),
            )
        }
    }

    #[payable]
    pub fn remove_version(&mut self, version_key: String) {
        assert_one_yocto();
        self.assert_owner();

        self.versions.entry(version_key).and_modify(|e| match e {
            VersionEntry::Code { code, .. } => {
                *code = None;
            }
            VersionEntry::GlobalHash(_) => {
                templar_common::panic_with_message("Global contract cannot be removed")
            }
        });
    }

    #[payable]
    pub fn deploy(
        &mut self,
        name: String,
        version_key: String,
        init_args: Base64VecU8,
        full_access_keys: Option<Vec<near_sdk::PublicKey>>,
    ) -> Promise {
        require!(!name.is_empty(), "Name must not be empty");
        self.assert_owner();

        // Through `Deref` every field access borrows the whole contract, so the read of `versions`
        // held across the write to `registry` needs them named as the disjoint fields they are.
        let state = &mut *self.state;

        let Some(version) = state.versions.get(&version_key) else {
            templar_common::panic_with_message("Version key does not exist");
        };

        let attached_deposit = env::attached_deposit();

        let current_account_id = env::current_account_id();
        let market_id = format!("{name}.{current_account_id}");

        let market_id: AccountId = market_id.parse().unwrap_or_else(|_| {
            templar_common::panic_with_message("New market ID is not a valid account ID")
        });

        require!(
            market_id.is_sub_account_of(&current_account_id),
            "Market ID cannot be created",
        );

        require!(
            !state.registry.contains_key(&market_id),
            "Market ID collision",
        );

        state
            .registry
            .insert(market_id.clone(), RegistryEntry::Reserved);

        near_sdk::log!("Deploying market to {market_id}");

        let mut promise = Promise::new(market_id.clone())
            .create_account()
            .transfer(env::attached_deposit());

        match version {
            VersionEntry::Code { code, .. } => {
                let code = code.as_ref().unwrap_or_else(|| {
                    templar_common::panic_with_message("Version code has been deleted")
                });

                let minimum_deposit = env::storage_byte_cost().saturating_mul(code.len() as u128);

                require!(
                    attached_deposit >= minimum_deposit,
                    format!("Insufficient deposit to pay for storage (minimum: {minimum_deposit})"),
                );

                promise = promise.deploy_contract(code.clone());
            }
            VersionEntry::GlobalHash(hash) => promise = promise.use_global_contract(*hash),
        }

        for key in full_access_keys.unwrap_or_default() {
            near_sdk::log!(
                "WARNING: Deploying market with full-access key {}",
                String::from(&key),
            );
            promise = promise.add_full_access_key(key);
        }

        promise
            .function_call_weight(
                "new".to_string(),
                init_args.0,
                NearToken::from_near(0),
                Gas::from_tgas(2),
                near_sdk::GasWeight(20),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_unused_gas_weight(1)
                    .with_static_gas(Gas::from_tgas(2))
                    .deploy_01_finalize(
                        market_id,
                        Deployment {
                            version_key,
                            code_hash: version.code_hash().into(),
                            block_height: env::block_height().into(),
                        },
                    ),
            )
    }

    #[private]
    pub fn deploy_01_finalize(
        &mut self,
        market_id: AccountId,
        deployment: Deployment,
    ) -> PromiseOrValue<AccountId> {
        let successful = env::promise_result_checked(0, 0x1000).is_ok();

        if successful {
            self.registry
                .insert(market_id.clone(), RegistryEntry::Deployed(deployment));

            PromiseOrValue::Value(market_id)
        } else {
            self.registry.remove(&market_id);

            PromiseOrValue::Promise(
                Self::ext(env::current_account_id()).fail("Market deployment failed".to_string()),
            )
        }
    }

    #[private]
    pub fn fail(&self, message: String) {
        templar_common::panic_with_message(&message);
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::{
        mock::MockAction,
        test_utils::{get_created_receipts, VMContextBuilder},
        testing_env,
    };
    use rstest::rstest;

    use super::*;

    const STORED: &str = "market@1.5.0";
    const REMOVED: &str = "market@1.0.0";
    const GLOBAL: &str = "oracle@0.4.1";

    fn contract() -> Contract {
        testing_env!(VMContextBuilder::new().build());
        let mut contract = Contract::new();
        contract.versions.insert(
            STORED.to_string(),
            VersionEntry::Code {
                hash: [1u8; 32],
                code: Some(vec![0xau8; 300]),
            },
        );
        contract.versions.insert(
            REMOVED.to_string(),
            VersionEntry::Code {
                hash: [2u8; 32],
                code: None,
            },
        );
        contract
            .versions
            .insert(GLOBAL.to_string(), VersionEntry::GlobalHash([3u8; 32]));
        contract
    }

    #[rstest]
    #[case(STORED, Some(VersionAvailability::Stored { code_len: 300 }))]
    #[case(REMOVED, Some(VersionAvailability::Removed))]
    #[case(GLOBAL, Some(VersionAvailability::Global))]
    #[case("nothing@0.0.0", None)]
    fn get_version_separates_all_four_states(
        #[case] key: &str,
        #[case] expected: Option<VersionAvailability>,
    ) {
        let info = contract().get_version(key.to_string());
        assert_eq!(info.map(|info| info.availability), expected);
    }

    /// `get_deployment` maps a reserved name to `None`, which reads as "free" — but `deploy`
    /// refuses it just as firmly as a deployed one.
    #[test]
    fn get_registry_entry_reports_reserved_that_get_deployment_hides() {
        let mut contract = contract();
        let reserved: AccountId = "reserved.registry.near".parse().unwrap();
        let deployed: AccountId = "deployed.registry.near".parse().unwrap();
        let deployment = Deployment {
            version_key: STORED.to_string(),
            code_hash: [1u8; 32].into(),
            block_height: 1.into(),
        };
        contract
            .registry
            .insert(reserved.clone(), RegistryEntry::Reserved);
        contract.registry.insert(
            deployed.clone(),
            RegistryEntry::Deployed(deployment.clone()),
        );

        assert_eq!(contract.get_deployment(reserved.clone()), None);
        assert_eq!(
            contract.get_registry_entry(reserved),
            Some(RegistryEntryView::Reserved),
        );
        assert_eq!(
            contract.get_registry_entry(deployed),
            Some(RegistryEntryView::Deployed(deployment)),
        );
        assert_eq!(
            contract.get_registry_entry("free.registry.near".parse().unwrap()),
            None,
        );
    }

    #[rstest]
    #[case::whole(0, 300, 300)]
    #[case::prefix(0, 10, 10)]
    #[case::tail(290, 64, 10)]
    #[case::past_the_end_reads_empty(300, 64, 0)]
    #[case::far_past_the_end_reads_empty(9_999, 64, 0)]
    fn code_chunk_clamps_to_the_blob(
        #[case] offset: u32,
        #[case] len: u32,
        #[case] expected: usize,
    ) {
        let chunk = contract()
            .get_version_code_chunk(STORED.to_string(), offset, len)
            .expect("a stored version yields bytes");
        assert_eq!(chunk.len(), expected);
        assert!(chunk.iter().all(|byte| *byte == 0xau8));
    }

    /// Reassembly is the point: chunks concatenated in order must equal the stored blob.
    #[test]
    fn code_chunks_reassemble_exactly() {
        let contract = contract();
        let mut reassembled = Vec::new();
        let mut offset = 0;
        loop {
            let chunk = contract
                .get_version_code_chunk(STORED.to_string(), offset, 128)
                .expect("a stored version yields bytes");
            if chunk.is_empty() {
                break;
            }
            offset += u32::try_from(chunk.len()).unwrap();
            reassembled.extend(chunk);
        }
        assert_eq!(reassembled, vec![0xau8; 300]);
    }

    /// Neither a global version nor one whose code was removed has bytes to read, so the caller
    /// has to resolve those by hash instead.
    #[rstest]
    #[case(GLOBAL)]
    #[case(REMOVED)]
    #[case("nothing@0.0.0")]
    fn code_chunk_is_absent_without_stored_code(#[case] key: &str) {
        assert_eq!(
            contract().get_version_code_chunk(key.to_string(), 0, 64),
            None,
        );
    }

    #[test]
    #[should_panic(expected = "len exceeds maximum")]
    fn code_chunk_refuses_an_oversized_read() {
        contract().get_version_code_chunk(STORED.to_string(), 0, Contract::MAX_CODE_CHUNK_LEN + 1);
    }

    /// Deploy and `migrate` must share one receipt, which is what makes a failed migration revert
    /// the code with it. Two receipts would leave the new code deployed over unmigrated state.
    #[test]
    fn upgrade_batches_deploy_then_migrate_into_one_self_receipt() {
        testing_env!(VMContextBuilder::new()
            .current_account_id("registry.near".parse().unwrap())
            .predecessor_account_id("registry.near".parse().unwrap())
            .attached_deposit(NearToken::from_yoctonear(1))
            .build());
        let mut contract = Contract::new();
        let code = vec![0xde, 0xad, 0xbe, 0xef];
        let migrate_args = br#"{"from_version":"pre_global_contracts"}"#.to_vec();

        contract
            .upgrade(
                UpgradeSource::Code(Base64VecU8(code.clone())),
                Base64VecU8(migrate_args.clone()),
            )
            .detach();

        let receipts = get_created_receipts();
        assert_eq!(receipts.len(), 1, "the upgrade must not fan out");
        let receipt = &receipts[0];
        assert_eq!(receipt.receiver_id.as_str(), "registry.near");
        assert_eq!(receipt.actions.len(), 2);

        let receipt_index = match &receipt.actions[0] {
            MockAction::DeployContract {
                receipt_index,
                code: deployed,
            } => {
                assert_eq!(deployed, &code);
                *receipt_index
            }
            action => panic!("expected the deploy first, got {action:?}"),
        };
        match &receipt.actions[1] {
            MockAction::FunctionCallWeight {
                receipt_index: migrate_index,
                method_name,
                args,
                prepaid_gas,
                ..
            } => {
                assert_eq!(
                    *migrate_index, receipt_index,
                    "migrate must ride the deploy"
                );
                assert_eq!(method_name, b"migrate");
                assert_eq!(args, &migrate_args);
                assert_eq!(*prepaid_gas, Contract::GAS_FOR_MIGRATE);
            }
            action => panic!("expected the migrate second, got {action:?}"),
        }
    }

    #[test]
    #[should_panic(expected = "Requires attached deposit of exactly 1 yoctoNEAR")]
    fn upgrade_requires_one_yocto() {
        testing_env!(VMContextBuilder::new()
            .current_account_id("registry.near".parse().unwrap())
            .predecessor_account_id("registry.near".parse().unwrap())
            .build());
        let mut contract = Contract::new();
        contract
            .upgrade(
                UpgradeSource::Code(Base64VecU8(vec![1, 2, 3])),
                Base64VecU8(Vec::new()),
            )
            .detach();
    }

    #[test]
    #[should_panic(expected = "Upgrade code must not be empty")]
    fn upgrade_refuses_an_empty_blob() {
        testing_env!(VMContextBuilder::new()
            .current_account_id("registry.near".parse().unwrap())
            .predecessor_account_id("registry.near".parse().unwrap())
            .attached_deposit(NearToken::from_yoctonear(1))
            .build());
        let mut contract = Contract::new();
        contract
            .upgrade(
                UpgradeSource::Code(Base64VecU8(Vec::new())),
                Base64VecU8(Vec::new()),
            )
            .detach();
    }
}

#[cfg(target_arch = "wasm32")]
mod custom_getrandom {
    #![allow(clippy::no_mangle_with_rust_abi)]

    use getrandom::{register_custom_getrandom, Error};
    use near_sdk::env;

    register_custom_getrandom!(custom_getrandom);

    #[allow(clippy::unnecessary_wraps)]
    pub fn custom_getrandom(buf: &mut [u8]) -> Result<(), Error> {
        buf.copy_from_slice(&env::random_seed_array());
        Ok(())
    }
}
