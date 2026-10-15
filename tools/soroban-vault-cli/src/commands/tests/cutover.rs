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

#[test]
fn confirm_governance_cutover_uses_constructor_admin_never_governance_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    for key in ["vault", "governance"] {
        manifest
            .contracts
            .insert(key.to_string(), imported_record(CONTRACT));
    }
    let mut pending = imported_record(OTHER_CONTRACT);
    pending
        .constructor_args
        .insert("admin".to_string(), ACCOUNT.to_string());
    manifest.contracts.insert(
        crate::commands::cutover::GOVERNANCE_PENDING_KEY.to_string(),
        pending,
    );
    manifest.save(&state).expect("save manifest");
    let cli = Cli {
        yes: true,
        workspace_path: dir.path().into(),
        command: Commands::Upgrade(UpgradeArgs {
            command: UpgradeCommand::ConfirmGovernanceCutover,
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    // The current governance's admin() is unreadable, so the expected
    // admin falls back to the pending record's constructor admin.
    let executor = UnreadableAdminExecutor(RecordingExecutor::new());
    let err = run(&cli, &executor).expect_err("unverifiable wasm hash must fail closed");
    let message = format!("{err:#}");
    assert!(
        message.contains("cannot be confirmed") || message.contains("cutover cannot be confirmed"),
        "{message}"
    );
    assert!(
        message.contains("wasm hash") && !message.contains("admin mismatch"),
        "admin must match the constructor admin; message was: {message}"
    );
    // The manifest still holds the pending record untouched.
    let after = Manifest::load_or_new(&state, "testnet", None).expect("reload");
    assert!(after
        .contracts
        .contains_key(crate::commands::cutover::GOVERNANCE_PENDING_KEY));
}

#[test]
fn confirm_governance_cutover_fails_closed_without_readable_admin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    for key in ["vault", "governance"] {
        manifest
            .contracts
            .insert(key.to_string(), imported_record(CONTRACT));
    }
    let pending = imported_record(OTHER_CONTRACT);
    manifest.contracts.insert(
        crate::commands::cutover::GOVERNANCE_PENDING_KEY.to_string(),
        pending,
    );
    manifest.save(&state).expect("save manifest");
    let cli = Cli {
        yes: true,
        workspace_path: dir.path().into(),
        command: Commands::Upgrade(UpgradeArgs {
            command: UpgradeCommand::ConfirmGovernanceCutover,
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    // The current governance's admin() is unreadable and the pending
    // record carries no constructor admin, so the confirmation must fail
    // closed rather than promote.
    let executor = UnreadableAdminExecutor(RecordingExecutor::new());
    let err = run(&cli, &executor).expect_err("missing admin fact must fail closed");
    let message = format!("{err:#}");
    assert!(
        message.contains("cannot determine the expected governance admin"),
        "{message}"
    );
    let after = Manifest::load_or_new(&state, "testnet", None).expect("reload");
    assert!(after
        .contracts
        .contains_key(crate::commands::cutover::GOVERNANCE_PENDING_KEY));
}

#[test]
fn confirm_governance_cutover_falls_back_to_constructor_admin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    for key in ["vault", "governance"] {
        manifest
            .contracts
            .insert(key.to_string(), imported_record(CONTRACT));
    }
    let mut pending = imported_record(OTHER_CONTRACT);
    pending
        .constructor_args
        .insert("admin".to_string(), ACCOUNT.to_string());
    manifest.contracts.insert(
        crate::commands::cutover::GOVERNANCE_PENDING_KEY.to_string(),
        pending,
    );
    manifest.save(&state).expect("save manifest");
    let cli = Cli {
        yes: true,
        workspace_path: dir.path().into(),
        command: Commands::Upgrade(UpgradeArgs {
            command: UpgradeCommand::ConfirmGovernanceCutover,
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    // The admin view is unreadable, so the expected admin comes from the
    // pending record's constructor args. The confirmation must report the
    // wasm-hash fact problem, never an admin determination failure, and
    // never promote.
    let executor = UnreadableAdminExecutor(RecordingExecutor::new());
    let err = run(&cli, &executor).expect_err("unfetched wasm hash must fail closed");
    let message = format!("{err:#}");
    assert!(message.contains("cutover cannot be confirmed"), "{message}");
    assert!(
        message.contains("wasm hash"),
        "guard must report the wasm hash fact; message was: {message}"
    );
    let after = Manifest::load_or_new(&state, "testnet", None).expect("reload");
    assert!(after
        .contracts
        .contains_key(crate::commands::cutover::GOVERNANCE_PENDING_KEY));
}

/// Executor whose `admin` views are unreadable (RPC hiccup / wrong
/// manifest governance), exercising the fail-closed admin fallback.
struct UnreadableAdminExecutor(RecordingExecutor);

impl crate::stellar::CommandExecutor for UnreadableAdminExecutor {
    fn run(
        &self,
        program: &str,
        args: &[String],
        redacted: &[usize],
        env: &[crate::stellar::CommandEnv],
    ) -> anyhow::Result<crate::stellar::CommandOutput> {
        if args.iter().any(|arg| arg == "admin") {
            return Err(anyhow::anyhow!("rpc unavailable"));
        }
        self.0.run(program, args, redacted, env)
    }
}
