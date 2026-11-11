#![allow(clippy::needless_pass_by_value)]

mod state;
mod storage_management;

use std::ops::{Deref, DerefMut};

use near_sdk::{
    assert_one_yocto, env,
    json_types::{Base58CryptoHash, Base64VecU8},
    near, require,
    store::LazyOption,
    AccountId, CryptoHash, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue,
};
use near_sdk_contract_tools::{
    owner::{Owner, OwnerExternal},
    rbac::Rbac,
    Owner, Rbac,
};
use templar_common::{
    contract::list,
    registry::{
        ApprovedCode, Deployment, RegistryEntryView, RegistryEvent, Role, UploadInfo,
        VersionAvailability, VersionInfo, VersionSource,
    },
    self_ext,
    upgrade::{UpgradeSource, MIGRATE_METHOD},
    versioned_state::{impl_versioned_state, StateVersion, VersionedState},
    UnwrapReject,
};

type State = state::V2;

#[near(serializers = [borsh])]
pub struct Upload {
    pub uploader: AccountId,
    code_len: u32,
    storage_charge: NearToken,
    // IterableMap swap-removal must rewrite only this handle, never another upload's blob.
    code: LazyOption<Vec<u8>>,
}

impl Upload {
    fn new(hash: CryptoHash, uploader: AccountId, code: Vec<u8>) -> Self {
        let code_len = u32::try_from(code.len()).unwrap_or(u32::MAX);
        let mut code = LazyOption::new(state::StorageKey::UploadCode { hash }, Some(code));
        // Persist the blob before its map handle; storage accounting includes both writes.
        code.flush();
        Self {
            uploader,
            code_len,
            storage_charge: NearToken::from_yoctonear(0),
            code,
        }
    }

    fn into_parts(mut self) -> (AccountId, NearToken, Vec<u8>) {
        let code = self.code.take().unwrap_or_else(|| {
            templar_common::panic_with_message("Invariant violation: upload code is missing")
        });
        self.code.flush();
        (self.uploader, self.storage_charge, code)
    }

    fn clear_code(&mut self) {
        self.code.set(None);
        self.code.flush();
    }
}

#[near(serializers = [borsh])]
pub struct StorageCharge {
    payer: AccountId,
    amount: NearToken,
}

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

    fn info(&self) -> VersionInfo {
        VersionInfo {
            code_hash: self.code_hash().into(),
            availability: self.availability(),
        }
    }
}

#[derive(Debug, Clone)]
#[near(serializers = [borsh])]
pub enum RegistryEntry {
    Reserved,
    Deployed(Deployment),
    /// New reservations retain their final-record budget; legacy V1 reservations have none.
    ReservedWithStorage(NearToken),
}

impl RegistryEntry {
    fn view(&self) -> RegistryEntryView {
        match self {
            Self::Reserved | Self::ReservedWithStorage(_) => RegistryEntryView::Reserved,
            Self::Deployed(deployment) => RegistryEntryView::Deployed(deployment.clone()),
        }
    }
}

