use super::*;
use rstest::{fixture, rstest};

#[test]
fn test_storage_version() {
    let v1 = StorageVersion::V1;
    assert_eq!(v1.number(), 1);
    assert!(v1.is_compatible());

    let current = StorageVersion::CURRENT;
    assert_eq!(current, StorageVersion::V1);
}

#[test]
fn test_versioned_state_new() {
    let state = VaultState::default();
    let versioned = VersionedState::new(state);

    assert_eq!(versioned.version, StorageVersion::CURRENT);
}

#[test]
fn test_memory_storage_empty() {
    let storage = MemoryStorage::new();
    assert!(!storage.is_initialized());
    assert!(storage.load_state().unwrap().is_none());
}

#[test]
fn test_memory_storage_save_load() {
    let mut storage = MemoryStorage::new();
    let state = VersionedState::default();

    storage.save_state(&state).unwrap();
    assert!(storage.is_initialized());

    let loaded = storage.load_state().unwrap();
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap(), state);
}

#[test]
fn test_memory_storage_with_state() {
    let state = VersionedState::default();
    let storage = MemoryStorage::with_state(state.clone());

    assert!(storage.is_initialized());
    assert_eq!(storage.get_state(), Some(&state));
}

#[test]
fn test_memory_storage_clear() {
    let state = VersionedState::default();
    let mut storage = MemoryStorage::with_state(state);

    storage.clear();
    assert!(!storage.is_initialized());
    assert!(storage.get_state().is_none());
}

#[test]
fn test_memory_storage_address_book_roundtrip() {
    use soroban_sdk::testutils::Address as _;

    let env = Env::default();
    let mut storage = MemoryStorage::new();
    let kernel_addr = [9u8; 32];
    let soroban_addr = SdkAddress::generate(&env);

    storage.save_address(&kernel_addr, &soroban_addr).unwrap();
    let loaded = storage.load_address(&kernel_addr).unwrap();
    assert_eq!(loaded, Some(soroban_addr));
}

#[test]
fn test_storage_key_variants() {
    let key1 = StorageKey::VaultState;
    let key2 = StorageKey::Version;
    let key3 = StorageKey::PendingWithdrawal(42);
    let key4 = StorageKey::ShareBalance([0u8; 32]);
    let key5 = StorageKey::TotalSupply;

    assert_ne!(key1, key2);
    assert_ne!(key3, key4);
    assert_ne!(key4, key5);
}

#[test]
fn test_soroban_storage_key_variants() {
    let env = Env::default();
    let key1 = SorobanStorageKey::StateBlob;
    let key2 = SorobanStorageKey::PolicyState;
    let key3 = SorobanStorageKey::Restrictions;
    let key4 = SorobanStorageKey::Version;
    let key5 = SorobanStorageKey::Config;
    let key6 = SorobanStorageKey::Paused;
    let key7 = SorobanStorageKey::AddressBook(BytesN::from_array(&env, &[0u8; 32]));

    // Keys should be distinct
    assert!(matches!(key1, SorobanStorageKey::StateBlob));
    assert!(matches!(key2, SorobanStorageKey::PolicyState));
    assert!(matches!(key3, SorobanStorageKey::Restrictions));
    assert!(matches!(key4, SorobanStorageKey::Version));
    assert!(matches!(key5, SorobanStorageKey::Config));
    assert!(matches!(key6, SorobanStorageKey::Paused));
    assert!(matches!(key7, SorobanStorageKey::AddressBook(_)));
}

// Helper to create a registered contract for storage tests
mod test_contract {
    use soroban_sdk::{contract, contractimpl, Env};

    #[contract]
    pub struct TestContract;

    #[contractimpl]
    impl TestContract {
        pub fn noop(_env: Env) {}
    }
}

#[fixture]
fn contract_env() -> (Env, soroban_sdk::Address) {
    let env = Env::default();
    let contract_id = env.register(test_contract::TestContract, ());
    (env, contract_id)
}

