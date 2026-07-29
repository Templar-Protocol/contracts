use super::*;

#[test]
fn export_env_uses_manifest_contracts() {
    let mut manifest = Manifest::new("testnet", None);
    manifest.contracts.insert(
        "vault".to_string(),
        ContractRecord {
            contract_id: "CV".to_string(),
            wasm_hash: "h".to_string(),
            salt: None,
            constructor_args: BTreeMap::new(),
            deploy_tx: None,
            initialized: true,
        },
    );
    let mut custodial = imported_record("CCUSTODIAL0");
    custodial.constructor_args = map_args([("custodian", ACCOUNT), ("asset", CONTRACT)]);
    manifest
        .contracts
        .insert("custodial_adapter_0".to_string(), custodial);
    assert!(export_env(&manifest).contains(&("SOROBAN_CONTRACT_ID".to_string(), "CV".to_string())));
    assert!(
        export_env(&manifest).contains(&("CUSTODIAL_0_ADDRESS".to_string(), ACCOUNT.to_string()))
    );
    assert!(!export_env(&manifest)
        .iter()
        .any(|(key, _)| key == "CUSTODIAL_ADDRESS"));
}

#[test]
fn status_ignores_unindexed_custodial_adapter_record() {
    let mut manifest = Manifest::new("testnet", None);
    let mut unindexed = imported_record("CLEGACY");
    unindexed.constructor_args = map_args([("custodian", ACCOUNT), ("asset", CONTRACT)]);
    manifest
        .contracts
        .insert("custodial_adapter".to_string(), unindexed);

    assert!(status_response(&manifest).custodial_adapters.is_empty());
    assert!(!export_env(&manifest)
        .iter()
        .any(|(key, value)| key.starts_with("CUSTODIAL") || value == "CLEGACY"));

    let mut indexed = imported_record("CCUSTODIAL0");
    indexed.constructor_args = map_args([("custodian", ACCOUNT), ("asset", CONTRACT)]);
    manifest
        .contracts
        .insert("custodial_adapter_0".to_string(), indexed);

    let status = status_response(&manifest);
    assert_eq!(status.custodial_adapters.len(), 1);
    assert_eq!(status.custodial_adapters[0].key, "custodial_adapter_0");
    assert_eq!(status.custodial_adapters[0].contract_id, "CCUSTODIAL0");
    assert!(export_env(&manifest).contains(&(
        "CUSTODIAL_ADAPTER_ID".to_string(),
        "CCUSTODIAL0".to_string()
    )));
}

#[test]
fn mainnet_write_requires_explicit_allow_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cli = base_cli(
        dir.path().join("manifest.json"),
        Commands::User(UserArgs {
            command: UserCommand::Deposit {
                operator: ACCOUNT.parse().expect("operator"),
                receiver: None,
                assets: None,
                assets_raw: Some(1),
                asset_decimals: 7,
                min_shares_out: None,
                min_shares_out_raw: 0,
                share_decimals: ShareDecimalsArg::Manifest,
            },
        }),
    );
    let cli = Cli {
        network: "mainnet".to_string(),
        ..cli
    };

    let err = run(&cli, &RecordingExecutor::new()).expect_err("mainnet write blocked");
    assert!(err.to_string().contains("mainnet write blocked"));
}

#[test]
fn fresh_state_rejects_non_stack_commands() {
    let cli = Cli {
        fresh_state: true,
        ..base_cli("manifest.json".into(), Commands::Status)
    };

    let error = guard_fresh_state_usage(&cli).expect_err("unsupported command must fail");

    assert!(error
        .to_string()
        .contains("--fresh-state is only valid with"));
}

#[test]
fn machine_readable_governance_changes_require_yes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance(&state);
    let cli = Cli {
        json: true,
        command: Commands::Governance(GovernanceArgs {
            command: GovernanceCommand::SubmitSetAdmin {
                admin: ACCOUNT.parse().expect("admin"),
                new_admin: OTHER_CONTRACT.parse().expect("new admin"),
            },
        }),
        ..base_cli(state, Commands::Status)
    };
    let executor = RecordingExecutor::new();

    let error = run(&cli, &executor).expect_err("machine-readable write must fail closed");

    assert!(error
        .to_string()
        .contains("dangerous governance change requires --yes"));
    assert!(submitted_calls(&executor.calls()).is_empty());
}
