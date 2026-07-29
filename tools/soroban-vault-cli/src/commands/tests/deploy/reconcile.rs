use super::*;

#[test]
fn reconcile_classifies_matching_recorded_contract_as_initialized() {
    let wasm_hash = format!("{:x}", Sha256::digest(b"vault wasm"));
    let mut manifest = Manifest::new("testnet", None);
    manifest.contracts.insert(
        "vault".to_string(),
        ContractRecord {
            contract_id: CONTRACT.to_string(),
            wasm_hash: wasm_hash.clone(),
            salt: None,
            constructor_args: BTreeMap::new(),
            deploy_tx: None,
            initialized: true,
        },
    );
    let cli = base_cli("manifest.json".into(), Commands::Status);
    let executor = ChainStateExecutor {
        wasm: b"vault wasm",
    };
    let stellar = Stellar::new(&cli, &executor);

    let response = reconcile_manifest(&stellar, &manifest, false);

    let vault = response
        .components
        .iter()
        .find(|component| component.key == "vault")
        .expect("vault component");
    assert_eq!(vault.status, ReconcileStatus::Initialized);
    assert_eq!(vault.chain_wasm_hash.as_deref(), Some(wasm_hash.as_str()));
    assert!(response.safe_to_resume);
}

#[test]
fn reconcile_detects_wasm_hash_mismatch_and_blocks_resume() {
    let mut manifest = Manifest::new("testnet", None);
    manifest.contracts.insert(
        "vault".to_string(),
        ContractRecord {
            contract_id: CONTRACT.to_string(),
            wasm_hash: "different".to_string(),
            salt: None,
            constructor_args: BTreeMap::new(),
            deploy_tx: None,
            initialized: true,
        },
    );
    let cli = base_cli("manifest.json".into(), Commands::Status);
    let executor = ChainStateExecutor {
        wasm: b"vault wasm",
    };
    let stellar = Stellar::new(&cli, &executor);

    let response = reconcile_manifest(&stellar, &manifest, false);

    let vault = response
        .components
        .iter()
        .find(|component| component.key == "vault")
        .expect("vault component");
    assert_eq!(vault.status, ReconcileStatus::Mismatched);
    assert!(!response.safe_to_resume);
    assert!(response.drift_detected);
}

#[test]
fn reconcile_probes_stellar_asset_contract_without_fetching_wasm() {
    let mut manifest = Manifest::new("testnet", None);
    manifest.contracts.insert(
        "asset_token".to_string(),
        ContractRecord {
            contract_id: ASSET_CONTRACT.to_string(),
            wasm_hash: "stellar-asset-contract".to_string(),
            salt: None,
            constructor_args: map_args([("asset", "native")]),
            deploy_tx: None,
            initialized: true,
        },
    );
    let cli = base_cli("manifest.json".into(), Commands::Status);
    let executor = RecordingExecutor::new();
    let stellar = Stellar::new(&cli, &executor);

    let response = reconcile_manifest(&stellar, &manifest, false);

    let asset = response
        .components
        .iter()
        .find(|component| component.key == "asset_token")
        .expect("asset-token component");
    assert_eq!(asset.status, ReconcileStatus::Initialized);
    assert_eq!(asset.chain_wasm_hash, None);
    assert!(response.safe_to_resume);
    let calls = executor.calls();
    assert!(calls
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "decimals")));
    assert!(calls.iter().any(|(_, args)| {
        matches!(args.as_slice(), [contract, id, asset, ..] if contract == "contract" && id == "id" && asset == "asset")
            && args
                .windows(2)
                .any(|pair| pair == ["--asset", "native"])
    }));
    assert!(!calls.iter().any(|(_, args)| {
        matches!(args.as_slice(), [contract, fetch, ..] if contract == "contract" && fetch == "fetch")
    }));
}

