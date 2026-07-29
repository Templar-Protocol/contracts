use super::*;

#[test]
fn deploy_contract_helper_checkpoints_contract_record_immediately() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let cli = base_cli(state.clone(), Commands::Status);
    let executor = RecordingExecutor::new();
    let context = CommandContext::new(&cli, &executor);
    let mut manifest = Manifest::new("testnet", None);

    let contract_id = deploy_contract_if_needed(
        &context,
        &mut manifest,
        ContractDeployment {
            key: "vault",
            wasm_hash: "abc123",
            constructor_args: Vec::new(),
            constructor_summary: BTreeMap::new(),
            force_new: false,
            initialization: InitializationState::Pending,
        },
    )
    .expect("deploy contract");

    let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    let record = loaded.contracts.get("vault").expect("vault record");
    assert_eq!(contract_id, CONTRACT);
    assert_eq!(record.contract_id, CONTRACT);
    assert!(!record.initialized);
}

#[test]
fn deploy_contract_helper_marks_constructor_deployments_initialized() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let cli = base_cli(state.clone(), Commands::Status);
    let executor = RecordingExecutor::new();
    let context = CommandContext::new(&cli, &executor);
    let mut manifest = Manifest::new("testnet", None);

    deploy_contract_if_needed(
        &context,
        &mut manifest,
        ContractDeployment {
            key: "governance",
            wasm_hash: "abc123",
            constructor_args: args([("--admin", ACCOUNT), ("--vault", CONTRACT)]),
            constructor_summary: map_args([("admin", ACCOUNT), ("vault", CONTRACT)]),
            force_new: false,
            initialization: InitializationState::Complete,
        },
    )
    .expect("deploy contract");

    let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    let record = loaded
        .contracts
        .get("governance")
        .expect("governance record");
    assert!(record.initialized);
}

#[test]
fn initialize_proxy_helper_checkpoints_initialized_state_immediately() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let cli = base_cli(state.clone(), Commands::Status);
    let executor = RecordingExecutor::new();
    let context = CommandContext::new(&cli, &executor);
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("proxy_4626".to_string(), uninitialized_record(CONTRACT));
    manifest.save(&state).expect("save manifest");

    initialize_proxy_if_needed(&context, &mut manifest, "proxy_4626", CONTRACT, Vec::new())
        .expect("initialize proxy");

    let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    assert!(
        loaded
            .contracts
            .get("proxy_4626")
            .expect("proxy record")
            .initialized
    );
}

#[test]
fn initialize_proxy_requires_a_manifest_record_before_invocation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let cli = base_cli(state, Commands::Status);
    let executor = RecordingExecutor::new();
    let context = CommandContext::new(&cli, &executor);
    let mut manifest = Manifest::new("testnet", None);

    let error =
        initialize_proxy_if_needed(&context, &mut manifest, "proxy_4626", CONTRACT, Vec::new())
            .expect_err("missing manifest record must fail");

    assert!(error
        .to_string()
        .contains("proxy_4626 deployment was not recorded in manifest"));
    assert!(executor.calls().is_empty());
}

#[test]
fn initialize_vault_requires_a_manifest_record_before_invocation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let cli = base_cli(state, Commands::Status);
    let executor = RecordingExecutor::new();
    let context = CommandContext::new(&cli, &executor);
    let mut manifest = Manifest::new("testnet", None);

    let error = initialize_vault_if_needed(
        &context,
        &mut manifest,
        CONTRACT,
        ACCOUNT,
        CONTRACT,
        CONTRACT,
        CONTRACT,
        0,
        0,
    )
    .expect_err("missing manifest record must fail");

    assert!(error
        .to_string()
        .contains("vault deployment was not recorded in manifest"));
    assert!(executor.calls().is_empty());
}