#[derive(PanicOnDefault, Owner, Rbac)]
#[rbac(roles = "Role")]
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
    /// Leave 90 Tgas for decoding and deploying local code, alongside the
    /// separate 10 Tgas solvency check.
    pub const GAS_FOR_MIGRATE: Gas = Gas::from_tgas(200);
    const GLOBAL_PUBLISH_ACCOUNT: &str = "deploy";
    const GLOBAL_PROBE_ACCOUNT: &str = "probe";

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

    /// Explicit RBAC membership; Owner authority is checked separately by action guards.
    pub fn has_role(&self, account_id: AccountId, role: Role) -> bool {
        <Self as Rbac>::has_role(&account_id, &role)
    }

    fn assert_role(&self, role: Role) {
        require!(
            self.own_get_owner().as_ref() == Some(&env::predecessor_account_id())
                || <Self as Rbac>::has_role(&env::predecessor_account_id(), &role),
            "Unauthorized role"
        );
    }

    #[payable]
    pub fn set_role(
        &mut self,
        account_id: AccountId,
        role: Role,
        set: Option<bool>,
        allow_removing_last_member: Option<bool>,
    ) {
        assert_one_yocto();
        self.assert_owner();
        let is_granted = set.unwrap_or(true);
        if is_granted {
            <Self as Rbac>::add_role(self, &account_id, &role);
        } else {
            if !allow_removing_last_member.unwrap_or(false) {
                require!(
                    <Self as Rbac>::with_members_of(&role, |members| members.len()) > 1,
                    "Cannot remove the last member of a role",
                );
            }
            <Self as Rbac>::remove_role(self, &account_id, &role);
        }
        self.assert_storage_solvency(NearToken::from_yoctonear(0));
        RegistryEvent::RoleSet {
            account_id,
            role,
            is_granted,
        }
        .emit();
    }

    pub fn list_role(&self, role: Role, offset: Option<u32>, count: Option<u32>) -> Vec<AccountId> {
        list(<Self as Rbac>::iter_members_of(&role), offset, count)
    }

    /// Stage bytes using the caller's prepaid NEP-145 available balance.
    pub fn upload_code(&mut self, #[serializer(borsh)] code: Vec<u8>) -> Base58CryptoHash {
        self.assert_role(Role::Uploader);
        let hash = env::sha256_array(&code);
        require!(!self.uploads.contains_key(&hash), "Code already uploaded");
        let uploader = env::predecessor_account_id();
        let storage_before = env::storage_usage();
        let upload = Upload::new(hash, uploader.clone(), code);
        let code_len = upload.code_len;
        self.uploads.insert(hash, upload);
        self.uploads.flush();
        let charge = env::storage_byte_cost()
            .checked_mul(u128::from(env::storage_usage() - storage_before))
            .expect_or_reject("Upload storage cost overflow");
        self.charge_storage(&uploader, charge);
        self.uploads
            .get_mut(&hash)
            .expect_or_reject("Upload was just inserted")
            .storage_charge = charge;
        self.uploads.flush();
        self.assert_storage_solvency(NearToken::from_yoctonear(0));
        RegistryEvent::CodeUploaded {
            code_hash: hash.into(),
            uploader,
            code_len,
        }
        .emit();
        hash.into()
    }

    fn remove_pending_upload(&mut self, hash: &CryptoHash) -> Option<AccountId> {
        let mut upload = self.uploads.remove(hash)?;
        upload.clear_code();
        self.uploads.flush();
        self.release_storage(&upload.uploader, upload.storage_charge);
        Some(upload.uploader)
    }

    /// Release pending storage credit to its payer; NEAR is reclaimed with `storage_withdraw`.
    #[payable]
    pub fn remove_upload(&mut self, code_hash: Base58CryptoHash) {
        assert_one_yocto();
        self.assert_role(Role::Uploader);
        let hash: CryptoHash = code_hash.into();
        let upload = self
            .uploads
            .get(&hash)
            .unwrap_or_else(|| templar_common::panic_with_message("Upload does not exist"));
        require!(
            self.own_get_owner().as_ref() == Some(&env::predecessor_account_id())
                || upload.uploader == env::predecessor_account_id(),
            "Upload belongs to another account",
        );
        let uploader = self
            .remove_pending_upload(&hash)
            .unwrap_or_else(|| templar_common::panic_with_message("Upload does not exist"));
        self.assert_storage_solvency(NearToken::from_yoctonear(0));
        RegistryEvent::UploadRemoved {
            code_hash,
            uploader,
        }
        .emit();
    }

    pub fn get_upload(&self, code_hash: Base58CryptoHash) -> Option<UploadInfo> {
        self.uploads
            .get(&CryptoHash::from(code_hash))
            .map(|upload| UploadInfo {
                uploader: upload.uploader.clone(),
                code_len: upload.code_len,
            })
    }

    pub fn list_uploads(&self, count: Option<u32>, offset: Option<u32>) -> Vec<Base58CryptoHash> {
        list(self.uploads.keys().copied().map(Into::into), offset, count)
    }

    /// Atomically deploy new code and run its `migrate` in one receipt, so a failed migration
    /// reverts the deploy with it. `migrate_args` names the `state::Migration` chain to run, and is
    /// empty when the stored state version is already the new code's.
    ///
    /// The only way to replace the code of a registry whose full-access keys have been removed.
    #[payable]
    pub fn upgrade(&mut self, code: UpgradeSource, migrate_args: Base64VecU8) -> Promise {
        assert_one_yocto();
        self.assert_owner();

        require!(!code.is_empty_code(), "Upgrade code must not be empty");

        near_sdk::log!("Upgrading registry to {:?}", code.summary());

        code.deploy_and_migrate(MIGRATE_METHOD, migrate_args, Self::GAS_FOR_MIGRATE)
            .function_call(
                "upgrade_check_storage".to_owned(),
                b"{}".to_vec(),
                NearToken::ZERO,
                Gas::from_tgas(10),
            )
    }

    /// Runs in the deploy/migrate batch, so insufficient backing rolls back the upgrade too.
    #[private]
    pub fn upgrade_check_storage(&self) {
        self.assert_storage_solvency(NearToken::ZERO);
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
        self.versions.get(&version_key).map(VersionEntry::info)
    }

    /// Whether a name is taken, and by what.
    ///
    /// [`Self::deploy`] refuses any name already present, so a `Reserved` name is as unusable as a
    /// deployed one — a state [`Self::get_deployment`] reports as absent.
    pub fn get_registry_entry(&self, account_id: AccountId) -> Option<RegistryEntryView> {
        self.registry.get(&account_id).map(RegistryEntry::view)
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
            RegistryEntry::Reserved | RegistryEntry::ReservedWithStorage(_) => None,
            RegistryEntry::Deployed(deployment) => Some(deployment),
        })
    }

    /// Open a throwaway sub-account funded by the attached deposit, for a caller to act on and
    /// then `delete_account` back into the registry.
    ///
    /// `prefix` separates the publish and probe accounts so a probe cannot land on an in-flight
    /// publish.
    fn scratch_account(prefix: &str) -> Promise {
        let account_id: AccountId = format!("{prefix}.{}", env::current_account_id())
            .parse()
            .unwrap_or_else(|_| {
                templar_common::panic_with_message("Failed to construct scratch account ID.")
            });

        Promise::new(account_id)
            .create_account()
            .transfer(env::attached_deposit())
    }

    /// Approve uploaded bytes or probe an existing global contract, without sending a blob.
    #[payable]
    pub fn approve_version(
        &mut self,
        version_key: String,
        code: ApprovedCode,
    ) -> PromiseOrValue<()> {
        assert_one_yocto();
        self.assert_owner();
        require!(
            !self.versions.contains_key(&version_key),
            "Version key already exists",
        );
        match code {
            ApprovedCode::Uploaded(hash) => {
                let hash: CryptoHash = hash.into();
                require!(self.uploads.contains_key(&hash), "Upload does not exist");
                // The permanent version-key/tombstone metadata is registry-funded.
                self.versions
                    .insert(version_key.clone(), VersionEntry::Code { hash, code: None });
                self.versions.flush();
                let storage_before = env::storage_usage();
                let upload = self
                    .uploads
                    .remove(&hash)
                    .unwrap_or_else(|| templar_common::panic_with_message("Upload does not exist"));
                let (payer, previous_charge, code) = upload.into_parts();
                let version = VersionEntry::Code {
                    hash,
                    code: Some(code),
                };
                let info = version.info();
                self.versions.insert(version_key.clone(), version);
                self.version_storage.insert(
                    version_key.clone(),
                    StorageCharge {
                        payer: payer.clone(),
                        amount: NearToken::from_yoctonear(0),
                    },
                );
                self.uploads.flush();
                self.versions.flush();
                self.version_storage.flush();
                let storage_after = env::storage_usage();
                let retained_charge = if storage_after >= storage_before {
                    let growth = env::storage_byte_cost()
                        .checked_mul(u128::from(storage_after - storage_before))
                        .expect_or_reject("Retained storage cost overflow");
                    self.charge_storage(&payer, growth);
                    previous_charge.checked_add(growth)
                } else {
                    let released = env::storage_byte_cost()
                        .checked_mul(u128::from(storage_before - storage_after))
                        .expect_or_reject("Released storage cost overflow");
                    self.release_storage(&payer, released);
                    previous_charge.checked_sub(released)
                }
                .expect_or_reject("Invariant violation: retained upload storage charge");
                self.version_storage
                    .get_mut(&version_key)
                    .expect_or_reject("Version storage payer was just inserted")
                    .amount = retained_charge;
                self.version_storage.flush();
                self.assert_storage_solvency(NearToken::from_yoctonear(0));
                RegistryEvent::VersionAdded { version_key, info }.emit();
                PromiseOrValue::Value(())
            }
            ApprovedCode::Global(hash) => {
                self.add_version(version_key, VersionSource::ExistingGlobal(hash))
            }
        }
    }

    fn finish_version(&mut self, version_key: String, info: VersionInfo) {
        self.versions.flush();
        let _ = self.remove_pending_upload(&CryptoHash::from(info.code_hash));
        self.assert_storage_solvency(NearToken::from_yoctonear(0));
        RegistryEvent::VersionAdded { version_key, info }.emit();
    }

    #[payable]
    pub fn add_version(
        &mut self,
        #[serializer(borsh)] version_key: String,
        #[serializer(borsh)] source: VersionSource,
    ) -> PromiseOrValue<()> {
        self.assert_owner();
        require!(
            !self.versions.contains_key(&version_key),
            "Version key already exists",
        );

        match source {
            VersionSource::Stored(code) => {
                assert_one_yocto();
                let version_entry = VersionEntry::Code {
                    hash: env::sha256_array(&code.0),
                    code: Some(code.0),
                };
                let info = version_entry.info();
                self.versions.insert(version_key.clone(), version_entry);
                self.finish_version(version_key, info);
                PromiseOrValue::Value(())
            }
            VersionSource::PublishGlobal(code) => {
                require!(
                    !env::attached_deposit().is_zero(),
                    "Deposit required to pay for global contract deployment",
                );
                let version_entry = VersionEntry::GlobalHash(env::sha256_array(&code.0));
                self.versions.insert(version_key.clone(), version_entry);
                self.versions.flush();
                self.assert_storage_solvency(env::attached_deposit());
                PromiseOrValue::Promise(
                    Self::scratch_account(Self::GLOBAL_PUBLISH_ACCOUNT)
                        .deploy_global_contract(code.0)
                        .delete_account(env::current_account_id())
                        .then(self_ext!(Gas::from_tgas(20)).add_version_01_finalize(version_key)),
                )
            }
            // The hash is verified, not trusted: a hash with no global contract behind it fails
            // this receipt and `add_version_01_finalize` frees the key again. Without the probe an
            // unverified typo would burn the key permanently, since `remove_version` panics on a
            // `GlobalHash` entry.
            VersionSource::ExistingGlobal(hash) => {
                // One yocto, as for `Stored`: the probe account is created and deleted inside a
                // single receipt, so it never has to satisfy a storage-staking minimum, and
                // `use_global_contract` copies no code to stake for. The cost here is gas.
                assert_one_yocto();
                self.versions
                    .insert(version_key.clone(), VersionEntry::GlobalHash(hash.into()));
                self.versions.flush();
                self.assert_storage_solvency(env::attached_deposit());
                PromiseOrValue::Promise(
                    Self::scratch_account(Self::GLOBAL_PROBE_ACCOUNT)
                        .use_global_contract(hash)
                        .delete_account(env::current_account_id())
                        .then(self_ext!(Gas::from_tgas(20)).add_version_01_finalize(version_key)),
                )
            }
        }
    }

    #[private]
    pub fn add_version_01_finalize(&mut self, version_key: String) -> PromiseOrValue<()> {
        let result = env::promise_result_checked(0, 0x1000);
        if result.is_ok() {
            if let Some(info) = self.versions.get(&version_key).map(VersionEntry::info) {
                self.finish_version(version_key, info);
            }
            PromiseOrValue::Value(())
        } else {
            self.versions.remove(&version_key);
            self.versions.flush();
            // This only releases the provisional key. An existing backing
            // shortfall must not roll back failed-global cleanup.
            PromiseOrValue::Promise(
                self_ext!(Gas::from_tgas(1)).fail("Failed to deploy global contract".to_string()),
            )
        }
    }

    #[payable]
    pub fn remove_version(&mut self, version_key: String) {
        assert_one_yocto();
        self.assert_owner();

        match self.versions.get_mut(&version_key) {
            None | Some(VersionEntry::Code { code: None, .. }) => {}
            Some(VersionEntry::Code { hash, code }) => {
                *code = None;
                let code_hash = (*hash).into();
                self.versions.flush();
                if let Some(charge) = self.version_storage.remove(&version_key) {
                    self.version_storage.flush();
                    self.release_storage(&charge.payer, charge.amount);
                }
                self.assert_storage_solvency(NearToken::from_yoctonear(0));
                RegistryEvent::VersionRemoved {
                    version_key,
                    code_hash,
                }
                .emit();
            }
            Some(VersionEntry::GlobalHash(_)) => {
                templar_common::panic_with_message("Global contract cannot be removed")
            }
        }
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
        self.assert_role(Role::Deployer);
        let full_access_keys = Some(full_access_keys.unwrap_or_default());
        require!(
            !matches!(
                name.as_str(),
                Self::GLOBAL_PROBE_ACCOUNT | Self::GLOBAL_PUBLISH_ACCOUNT
            ),
            "Deployment name is reserved",
        );

        // Through `Deref` every field access borrows the whole contract, so the read of `versions`
        // held across the write to `registry` needs them named as the disjoint fields they are.
        let state = &mut *self.state;

        let Some(version) = state.versions.get(&version_key) else {
            templar_common::panic_with_message("Version key does not exist");
        };

        let attached_deposit = env::attached_deposit();

        let current_account_id = env::current_account_id();
        let account_id: AccountId = format!("{name}.{current_account_id}")
            .parse()
            .unwrap_or_else(|_| {
                templar_common::panic_with_message("Deployment name is not a valid account ID")
            });

        require!(
            account_id.is_sub_account_of(&current_account_id),
            "Deployment account cannot be created",
        );

        require!(
            !state.registry.contains_key(&account_id),
            // templar-backend's relayer matches this text verbatim until ENG-781.
            "Market ID collision",
        );

        let deployment = Deployment {
            version_key,
            code_hash: version.code_hash().into(),
            block_height: env::block_height().into(),
        };
        let final_bytes = near_sdk::borsh::object_length(&deployment).unwrap_or_reject();
        let reserved_bytes = near_sdk::borsh::object_length(&NearToken::ZERO).unwrap_or_reject();
        let storage_reserve = env::storage_byte_cost()
            .checked_mul(
                u128::try_from(
                    final_bytes
                        .checked_sub(reserved_bytes)
                        .expect_or_reject("Invariant violation: deployment storage reserve"),
                )
                .unwrap_or_reject(),
            )
            .expect_or_reject("Deployment storage reserve overflow");
        state.pending_deployment_storage = state
            .pending_deployment_storage
            .checked_add(storage_reserve)
            .expect_or_reject("Deployment reserve overflow");
        state.registry.insert(
            account_id.clone(),
            RegistryEntry::ReservedWithStorage(storage_reserve),
        );

        near_sdk::log!("Deploying {} to {account_id}", deployment.version_key);

        let mut promise = Promise::new(account_id.clone())
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

        promise = add_full_access_keys(promise, full_access_keys.as_deref().unwrap_or_default());

        let promise = promise
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
                        Some(account_id),
                        None,
                        deployment,
                        Some(env::predecessor_account_id()),
                        full_access_keys,
                    ),
            );
        self.registry.flush();
        self.assert_storage_solvency(attached_deposit);
        promise
    }

    #[private]
    pub fn deploy_01_finalize(
        &mut self,
        account_id: Option<AccountId>,
        // 2.0.0's name for `account_id`, so its callbacks still in flight across the upgrade land.
        market_id: Option<AccountId>,
        deployment: Deployment,
        // Option fields deserialize missing 2.0.0 provenance as None and emit null.
        deployer: Option<AccountId>,
        full_access_keys: Option<Vec<near_sdk::PublicKey>>,
    ) -> PromiseOrValue<AccountId> {
        let Some(account_id) = account_id.or(market_id) else {
            templar_common::panic_with_message("Missing deployment account ID");
        };
        if let Some(RegistryEntry::ReservedWithStorage(reserve)) = self.registry.get(&account_id) {
            self.pending_deployment_storage = self
                .pending_deployment_storage
                .checked_sub(*reserve)
                .expect_or_reject("Invariant violation: deployment reserve underflow");
        }
        let successful = env::promise_result_checked(0, 0x1000).is_ok();

        if successful {
            self.registry.insert(
                account_id.clone(),
                RegistryEntry::Deployed(deployment.clone()),
            );
            self.registry.flush();
            self.assert_storage_solvency(NearToken::from_yoctonear(0));
            RegistryEvent::Deployed {
                account_id: account_id.clone(),
                deployment,
                deployer,
                full_access_keys,
            }
            .emit();
            PromiseOrValue::Value(account_id)
        } else {
            self.registry.remove(&account_id);
            self.registry.flush();
            // This only releases storage and the held reserve. An existing
            // backing shortfall must not roll back failed-deployment cleanup.

            PromiseOrValue::Promise(
                Self::ext(env::current_account_id()).fail("Deployment failed".to_string()),
            )
        }
    }

    #[private]
    pub fn fail(&self, message: String) {
        templar_common::panic_with_message(&message);
    }
}