#[rstest]
fn test_soroban_storage_with_sdk_env(contract_env: (Env, soroban_sdk::Address)) {
    let (env, contract_id) = contract_env;
    env.as_contract(&contract_id, || {
        let storage = SorobanStorage::new(&env);

        // Fresh storage should not be initialized
        assert!(!storage.is_initialized());
        assert!(storage.get_version().is_none());
        assert!(storage.load_state_blob().is_none());

        // Save state
        let mut kernel = VaultState::default();
        kernel.total_assets = 10000;
        kernel.total_shares = 5000;
        kernel.idle_assets = 2000;
        kernel.external_assets = 8000;
        kernel.next_op_id = 1;
        let versioned = VersionedState::new(kernel);
        let mut storage_mut = SorobanStorage::new(&env);
        Storage::save_state(&mut storage_mut, &versioned).unwrap();

        // Now storage should be initialized
        assert!(storage.is_initialized());
        assert_eq!(
            storage.get_version(),
            Some(StorageVersion::CURRENT.number())
        );

        // Load and verify
        let loaded = storage.load_state().unwrap().unwrap();
        assert_eq!(loaded.state.total_assets, 10000);
        assert_eq!(loaded.state.total_shares, 5000);
        assert_eq!(loaded.state.idle_assets, 2000);
        assert_eq!(loaded.state.external_assets, 8000);
        assert_eq!(loaded.state.next_op_id, 1);
    });
}

#[rstest]
fn test_soroban_storage_pause_state(contract_env: (Env, soroban_sdk::Address)) {
    let (env, contract_id) = contract_env;
    env.as_contract(&contract_id, || {
        let storage = SorobanStorage::new(&env);

        // Default is not paused
        assert!(!storage.is_paused());

        // Set paused
        storage.set_paused(true);
        assert!(storage.is_paused());

        // Unset paused
        storage.set_paused(false);
        assert!(!storage.is_paused());
    });
}

#[rstest]
fn test_soroban_storage_roundtrip_op_state_and_queue(contract_env: (Env, soroban_sdk::Address)) {
    use alloc::collections::BTreeMap;
    use templar_vault_kernel::state::queue::{PendingWithdrawal, WithdrawQueue};
    use templar_vault_kernel::{OpState, WithdrawingState};

    let (env, contract_id) = contract_env;
    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);
        let mut state = VaultState::default();

        let owner = [1u8; 32];
        let receiver = [2u8; 32];
        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id: 7,
            index: 1,
            remaining: 500,
            collected: 200,
            receiver,
            owner,
            escrow_shares: 700,
        });

        let mut pending = BTreeMap::new();
        pending.insert(3, PendingWithdrawal::new(owner, receiver, 700, 800, 123));
        state.withdraw_queue = WithdrawQueue::with_state(pending, 3, 4);
        state.total_assets = 1000;
        state.total_shares = 900;
        state.idle_assets = 100;
        state.external_assets = 900;
        state.next_op_id = 8;

        let versioned = VersionedState::new(state.clone());
        storage.save_state(&versioned).unwrap();

        let loaded = storage.load_state().unwrap().unwrap();
        assert_eq!(loaded.state, state);
    });
}

#[rstest]
fn test_soroban_storage_implements_storage_trait(contract_env: (Env, soroban_sdk::Address)) {
    let (env, contract_id) = contract_env;
    env.as_contract(&contract_id, || {
        let mut storage = SorobanStorage::new(&env);

        // Test Storage trait methods
        assert!(!Storage::is_initialized(&storage));
        assert!(storage.load_state().unwrap().is_none());

        // Save state via trait
        let versioned = VersionedState::default();
        storage.save_state(&versioned).unwrap();

        // Verify via trait
        assert!(Storage::is_initialized(&storage));
        let loaded = storage.load_state().unwrap().unwrap();
        assert_eq!(loaded.version, StorageVersion::CURRENT);
    });
}