#[test]
fn reconcile_rejects_wrong_stellar_asset_contract_id() {
    let mut manifest = Manifest::new("testnet", None);
    manifest.contracts.insert(
        "asset_token".to_string(),
        ContractRecord {
            contract_id: OTHER_CONTRACT.to_string(),
            wasm_hash: "stellar-asset-contract".to_string(),
            salt: None,
            constructor_args: map_args([("asset", "native")]),
            deploy_tx: None,
            initialized: true,
        },
    );
    let cli = base_cli("manifest.json".into(), Commands::Status);
    let executor = RecordingExecutor::new();
    let stellar = Stellar::new(&cli, &executor);

    let response = reconcile_manifest(&stellar, &manifest, false);

    let asset = response
        .components
        .iter()
        .find(|component| component.key == "asset_token")
        .expect("asset-token component");
    assert_eq!(asset.status, ReconcileStatus::Mismatched);
    assert!(!response.safe_to_resume);
    assert!(asset
        .warnings
        .iter()
        .any(|warning| warning.contains(ASSET_CONTRACT)));
    assert!(!executor
        .calls()
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "decimals")));
}

#[test]
fn reconcile_keeps_stellar_asset_without_descriptor_unknown() {
    let mut manifest = Manifest::new("testnet", None);
    manifest.contracts.insert(
        "asset_token".to_string(),
        ContractRecord {
            contract_id: ASSET_CONTRACT.to_string(),
            wasm_hash: "stellar-asset-contract".to_string(),
            salt: None,
            constructor_args: BTreeMap::new(),
            deploy_tx: None,
            initialized: true,
        },
    );
    let cli = base_cli("manifest.json".into(), Commands::Status);
    let executor = RecordingExecutor::new();
    let stellar = Stellar::new(&cli, &executor);

    let response = reconcile_manifest(&stellar, &manifest, false);

    let asset = response
        .components
        .iter()
        .find(|component| component.key == "asset_token")
        .expect("asset-token component");
    assert_eq!(asset.status, ReconcileStatus::Unknown);
    assert!(!response.safe_to_resume);
    assert!(asset
        .warnings
        .iter()
        .any(|warning| warning.contains("no canonical asset descriptor")));
    assert!(executor.calls().is_empty());
}

#[test]
fn reconcile_blocks_resume_when_stellar_asset_contract_probe_fails() {
    let mut manifest = Manifest::new("testnet", None);
    manifest.contracts.insert(
        "asset_token".to_string(),
        ContractRecord {
            contract_id: ASSET_CONTRACT.to_string(),
            wasm_hash: "stellar-asset-contract".to_string(),
            salt: None,
            constructor_args: map_args([("asset", "native")]),
            deploy_tx: None,
            initialized: true,
        },
    );
    let cli = base_cli("manifest.json".into(), Commands::Status);
    let executor = ChainStateExecutor {
        wasm: b"unreachable",
    };
    let stellar = Stellar::new(&cli, &executor);

    let response = reconcile_manifest(&stellar, &manifest, false);

    let asset = response
        .components
        .iter()
        .find(|component| component.key == "asset_token")
        .expect("asset-token component");
    assert_eq!(asset.status, ReconcileStatus::Missing);
    assert!(!response.safe_to_resume);
}

#[test]
fn reconcile_verifies_vault_version_for_capable_curator_proxy() {
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(CONTRACT));
    let mut proxy = imported_record(CONTRACT);
    proxy.constructor_args.insert(
        CURATOR_PROXY_VERSION_DISCOVERY_ARG.to_string(),
        "true".to_string(),
    );
    manifest
        .contracts
        .insert("curator_proxy".to_string(), proxy);
    let cli = base_cli("manifest.json".into(), Commands::Status);
    let executor = TtlRecordingExecutor::new();
    let stellar = Stellar::new(&cli, &executor);

    let response = reconcile_manifest(&stellar, &manifest, true);

    let proxy = response
        .components
        .iter()
        .find(|component| component.key == "curator_proxy")
        .expect("curator proxy component");
    assert_eq!(proxy.status, ReconcileStatus::Initialized);
    assert!(proxy
        .wiring
        .iter()
        .any(|check| { check.field == "vault_version" && check.status == WiringStatus::Match }));
    assert!(executor
        .calls()
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "vault_version")));
}

