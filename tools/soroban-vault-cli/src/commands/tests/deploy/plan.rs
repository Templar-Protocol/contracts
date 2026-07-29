use super::*;

#[test]
fn deploy_plan_does_not_execute_or_write_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_stack_wasms(dir.path());
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let before = fs::read_to_string(&state).expect("read manifest");
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Plan(crate::cli::DeployPlanArgs {
                command: crate::cli::DeployPlanCommand::Stack(Box::new(DeployStackArgs {
                    admin: Some(ACCOUNT.parse().expect("admin")),
                    asset_token: Some(CONTRACT.parse().expect("asset token")),
                    governance_timelock_ns: Some(1_000),
                    virtual_shares: 0,
                    virtual_assets: 0,
                    share_name: "Templar Vault Share".to_string(),
                    share_symbol: "tvSHARE".to_string(),
                    share_decimals: 7,
                    blend_pools: vec![CONTRACT.parse().expect("pool")],
                    custodians: Vec::new(),
                    adapter_admin: Some(OTHER_CONTRACT.parse().expect("adapter admin")),
                    build: false,
                    force_new: false,
                })),
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("deploy plan");

    assert!(executor.calls().is_empty());
    let after = fs::read_to_string(&state).expect("read manifest");
    assert_eq!(before, after);
}

#[test]
fn force_new_stack_plan_requires_explicit_governance_timelock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let mut stack = test_deploy_stack_args(ACCOUNT);
    stack.force_new = true;
    stack.governance_timelock_ns = None;
    let cli = base_cli(
        state,
        Commands::Deploy(DeployArgs {
            command: DeployCommand::Plan(crate::cli::DeployPlanArgs {
                command: DeployPlanCommand::Stack(Box::new(stack)),
            }),
        }),
    );
    let executor = RecordingExecutor::new();

    let error = run(&cli, &executor).expect_err("invalid force-new plan must fail");

    assert!(error
        .to_string()
        .contains("new governance deployment requires --governance-timelock-ns"));
    assert!(executor.calls().is_empty());
}

#[test]
fn deploy_plan_uses_unique_keys_for_multiple_custodians() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("asset_token".to_string(), imported_record(CONTRACT));
    let cli = base_cli(state, Commands::Status);
    let plan = deploy_adapters_plan(
        &cli,
        &manifest,
        &crate::cli::DeployAdaptersArgs {
            vault: None,
            governance: None,
            asset_token: None,
            blend_pools: Vec::new(),
            custodians: vec![
                ACCOUNT.parse().expect("first custodian"),
                ACCOUNT.parse().expect("duplicate custodian"),
                CONTRACT.parse().expect("second custodian"),
            ],
            adapter_admin: ACCOUNT.parse().expect("adapter admin"),
            build: false,
            force_new: false,
        },
    )
    .expect("plan adapters");

    let custodial_keys = plan
        .contracts_to_deploy
        .iter()
        .filter_map(|contract| {
            contract
                .key
                .starts_with("custodial_adapter_")
                .then_some(contract.key.as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        custodial_keys,
        vec!["custodial_adapter_0", "custodial_adapter_1"]
    );
}

#[test]
fn deploy_plan_uses_unique_keys_for_multiple_blend_pools() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(CONTRACT));
    let cli = base_cli(state, Commands::Status);
    let plan = deploy_adapters_plan(
        &cli,
        &manifest,
        &crate::cli::DeployAdaptersArgs {
            vault: None,
            governance: None,
            asset_token: None,
            blend_pools: vec![
                ACCOUNT.parse().expect("first pool"),
                CONTRACT.parse().expect("second pool"),
            ],
            custodians: Vec::new(),
            adapter_admin: OTHER_CONTRACT.parse().expect("adapter admin"),
            build: false,
            force_new: false,
        },
    )
    .expect("plan adapters");

    let blend_keys = plan
        .contracts_to_deploy
        .iter()
        .filter_map(|contract| {
            contract
                .key
                .starts_with("blend_adapter_")
                .then_some(contract.key.as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(blend_keys, vec!["blend_adapter_0", "blend_adapter_1"]);
}

#[test]
fn deploy_plan_records_companion_upgrade_check_for_vault_admin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(OTHER_CONTRACT));
    let cli = base_cli(state, Commands::Status);

    let plan = deploy_adapters_plan(
        &cli,
        &manifest,
        &crate::cli::DeployAdaptersArgs {
            vault: None,
            governance: None,
            asset_token: None,
            blend_pools: vec![ACCOUNT.parse().expect("pool")],
            custodians: Vec::new(),
            adapter_admin: "vault".parse().expect("vault adapter admin"),
            build: false,
            force_new: false,
        },
    )
    .expect("plan adapters");

    assert!(plan
        .stellar_commands
        .iter()
        .any(|command| command.contains("--send no -- version")));
    assert!(plan
        .stellar_commands
        .iter()
        .any(|command| command.contains(&format!("--admin {CONTRACT}"))));
    assert!(plan
        .warnings
        .iter()
        .any(|warning| warning.contains("companion-upgrade capability 0x40")));
}

#[test]
fn fresh_plan_rejects_existing_manifest_without_writing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    Manifest::new("testnet", None)
        .save(&state)
        .expect("save existing manifest");
    let cli = Cli {
        fresh_state: true,
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Plan(crate::cli::DeployPlanArgs {
                command: DeployPlanCommand::Stack(Box::new(test_deploy_stack_args(ACCOUNT))),
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    let error = run(&cli, &executor).expect_err("existing path must be rejected");

    assert!(error
        .to_string()
        .contains("fresh deployment requires an unused --state path"));
    assert!(executor.calls().is_empty());
}