#[rstest]
fn test_storage_trait_get_version_fails_when_uninitialized(
    contract_env: (Env, soroban_sdk::Address),
) {
    let (env, contract_id) = contract_env;
    env.as_contract(&contract_id, || {
        let storage = SorobanStorage::new(&env);
        let err = Storage::get_version(&storage).unwrap_err();
        assert_eq!(err, RuntimeError::storage_error("version not initialized"));
    });
}

#[rstest]
fn test_soroban_storage_load_state_rejects_corrupted_blob(
    contract_env: (Env, soroban_sdk::Address),
) {
    let (env, contract_id) = contract_env;
    env.as_contract(&contract_id, || {
        let storage = SorobanStorage::new(&env);
        storage.save_state_blob(&alloc::vec![1, 2, 3, 4, 5]);

        let err = Storage::load_state(&storage).unwrap_err();
        assert_eq!(
            err,
            RuntimeError::storage_error("state blob deserialize failed")
        );
    });
}

#[rstest]
fn test_soroban_storage_load_state_rejects_trailing_bytes(
    contract_env: (Env, soroban_sdk::Address),
) {
    let (env, contract_id) = contract_env;
    env.as_contract(&contract_id, || {
        let storage = SorobanStorage::new(&env);
        let versioned = VersionedState::new(VaultState::default());
        let mut bytes = borsh::to_vec(&versioned).unwrap();
        bytes.push(0xff);
        storage.save_state_blob(&bytes);

        let err = Storage::load_state(&storage).unwrap_err();
        assert_eq!(
            err,
            RuntimeError::storage_error("state blob deserialize failed")
        );
    });
}

#[rstest]
fn test_soroban_storage_load_state_rejects_missing_version_key(
    contract_env: (Env, soroban_sdk::Address),
) {
    let (env, contract_id) = contract_env;
    env.as_contract(&contract_id, || {
        let storage = SorobanStorage::new(&env);
        let versioned = VersionedState::new(VaultState::default());
        let bytes = borsh::to_vec(&versioned).unwrap();
        storage.save_state_blob(&bytes);

        let err = Storage::load_state(&storage).unwrap_err();
        assert_eq!(err, RuntimeError::storage_error("state version missing"));
    });
}

#[rstest]
fn test_soroban_storage_load_state_rejects_mismatched_version(
    contract_env: (Env, soroban_sdk::Address),
) {
    let (env, contract_id) = contract_env;
    env.as_contract(&contract_id, || {
        let storage = SorobanStorage::new(&env);
        let versioned = VersionedState::new(VaultState::default());
        let bytes = borsh::to_vec(&versioned).unwrap();
        storage.save_state_blob(&bytes);
        storage.set_version(StorageVersion::new(2).number());

        let err = Storage::load_state(&storage).unwrap_err();
        assert_eq!(err, RuntimeError::storage_error("state version mismatch"));
    });
}

#[rstest]
fn test_soroban_storage_load_state_rejects_incompatible_version(
    contract_env: (Env, soroban_sdk::Address),
) {
    let (env, contract_id) = contract_env;
    env.as_contract(&contract_id, || {
        let storage = SorobanStorage::new(&env);
        let versioned = VersionedState::with_version(StorageVersion::new(2), VaultState::default());
        let bytes = borsh::to_vec(&versioned).unwrap();
        storage.save_state_blob(&bytes);
        storage.set_version(StorageVersion::new(2).number());

        let err = Storage::load_state(&storage).unwrap_err();
        assert_eq!(
            err,
            RuntimeError::storage_error("unsupported state version")
        );
    });
}

#[rstest]
#[case(StorageVersion::new(0), true)]
#[case(StorageVersion::V1, true)]
#[case(StorageVersion::new(2), false)]
#[case(StorageVersion::new(u32::MAX), false)]
fn test_storage_version_compatibility_cases(
    #[case] version: StorageVersion,
    #[case] expected: bool,
) {
    assert_eq!(version.is_compatible(), expected);
}