#[test]
fn reconcile_does_not_require_vault_version_for_legacy_proxy_manifest() {
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("curator_proxy".to_string(), imported_record(CONTRACT));
    let cli = base_cli("manifest.json".into(), Commands::Status);
    let executor = TtlRecordingExecutor::new();
    let stellar = Stellar::new(&cli, &executor);

    let response = reconcile_manifest(&stellar, &manifest, true);

    let proxy = response
        .components
        .iter()
        .find(|component| component.key == "curator_proxy")
        .expect("curator proxy component");
    assert_eq!(proxy.status, ReconcileStatus::Initialized);
    assert!(!proxy
        .wiring
        .iter()
        .any(|check| check.field == "vault_version"));
    assert!(!executor
        .calls()
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "vault_version")));
}

#[test]
fn current_initialized_proxy_without_marker_still_needs_version_verification() {
    let mut proxy = imported_record(CONTRACT);
    proxy.wasm_hash = "current-proxy-hash".to_string();
    proxy.initialized = true;

    assert!(curator_proxy_needs_version_verification(
        &proxy,
        "current-proxy-hash"
    ));

    proxy.constructor_args.insert(
        CURATOR_PROXY_VERSION_DISCOVERY_ARG.to_string(),
        "true".to_string(),
    );
    assert!(!curator_proxy_needs_version_verification(
        &proxy,
        "current-proxy-hash"
    ));
}

#[test]
fn stack_reverification_preserves_legacy_initializer_provenance() {
    let legacy_hash = "11".repeat(32);
    let mut proxy = imported_record(CONTRACT);
    proxy.wasm_hash = "current-proxy-hash".to_string();
    proxy.constructor_args.insert(
        CURATOR_PROXY_INITIALIZER_ARG.to_string(),
        "initialize_legacy_v1".to_string(),
    );
    proxy.constructor_args.insert(
        CURATOR_PROXY_LEGACY_V1_HASH_ARG.to_string(),
        legacy_hash.clone(),
    );
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("curator_proxy".to_string(), proxy);

    record_standard_curator_proxy_initialization_if_missing(
        &mut manifest,
        "different-vault",
        "different-governance",
    )
    .expect("preserve existing initialization provenance");

    let proxy = manifest
        .contracts
        .get("curator_proxy")
        .expect("curator proxy record");
    assert_eq!(
        proxy.constructor_args.get(CURATOR_PROXY_INITIALIZER_ARG),
        Some(&"initialize_legacy_v1".to_string())
    );
    assert_eq!(
        proxy.constructor_args.get(CURATOR_PROXY_LEGACY_V1_HASH_ARG),
        Some(&legacy_hash)
    );
    assert!(!proxy
        .constructor_args
        .contains_key(CURATOR_PROXY_VERSION_DISCOVERY_ARG));
}

#[test]
fn reconcile_blocks_capable_curator_proxy_when_vault_version_fails() {
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(CONTRACT));
    let mut proxy = imported_record(CONTRACT);
    proxy.constructor_args.insert(
        CURATOR_PROXY_VERSION_DISCOVERY_ARG.to_string(),
        "true".to_string(),
    );
    manifest
        .contracts
        .insert("curator_proxy".to_string(), proxy);
    let cli = base_cli("manifest.json".into(), Commands::Status);
    let executor = FailingVaultVersionExecutor::new();
    let stellar = Stellar::new(&cli, &executor);

    let response = reconcile_manifest(&stellar, &manifest, true);

    let proxy = response
        .components
        .iter()
        .find(|component| component.key == "curator_proxy")
        .expect("curator proxy component");
    assert_eq!(proxy.status, ReconcileStatus::Unknown);
    assert!(!response.safe_to_resume);
    assert!(proxy
        .warnings
        .iter()
        .any(|warning| warning.contains("vault_version")));
    assert!(executor
        .calls()
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "vault_version")));
}

