use super::*;

#[test]
fn cutover_guard_blocks_unverified_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(CONTRACT));
    manifest.save(&state).expect("save manifest");
    let cli = Cli {
        yes: true,
        workspace_path: dir.path().into(),
        command: Commands::Governance(GovernanceArgs {
            command: GovernanceCommand::SubmitSetGovernance {
                admin: ACCOUNT.parse().expect("admin"),
                new_governance: OTHER_CONTRACT.parse().expect("new governance"),
                expected_admin: None,
                expected_wasm_hash: None,
                allow_unverified_governance: false,
            },
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    // RecordingExecutor answers every unknown invoke with CONTRACT, so the
    // target vault() view reports CONTRACT, not the manifest vault: the
    // guard must refuse submission.
    let executor = RecordingExecutor::new();

    let err = run(&cli, &executor).expect_err("guard must refuse unverified target");

    assert!(format!("{err:#}").contains("cutover guard refused"));
    let submitted = submitted_calls(&executor.calls());
    assert!(
        !submitted
            .iter()
            .any(|(_, args)| args.iter().any(|arg| arg == "submit_set_governance")),
        "guard must block the proposal submission"
    );
}

#[test]
fn cutover_guard_override_allows_submission() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let cli = Cli {
        yes: true,
        workspace_path: dir.path().into(),
        command: Commands::Governance(GovernanceArgs {
            command: GovernanceCommand::SubmitSetGovernance {
                admin: ACCOUNT.parse().expect("admin"),
                new_governance: OTHER_CONTRACT.parse().expect("new governance"),
                expected_admin: None,
                expected_wasm_hash: None,
                allow_unverified_governance: true,
            },
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("override flag submits");

    let submitted = submitted_calls(&executor.calls());
    assert!(submitted
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "submit_set_governance")));
}

#[test]
fn promotion_moves_pending_to_governance_and_keeps_previous() {
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(CONTRACT));
    manifest.contracts.insert(
        crate::commands::cutover::GOVERNANCE_PENDING_KEY.to_string(),
        imported_record(OTHER_CONTRACT),
    );
    let promoted = crate::commands::cutover::promote_pending_governance(&mut manifest)
        .expect("promote pending governance");
    assert_eq!(promoted, OTHER_CONTRACT);
    assert_eq!(manifest.contracts["governance"].contract_id, OTHER_CONTRACT);
    assert_eq!(
        manifest
            .contracts
            .get(crate::commands::cutover::GOVERNANCE_PREVIOUS_KEY)
            .expect("previous governance")
            .contract_id,
        CONTRACT
    );
    assert!(!manifest
        .contracts
        .contains_key(crate::commands::cutover::GOVERNANCE_PENDING_KEY));
}