fn add_full_access_keys(mut promise: Promise, full_access_keys: &[near_sdk::PublicKey]) -> Promise {
    for key in full_access_keys {
        near_sdk::log!(
            "WARNING: Deploying with full-access key {}",
            String::from(key),
        );
        promise = promise.add_full_access_key(key.clone());
    }
    promise
}

#[cfg(test)]
mod tests {
    use near_sdk::{
        mock::MockAction,
        serde_json,
        test_utils::{get_created_receipts, get_logs, VMContextBuilder},
        testing_env, PromiseResult,
    };
    use rstest::rstest;

    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Identity {
        Owner,
        Uploader,
        Deployer,
        Both,
        Unrelated,
    }

    impl Identity {
        fn account(self) -> AccountId {
            match self {
                Self::Owner => "owner.near",
                Self::Uploader => "uploader.near",
                Self::Deployer => "deployer.near",
                Self::Both => "both.near",
                Self::Unrelated => "unrelated.near",
            }
            .parse()
            .unwrap()
        }
    }

    fn authorized_contract(identity: Identity, deposit: NearToken) -> Contract {
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(Identity::Owner.account())
            .build());
        let mut contract = Contract::new();
        for (account, roles) in [
            (Identity::Uploader, &[Role::Uploader][..]),
            (Identity::Deployer, &[Role::Deployer][..]),
            (Identity::Both, &[Role::Uploader, Role::Deployer][..]),
        ] {
            for role in roles {
                <Contract as Rbac>::add_role(&mut contract, &account.account(), role);
            }
        }
        for account in [
            Identity::Owner,
            Identity::Uploader,
            Identity::Deployer,
            Identity::Both,
            Identity::Unrelated,
        ] {
            fund_storage(&mut contract, account.account());
        }
        testing_env!(VMContextBuilder::new()
            .storage_usage(env::storage_usage())
            .signer_account_id(Identity::Owner.account())
            .predecessor_account_id(identity.account())
            .attached_deposit(deposit)
            .build());
        contract
    }

    fn fund_storage(contract: &mut Contract, payer: AccountId) {
        testing_env!(VMContextBuilder::new()
            .storage_usage(env::storage_usage())
            .predecessor_account_id(payer)
            .attached_deposit(NearToken::from_near(1))
            .build());
        contract.storage_deposit(None, None);
    }

    fn stage_upload(contract: &mut Contract, payer: AccountId, code: Vec<u8>) -> Base58CryptoHash {
        let predecessor = env::predecessor_account_id();
        let signer = env::signer_account_id();
        let attached = env::attached_deposit();
        let explicit_role = <Contract as Rbac>::has_role(&payer, &Role::Uploader);
        if !explicit_role {
            <Contract as Rbac>::add_role(contract, &payer, &Role::Uploader);
        }
        contract.versions.flush();
        testing_env!(VMContextBuilder::new()
            .storage_usage(env::storage_usage())
            .predecessor_account_id(payer.clone())
            .build());
        let hash = contract.upload_code(code);
        if !explicit_role {
            <Contract as Rbac>::remove_role(contract, &payer, &Role::Uploader);
        }
        testing_env!(VMContextBuilder::new()
            .storage_usage(env::storage_usage())
            .predecessor_account_id(predecessor)
            .signer_account_id(signer)
            .attached_deposit(attached)
            .build());
        hash
    }

    fn assert_panics(message: &str, action: impl FnOnce()) {
        let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).unwrap_err();
        let actual = error
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| error.downcast_ref::<&str>().copied())
            .unwrap();
        assert!(
            actual.contains(message),
            "expected {message:?}, got {actual:?}"
        );
    }

    #[derive(Clone, Copy, Debug)]
    enum AdminMethod {
        ApproveUploaded,
        ApproveGlobal,
        AddStored,
        AddPublishGlobal,
        AddExistingGlobal,
        RemoveVersion,
        Upgrade,
        GrantRole,
        RevokeRole,
        ProposeOwner,
    }

    fn call_admin(contract: &mut Contract, method: AdminMethod) {
        let hash = env::sha256_array([42]);
        match method {
            AdminMethod::ApproveUploaded => {
                let _ =
                    contract.approve_version("new".to_owned(), ApprovedCode::Uploaded(hash.into()));
            }
            AdminMethod::ApproveGlobal => {
                let _ =
                    contract.approve_version("new".to_owned(), ApprovedCode::Global(hash.into()));
            }
            AdminMethod::AddStored => {
                let _ =
                    contract.add_version("new".to_owned(), VersionSource::Stored(vec![42].into()));
            }
            AdminMethod::AddPublishGlobal => {
                let _ = contract.add_version(
                    "new".to_owned(),
                    VersionSource::PublishGlobal(vec![42].into()),
                );
            }
            AdminMethod::AddExistingGlobal => {
                let _ = contract
                    .add_version("new".to_owned(), VersionSource::ExistingGlobal(hash.into()));
            }
            AdminMethod::RemoveVersion => contract.remove_version("existing".to_owned()),
            AdminMethod::Upgrade => contract
                .upgrade(UpgradeSource::Code(vec![42].into()), vec![].into())
                .detach(),
            AdminMethod::GrantRole => {
                contract.set_role(
                    Identity::Unrelated.account(),
                    Role::Uploader,
                    Some(true),
                    None,
                );
            }
            AdminMethod::RevokeRole => {
                contract.set_role(
                    Identity::Uploader.account(),
                    Role::Uploader,
                    Some(false),
                    Some(true),
                );
            }
            AdminMethod::ProposeOwner => {
                contract.own_propose_owner(Some(Identity::Unrelated.account()));
            }
        }
    }

    #[rstest]
    fn admin_methods_reject_hot_roles(
        #[values(
            Identity::Owner,
            Identity::Uploader,
            Identity::Deployer,
            Identity::Both,
            Identity::Unrelated
        )]
        identity: Identity,
        #[values(
            AdminMethod::ApproveUploaded,
            AdminMethod::ApproveGlobal,
            AdminMethod::AddStored,
            AdminMethod::AddPublishGlobal,
            AdminMethod::AddExistingGlobal,
            AdminMethod::RemoveVersion,
            AdminMethod::Upgrade,
            AdminMethod::GrantRole,
            AdminMethod::RevokeRole,
            AdminMethod::ProposeOwner
        )]
        method: AdminMethod,
    ) {
        let mut contract = authorized_contract(identity, NearToken::from_yoctonear(1));
        let hash = env::sha256_array([42]);
        stage_upload(&mut contract, Identity::Uploader.account(), vec![42]);
        contract.versions.insert(
            "existing".to_owned(),
            VersionEntry::Code {
                hash: [8; 32],
                code: Some(vec![8]),
            },
        );
        if identity == Identity::Owner {
            call_admin(&mut contract, method);
            match method {
                AdminMethod::ApproveUploaded | AdminMethod::AddStored => assert_eq!(
                    contract.get_version("new".to_owned()).unwrap().availability,
                    VersionAvailability::Stored { code_len: 1 }
                ),
                AdminMethod::ApproveGlobal
                | AdminMethod::AddPublishGlobal
                | AdminMethod::AddExistingGlobal => assert_eq!(
                    contract.get_version("new".to_owned()).unwrap().availability,
                    VersionAvailability::Global
                ),
                AdminMethod::RemoveVersion => assert_eq!(
                    contract
                        .get_version("existing".to_owned())
                        .unwrap()
                        .availability,
                    VersionAvailability::Removed
                ),
                AdminMethod::GrantRole => {
                    assert!(contract.has_role(Identity::Unrelated.account(), Role::Uploader));
                }
                AdminMethod::RevokeRole => {
                    assert!(!contract.has_role(Identity::Uploader.account(), Role::Uploader));
                }
                AdminMethod::ProposeOwner => assert_eq!(
                    contract.own_get_proposed_owner(),
                    Some(Identity::Unrelated.account())
                ),
                AdminMethod::Upgrade => {}
            }
        } else {
            assert_panics("Owner only", || call_admin(&mut contract, method));
            assert_eq!(contract.get_version("new".to_owned()), None);
            assert_eq!(
                contract.get_upload(hash.into()).unwrap().uploader,
                Identity::Uploader.account()
            );
            assert_eq!(
                contract
                    .get_version("existing".to_owned())
                    .unwrap()
                    .availability,
                VersionAvailability::Stored { code_len: 1 }
            );
            assert_eq!(contract.own_get_proposed_owner(), None);
            assert!(contract.has_role(Identity::Uploader.account(), Role::Uploader));
            assert!(!contract.has_role(Identity::Unrelated.account(), Role::Uploader));
            assert!(emitted_events().is_empty());
            assert!(get_created_receipts().is_empty());
        }
    }

    #[rstest]
    fn admin_confirmation_deposits_are_exact(
        #[values(0, 2)] deposit: u128,
        #[values(
            AdminMethod::ApproveUploaded,
            AdminMethod::ApproveGlobal,
            AdminMethod::AddStored,
            AdminMethod::AddExistingGlobal,
            AdminMethod::RemoveVersion,
            AdminMethod::Upgrade,
            AdminMethod::GrantRole,
            AdminMethod::RevokeRole,
            AdminMethod::ProposeOwner
        )]
        method: AdminMethod,
    ) {
        let mut contract = authorized_contract(Identity::Owner, NearToken::from_yoctonear(deposit));
        assert_panics("Requires attached deposit of exactly 1 yoctoNEAR", || {
            call_admin(&mut contract, method);
        });
    }

    #[rstest]
    fn uploader_methods_follow_current_predecessor_roles(
        #[values(
            Identity::Owner,
            Identity::Uploader,
            Identity::Deployer,
            Identity::Both,
            Identity::Unrelated
        )]
        identity: Identity,
    ) {
        let mut contract = authorized_contract(identity, NearToken::ZERO);
        let hash = env::sha256_array([42]);
        let allowed = matches!(
            identity,
            Identity::Owner | Identity::Uploader | Identity::Both
        );
        if allowed {
            assert_eq!(contract.upload_code(vec![42]), Base58CryptoHash::from(hash));
            assert_eq!(
                contract.get_upload(hash.into()).unwrap().uploader,
                identity.account()
            );
            testing_env!(VMContextBuilder::new()
                .predecessor_account_id(identity.account())
                .attached_deposit(NearToken::from_yoctonear(1))
                .build());
            contract.remove_upload(hash.into());
            assert_eq!(contract.get_upload(hash.into()), None);
            assert!(get_created_receipts().is_empty());
        } else {
            assert_panics("Unauthorized role", || {
                contract.upload_code(vec![42]);
            });
            assert!(contract.uploads.is_empty());
            contract
                .uploads
                .insert(hash, Upload::new(hash, identity.account(), vec![42]));
            testing_env!(VMContextBuilder::new()
                .predecessor_account_id(identity.account())
                .attached_deposit(NearToken::from_yoctonear(1))
                .build());
            assert_panics("Unauthorized role", || contract.remove_upload(hash.into()));
            assert!(contract.get_upload(hash.into()).is_some());
        }
    }

    #[rstest]
    fn only_proposed_owner_can_accept(
        #[values(
            Identity::Owner,
            Identity::Uploader,
            Identity::Deployer,
            Identity::Both,
            Identity::Unrelated
        )]
        identity: Identity,
    ) {
        let mut contract = authorized_contract(identity, NearToken::from_yoctonear(1));
        <Contract as Owner>::update_proposed_unchecked(
            &mut contract,
            Some(Identity::Unrelated.account()),
        );
        if identity == Identity::Unrelated {
            contract.own_accept_owner();
            assert_eq!(contract.own_get_owner(), Some(identity.account()));
            assert_eq!(contract.own_get_proposed_owner(), None);
        } else {
            assert_panics("Proposed owner only", || contract.own_accept_owner());
            assert_eq!(contract.own_get_owner(), Some(Identity::Owner.account()));
        }
    }

    #[rstest]
    fn upload_removal_checks_recorded_uploader_and_current_role(
        #[values(Identity::Owner, Identity::Uploader, Identity::Both)] identity: Identity,
    ) {
        let mut contract = authorized_contract(identity, NearToken::from_yoctonear(1));
        let hash = env::sha256_array([42]);
        stage_upload(&mut contract, Identity::Unrelated.account(), vec![42]);
        if identity == Identity::Owner {
            contract.remove_upload(hash.into());
            assert_eq!(contract.get_upload(hash.into()), None);
        } else {
            assert_panics("Upload belongs to another account", || {
                contract.remove_upload(hash.into());
            });
            assert!(contract.get_upload(hash.into()).is_some());
            <Contract as Rbac>::remove_role(&mut contract, &identity.account(), &Role::Uploader);
            assert_panics("Unauthorized role", || contract.remove_upload(hash.into()));
            assert!(contract.get_upload(hash.into()).is_some());
            if identity == Identity::Both {
                assert!(contract.has_role(identity.account(), Role::Deployer));
            }
        }
    }

    #[rstest]
    fn uploader_confirmation_deposits_are_exact(#[values(0, 2)] deposit: u128) {
        let mut contract = authorized_contract(Identity::Owner, NearToken::from_yoctonear(deposit));
        assert_panics("Requires attached deposit of exactly 1 yoctoNEAR", || {
            contract.remove_upload([0; 32].into());
        });
        assert_panics("Requires attached deposit of exactly 1 yoctoNEAR", || {
            contract.own_accept_owner();
        });
    }

    #[rstest]
    fn duplicate_uploads_do_not_replace_recorded_uploader(
        #[values(Identity::Owner, Identity::Uploader, Identity::Both)] identity: Identity,
    ) {
        let mut contract = authorized_contract(identity, NearToken::from_near(1));
        let hash = env::sha256_array([42]);
        contract.uploads.insert(
            hash,
            Upload::new(hash, Identity::Uploader.account(), vec![42]),
        );
        assert_panics("Code already uploaded", || {
            contract.upload_code(vec![42]);
        });
        assert_eq!(
            contract.get_upload(hash.into()).unwrap().uploader,
            Identity::Uploader.account()
        );
    }

    #[test]
    fn missing_or_occupied_approval_never_consumes_another_upload() {
        let mut contract = authorized_contract(Identity::Owner, NearToken::from_yoctonear(1));
        let hash = env::sha256_array([42]);
        stage_upload(&mut contract, Identity::Uploader.account(), vec![42]);
        let _ =
            contract.approve_version("occupied".to_owned(), ApprovedCode::Uploaded(hash.into()));
        contract.uploads.insert(
            [9; 32],
            Upload::new([9; 32], Identity::Uploader.account(), vec![9]),
        );
        assert_panics("Version key already exists", || {
            let _ = contract.approve_version(
                "occupied".to_owned(),
                ApprovedCode::Uploaded([9; 32].into()),
            );
        });
        assert!(contract.get_upload([9; 32].into()).is_some());
        assert_panics("Upload does not exist", || {
            let _ =
                contract.approve_version("free".to_owned(), ApprovedCode::Uploaded([8; 32].into()));
        });
        assert_eq!(contract.get_version("free".to_owned()), None);
        assert_panics("Upload does not exist", || {
            contract.remove_upload([8; 32].into());
        });
    }

    #[rstest]
    fn views_are_public_and_report_explicit_membership(
        #[values(
            Identity::Owner,
            Identity::Uploader,
            Identity::Deployer,
            Identity::Both,
            Identity::Unrelated
        )]
        identity: Identity,
    ) {
        use templar_common::versioned_state::MigrateExternalInterface;
        let mut contract = authorized_contract(identity, NearToken::ZERO);
        assert_eq!(Contract::get_stored_state_version(), 2);
        assert_eq!(Contract::get_target_state_version(), 2);
        assert!(!Contract::needs_migration());
        assert_eq!(contract.own_get_owner(), Some(Identity::Owner.account()));
        assert_eq!(contract.own_get_proposed_owner(), None);
        assert!(!contract.has_role(Identity::Owner.account(), Role::Uploader));
        assert!(!contract.has_role(Identity::Owner.account(), Role::Deployer));
        assert_eq!(
            contract.list_role(Role::Uploader, None, None),
            [Identity::Uploader.account(), Identity::Both.account()]
        );
        assert_eq!(
            contract.list_role(Role::Deployer, Some(1), Some(1)),
            [Identity::Both.account()]
        );
        assert!(contract.list_uploads(None, None).is_empty());
        assert!(contract.list_versions(None, None).is_empty());
        assert!(contract.list_deployments(None, None).is_empty());
        assert_eq!(contract.get_upload([1; 32].into()), None);
        assert_eq!(contract.get_version("missing".to_owned()), None);
        assert_eq!(contract.get_version_code_hash("missing".to_owned()), None);
        assert_eq!(
            contract.get_version_code_chunk("missing".to_owned(), 0, 1),
            None
        );
        assert_eq!(
            contract.get_registry_entry(Identity::Unrelated.account()),
            None
        );
        assert_eq!(contract.get_deployment(Identity::Unrelated.account()), None);
        contract.versions.insert(
            "version".to_owned(),
            VersionEntry::Code {
                hash: [1; 32],
                code: Some(vec![1, 2]),
            },
        );
        contract.uploads.insert(
            [2; 32],
            Upload::new([2; 32], Identity::Uploader.account(), vec![3]),
        );
        let deployed = Deployment {
            version_key: "version".to_owned(),
            code_hash: [1; 32].into(),
            block_height: 7.into(),
        };
        contract.registry.insert(
            Identity::Unrelated.account(),
            RegistryEntry::Deployed(deployed.clone()),
        );
        assert_eq!(
            contract.get_version("version".to_owned()),
            Some(VersionInfo {
                code_hash: [1; 32].into(),
                availability: VersionAvailability::Stored { code_len: 2 },
            })
        );
        assert_eq!(
            contract.get_version_code_hash("version".to_owned()),
            Some([1; 32].into())
        );
        assert_eq!(
            contract.get_version_code_chunk("version".to_owned(), 1, 2),
            Some(vec![2])
        );
        assert_eq!(
            contract.get_upload([2; 32].into()),
            Some(UploadInfo {
                uploader: Identity::Uploader.account(),
                code_len: 1
            })
        );
        assert_eq!(
            contract.list_uploads(None, None),
            [Base58CryptoHash::from([2; 32])]
        );
        assert_eq!(contract.list_versions(None, None), [&"version".to_owned()]);
        assert_eq!(
            contract.list_deployments(None, None),
            [&Identity::Unrelated.account()]
        );
        assert_eq!(
            contract.get_registry_entry(Identity::Unrelated.account()),
            Some(RegistryEntryView::Deployed(deployed.clone()))
        );
        assert_eq!(
            contract.get_deployment(Identity::Unrelated.account()),
            Some(&deployed)
        );
        for (count, offset) in [(Some(0), None), (None, Some(99))] {
            assert!(contract.list_versions(count, offset).is_empty());
            assert!(contract.list_deployments(count, offset).is_empty());
            assert!(contract.list_uploads(count, offset).is_empty());
            assert!(contract.list_role(Role::Uploader, offset, count).is_empty());
        }
    }

    #[test]
    fn role_set_preserves_other_roles_and_reports_idempotent_updates() {
        let mut contract = authorized_contract(Identity::Owner, NearToken::from_yoctonear(1));
        contract.set_role(
            Identity::Both.account(),
            Role::Uploader,
            Some(false),
            Some(true),
        );
        assert_eq!(
            emitted_events(),
            [RegistryEvent::RoleSet {
                account_id: Identity::Both.account(),
                role: Role::Uploader,
                is_granted: false,
            }
            .to_json()]
        );
        assert!(!contract.has_role(Identity::Both.account(), Role::Uploader));
        assert!(contract.has_role(Identity::Both.account(), Role::Deployer));
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(Identity::Owner.account())
            .attached_deposit(NearToken::from_yoctonear(1))
            .build());
        contract.set_role(
            Identity::Both.account(),
            Role::Uploader,
            Some(false),
            Some(true),
        );
        assert_eq!(
            emitted_events(),
            [RegistryEvent::RoleSet {
                account_id: Identity::Both.account(),
                role: Role::Uploader,
                is_granted: false,
            }
            .to_json()]
        );
    }

    #[rstest]
    #[case::stored(None)]
    #[case::global_success(Some(true))]
    #[case::global_failure(Some(false))]
    fn registration_consumes_matching_upload_only_on_success(#[case] global: Option<bool>) {
        let mut contract = contract();
        let code = vec![0, 1, 2, 255];
        let hash = env::sha256_array(&code);
        let payer = env::predecessor_account_id();
        stage_upload(&mut contract, payer, code.clone());
        next_receipt(Vec::new());
        let source = if global.is_some() {
            VersionSource::ExistingGlobal(hash.into())
        } else {
            VersionSource::Stored(code.into())
        };
        let _ = contract.add_version("new".to_owned(), source);
        if let Some(success) = global {
            assert!(emitted_events().is_empty());
            assert!(contract.get_upload(hash.into()).is_some());
            next_receipt(vec![if success {
                PromiseResult::Successful(vec![])
            } else {
                PromiseResult::Failed
            }]);
            let _ = contract.add_version_01_finalize("new".to_owned());
        }
        let success = global.unwrap_or(true);
        assert_eq!(contract.get_upload(hash.into()).is_none(), success);
        assert_eq!(contract.get_version("new".to_owned()).is_some(), success);
        assert_eq!(emitted_events().len(), usize::from(success));
        assert!(!get_created_receipts().iter().any(|receipt| receipt
            .actions
            .iter()
            .any(|action| matches!(action, MockAction::Transfer { .. }))));
    }

    #[test]
    fn uploaded_approval_moves_bytes_and_retains_removed_key() {
        let mut contract = contract();
        let code = vec![0, 1, 2, 255];
        let hash = env::sha256_array(&code);
        let payer = env::predecessor_account_id();
        stage_upload(&mut contract, payer, code.clone());
        next_receipt(Vec::new());
        let _ = contract.approve_version("new".to_owned(), ApprovedCode::Uploaded(hash.into()));
        assert_eq!(contract.get_upload(hash.into()), None);
        assert_eq!(
            contract.get_version_code_chunk("new".to_owned(), 0, 10),
            Some(code)
        );
        let info = VersionInfo {
            code_hash: hash.into(),
            availability: VersionAvailability::Stored { code_len: 4 },
        };
        assert_eq!(contract.get_version("new".to_owned()), Some(info));
        assert_eq!(
            emitted_events(),
            [RegistryEvent::VersionAdded {
                version_key: "new".to_owned(),
                info
            }
            .to_json()]
        );
        contract.remove_version("new".to_owned());
        assert_eq!(
            contract.get_version("new".to_owned()).unwrap().availability,
            VersionAvailability::Removed
        );
        assert!(contract
            .list_versions(None, None)
            .iter()
            .any(|key| key.as_str() == "new"));
    }

    #[rstest]
    #[case(0)]
    #[case(4)]
    #[case(32_768)]
    fn upload_storage_is_locked_and_owner_removal_credits_the_uploader(#[case] code_len: usize) {
        let mut contract = authorized_contract(Identity::Uploader, NearToken::ZERO);
        let payer = Identity::Uploader.account();
        let before = contract.storage_balance_of(payer.clone()).unwrap();
        let storage_before = env::storage_usage();
        let hash = contract.upload_code(vec![42; code_len]);
        let stored_bytes = env::storage_usage() - storage_before;
        let cost = env::storage_byte_cost()
            .checked_mul(u128::from(stored_bytes))
            .unwrap();
        let charged = contract.storage_balance_of(payer.clone()).unwrap();
        assert_eq!(charged.total, before.total);
        assert_eq!(
            charged.available,
            before.available.checked_sub(cost).unwrap()
        );
        assert_eq!(
            contract.get_upload(hash).unwrap().code_len,
            u32::try_from(code_len).unwrap()
        );
        testing_env!(VMContextBuilder::new()
            .storage_usage(env::storage_usage())
            .predecessor_account_id(Identity::Owner.account())
            .attached_deposit(NearToken::from_yoctonear(1))
            .build());
        contract.remove_upload(hash);
        assert_eq!(contract.storage_balance_of(payer).unwrap(), before);
        assert_eq!(env::storage_usage(), storage_before);
        assert!(get_created_receipts().is_empty());
    }

    #[rstest]
    #[case::owner(Identity::Owner, true)]
    #[case::uploader(Identity::Uploader, false)]
    #[case::deployer(Identity::Deployer, true)]
    #[case::both(Identity::Both, true)]
    #[case::unrelated(Identity::Unrelated, false)]
    fn deploy_authority_follows_predecessor(#[case] identity: Identity, #[case] allowed: bool) {
        // The fixture sets signer=Owner independently of the selected predecessor.
        let mut contract = authorized_contract(identity, NearToken::from_near(1));
        contract.versions.insert(
            "code".to_owned(),
            VersionEntry::Code {
                hash: [1; 32],
                code: Some(vec![1]),
            },
        );
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract
                .deploy("child".to_owned(), "code".to_owned(), vec![].into(), None)
                .detach();
        }));
        if allowed {
            outcome.unwrap();
            let child: AccountId = format!("child.{}", env::current_account_id())
                .parse()
                .unwrap();
            assert_eq!(
                contract.get_registry_entry(child),
                Some(RegistryEntryView::Reserved)
            );
        } else {
            let error = outcome.unwrap_err();
            let message = error
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| error.downcast_ref::<&str>().copied())
                .unwrap();
            assert!(message.contains("Unauthorized role"), "{message}");
            assert!(contract.registry.is_empty());
        }
    }

    #[test]
    fn ownership_transfer_preserves_explicit_grants_without_synthetic_membership() {
        let mut contract = contract();
        let old = contract.own_get_owner().unwrap();
        let new: AccountId = "new.near".parse().unwrap();
        next_receipt(Vec::new());
        contract.set_role(old.clone(), Role::Uploader, Some(true), None);
        assert_eq!(
            emitted_events(),
            [RegistryEvent::RoleSet {
                account_id: old.clone(),
                role: Role::Uploader,
                is_granted: true,
            }
            .to_json()]
        );
        next_receipt(Vec::new());
        contract.set_role(old.clone(), Role::Uploader, Some(true), None);
        contract.set_role(old.clone(), Role::Deployer, Some(true), None);
        contract.set_role(old.clone(), Role::Deployer, Some(false), Some(true));
        assert!(contract.has_role(old.clone(), Role::Uploader));
        assert!(contract.list_role(Role::Deployer, None, None).is_empty());
        next_receipt(Vec::new());
        contract.set_role(old.clone(), Role::Deployer, Some(false), Some(true));
        assert_eq!(
            contract.list_role(Role::Uploader, None, None),
            [old.clone()]
        );
        assert!(contract.list_role(Role::Uploader, None, Some(0)).is_empty());
        assert!(contract
            .list_role(Role::Uploader, Some(10), None)
            .is_empty());
        contract.own_propose_owner(Some(new.clone()));
        contract.own_propose_owner(None);
        assert_eq!(contract.own_get_proposed_owner(), None);
        contract.own_propose_owner(Some(new.clone()));
        testing_env!(VMContextBuilder::new()
            .predecessor_account_id(new.clone())
            .attached_deposit(NearToken::from_yoctonear(1))
            .build());
        contract.own_accept_owner();
        assert_eq!(contract.own_get_owner(), Some(new.clone()));
        assert_eq!(contract.own_get_proposed_owner(), None);
        assert!(contract.has_role(old.clone(), Role::Uploader));
        assert!(!contract.has_role(old.clone(), Role::Deployer));
        assert!(!contract.has_role(new.clone(), Role::Uploader));
        assert!(!contract.has_role(new, Role::Deployer));
        assert_eq!(contract.list_role(Role::Uploader, None, None), [old]);
    }

    const STORED: &str = "market@1.5.0";
    const REMOVED: &str = "market@1.0.0";
    const GLOBAL: &str = "oracle@0.4.1";

    fn contract() -> Contract {
        testing_env!(VMContextBuilder::new().build());
        let mut contract = Contract::new();
        let payer = env::predecessor_account_id();
        fund_storage(&mut contract, payer);
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

    /// A fresh receipt as the owner with one yocto attached, which also clears earlier logs.
    fn next_receipt(promise_results: Vec<PromiseResult>) {
        testing_env!(
            VMContextBuilder::new()
                .attached_deposit(NearToken::from_yoctonear(1))
                .build(),
            near_sdk::test_vm_config(),
            near_sdk::RuntimeFeesConfig::test(),
            std::collections::HashMap::default(),
            promise_results,
        );
    }

    fn emitted_events() -> Vec<serde_json::Value> {
        get_logs()
            .iter()
            .filter_map(|log| log.strip_prefix("EVENT_JSON:"))
            .map(|event| serde_json::from_str(event).unwrap())
            .collect()
    }

    fn deployment() -> Deployment {
        Deployment {
            version_key: STORED.to_string(),
            code_hash: [1u8; 32].into(),
            block_height: 1.into(),
        }
    }

    #[test]
    fn adding_stored_code_emits_version_added() {
        let mut contract = contract();
        next_receipt(Vec::new());
        let code = vec![0xbbu8; 10];

        let _ = contract.add_version(
            "new@1.0.0".to_string(),
            VersionSource::Stored(Base64VecU8(code.clone())),
        );

        let expected = RegistryEvent::VersionAdded {
            version_key: "new@1.0.0".to_string(),
            info: VersionInfo {
                code_hash: env::sha256_array(&code).into(),
                availability: VersionAvailability::Stored { code_len: 10 },
            },
        };
        assert_eq!(emitted_events(), vec![expected.to_json()]);
    }

    /// A global version is announced only once the publish or probe receipt succeeded; a failed one
    /// is rolled back without ever having been visible.
    #[rstest]
    #[case::confirmed(PromiseResult::Successful(Vec::new()), true)]
    #[case::rolled_back(PromiseResult::Failed, false)]
    fn add_version_finalize_emits_only_on_success(
        #[case] result: PromiseResult,
        #[case] added: bool,
    ) {
        let mut contract = contract();
        next_receipt(vec![result]);

        let _ = contract.add_version_01_finalize(GLOBAL.to_string());

        let expected = RegistryEvent::VersionAdded {
            version_key: GLOBAL.to_string(),
            info: VersionInfo {
                code_hash: [3u8; 32].into(),
                availability: VersionAvailability::Global,
            },
        };
        assert_eq!(
            emitted_events(),
            if added {
                vec![expected.to_json()]
            } else {
                Vec::new()
            },
        );
        assert_eq!(contract.get_version(GLOBAL.to_string()).is_some(), added);
    }

    /// Only a removal that clears code is announced: repeating one, or naming an unknown key,
    /// changes nothing.
    #[rstest]
    #[case::stored(STORED, true)]
    #[case::already_removed(REMOVED, false)]
    #[case::unknown("nothing@0.0.0", false)]
    fn remove_version_emits_only_when_it_clears_code(#[case] key: &str, #[case] removed: bool) {
        let mut contract = contract();
        next_receipt(Vec::new());

        contract.remove_version(key.to_string());

        let expected = RegistryEvent::VersionRemoved {
            version_key: key.to_string(),
            code_hash: [1u8; 32].into(),
        };
        assert_eq!(
            emitted_events(),
            if removed {
                vec![expected.to_json()]
            } else {
                Vec::new()
            },
        );
    }

    #[rstest]
    #[case::initialized(PromiseResult::Successful(Vec::new()), true)]
    #[case::released(PromiseResult::Failed, false)]
    fn deploy_finalize_emits_only_on_success(
        #[case] result: PromiseResult,
        #[case] deployed: bool,
    ) {
        let mut contract = contract();
        let account_id: AccountId = "one.registry.near".parse().unwrap();
        contract
            .registry
            .insert(account_id.clone(), RegistryEntry::Reserved);
        next_receipt(vec![result]);

        let _ =
            contract.deploy_01_finalize(Some(account_id.clone()), None, deployment(), None, None);

        let expected = RegistryEvent::Deployed {
            account_id: account_id.clone(),
            deployment: deployment(),
            deployer: None,
            full_access_keys: None,
        };
        assert_eq!(
            emitted_events(),
            if deployed {
                vec![expected.to_json()]
            } else {
                Vec::new()
            },
        );
        assert_eq!(
            contract.get_registry_entry(account_id),
            deployed.then(|| RegistryEntryView::Deployed(deployment())),
        );
    }

    #[test]
    #[should_panic(expected = "Missing deployment account ID")]
    fn finalize_needs_the_account_under_either_name() {
        let _ = contract().deploy_01_finalize(
            None,
            None,
            Deployment {
                version_key: STORED.to_string(),
                code_hash: [1u8; 32].into(),
                block_height: 1.into(),
            },
            None,
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