#[test]
fn resume_repair_marks_chain_initialized_manifest_records_initialized() {
    let wasm_hash = format!("{:x}", Sha256::digest(b"share wasm"));
    let mut manifest = Manifest::new("testnet", None);
    manifest.contracts.insert(
        "vault".to_string(),
        ContractRecord {
            contract_id: CONTRACT.to_string(),
            wasm_hash: "predeployed".to_string(),
            salt: None,
            constructor_args: BTreeMap::new(),
            deploy_tx: None,
            initialized: true,
        },
    );
    manifest.contracts.insert(
        "share_token".to_string(),
        ContractRecord {
            contract_id: CONTRACT.to_string(),
            wasm_hash,
            salt: None,
            constructor_args: map_args([("vault", CONTRACT), ("admin", CONTRACT)]),
            deploy_tx: None,
            initialized: false,
        },
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let cli = base_cli(state.clone(), Commands::Status);
    let executor = ChainStateExecutor {
        wasm: b"share wasm",
    };
    let context = CommandContext::new(&cli, &executor);

    let response = reconcile_manifest(context.stellar(), &manifest, true);
    let share = response
        .components
        .iter()
        .find(|component| component.key == "share_token")
        .expect("share component");
    assert_eq!(share.status, ReconcileStatus::Initialized);
    assert!(!share.manifest_initialized);
    assert!(!share.warnings.is_empty());

    apply_reconcile_safe_manifest_updates(&context, &mut manifest, &response)
        .expect("apply safe repair");

    let share = manifest
        .contracts
        .get("share_token")
        .expect("share token record");
    assert!(share.initialized);
    let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    assert!(
        loaded
            .contracts
            .get("share_token")
            .expect("saved share token record")
            .initialized
    );
}

#[test]
fn share_token_reconciliation_checks_only_immutable_vault_binding() {
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    let mut share_token = imported_record(CONTRACT);
    share_token.constructor_args = map_args([("admin", ACCOUNT), ("vault", CONTRACT)]);
    manifest
        .contracts
        .insert("share_token".to_string(), share_token.clone());
    let cli = base_cli("manifest.json".into(), Commands::Status);
    let executor = RecordingExecutor::new();
    let stellar = Stellar::new(&cli, &executor);

    let checks = verify_component_wiring(&stellar, &manifest, "share_token", &share_token)
        .expect("verify share-token wiring");

    assert!(checks
        .iter()
        .any(|check| { check.field == "vault" && check.status == WiringStatus::Match }));
    assert!(!checks.iter().any(|check| check.field == "admin"));
}

#[test]
fn unknown_wiring_does_not_mark_an_uninitialized_component_initialized() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    let mut share_token = imported_record(CONTRACT);
    share_token.initialized = false;
    manifest
        .contracts
        .insert("share_token".to_string(), share_token);
    manifest.save(&state).expect("save manifest");
    let cli = base_cli(state.clone(), Commands::Status);
    let executor = ChainStateExecutor {
        wasm: b"share token wasm",
    };
    let context = CommandContext::new(&cli, &executor);

    let response = reconcile_manifest(context.stellar(), &manifest, true);
    let share = response
        .components
        .iter()
        .find(|component| component.key == "share_token")
        .expect("share component");
    assert_eq!(share.status, ReconcileStatus::Deployed);
    assert!(!share.wiring.is_empty());
    assert!(share
        .wiring
        .iter()
        .all(|check| check.status == WiringStatus::Unknown));

    apply_reconcile_safe_manifest_updates(&context, &mut manifest, &response)
        .expect("apply safe updates");
    assert!(
        !manifest
            .contracts
            .get("share_token")
            .expect("share token record")
            .initialized
    );
}
