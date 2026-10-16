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
    pending.wasm_hash = "ab".repeat(32);
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
    // The expected admin comes from the pending record's constructor
    // admin, never from the current governance's identity.
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
    // The pending record carries no constructor admin, so the
    // confirmation must fail closed rather than promote.
    let executor = UnreadableAdminExecutor(RecordingExecutor::new());
    let err = run(&cli, &executor).expect_err("missing admin fact must fail closed");
    let message = format!("{err:#}");
    assert!(
        message.contains("cannot be confirmed") && message.contains("no valid constructor admin"),
        "{message}"
    );
    let after = Manifest::load_or_new(&state, "testnet", None).expect("reload");
    assert!(after
        .contracts
        .contains_key(crate::commands::cutover::GOVERNANCE_PENDING_KEY));
}

#[test]
fn confirm_governance_cutover_takes_expected_admin_from_pending_record() {
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
    pending.wasm_hash = "ab".repeat(32);
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
    // The guard compares the target's admin() against the pending record's
    // constructor admin and reports the wasm-hash fact (the executor never
    // serves the wasm fetch), never an admin determination failure.
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

/// Executor that serves the full cutover view set for `target`: the
/// target's `vault()`/`admin()` views, a WASM fetch returning
/// `governance_wasm`, and a vault `proxy_view` whose governance binding is
/// `bound_governance` (`None` = unreadable).
struct CutoverChainExecutor {
    target: &'static str,
    governance_wasm: &'static [u8],
    bound_governance: Option<String>,
    inner: RecordingExecutor,
}

impl CutoverChainExecutor {
    fn bound_to(target: &'static str, bound_governance: Option<String>) -> Self {
        Self {
            target,
            governance_wasm: b"fake governance wasm",
            bound_governance,
            inner: RecordingExecutor::new(),
        }
    }
}

impl crate::stellar::CommandExecutor for CutoverChainExecutor {
    fn run(
        &self,
        program: &str,
        args: &[String],
        redacted: &[usize],
        env: &[crate::stellar::CommandEnv],
    ) -> anyhow::Result<crate::stellar::CommandOutput> {
        let for_target = args
            .windows(2)
            .any(|pair| pair[0] == "--id" && pair[1] == self.target);
        if for_target && args.iter().any(|arg| arg == "admin") {
            return Ok(crate::stellar::CommandOutput {
                stdout: ACCOUNT.to_string(),
                stderr: String::new(),
            });
        }
        if for_target && args.windows(2).any(|pair| pair[0] == "--out-file") {
            let path = args
                .windows(2)
                .find_map(|pair| (pair[0] == "--out-file").then_some(&pair[1]))
                .expect("out-file");
            std::fs::write(path, self.governance_wasm).expect("write fetched wasm");
            return Ok(crate::stellar::CommandOutput {
                stdout: self.target.to_string(),
                stderr: String::new(),
            });
        }
        if args.iter().any(|arg| arg == "proxy_view") {
            return match &self.bound_governance {
                Some(governance) => Ok(crate::stellar::CommandOutput {
                    stdout: format!(
                        "[[[null,\"{governance}\",null,null],[null,null,null],[null,null,null,null],null],null,[null,null,null]]"
                    ),
                    stderr: String::new(),
                }),
                None => Err(anyhow::anyhow!("proxy_view unavailable")),
            };
        }
        if for_target && args.iter().any(|arg| arg == "vault") {
            return Ok(crate::stellar::CommandOutput {
                stdout: CONTRACT.to_string(),
                stderr: String::new(),
            });
        }
        self.inner.run(program, args, redacted, env)
    }
}

/// Manifest with vault+governance CONTRACT and a pending OTHER_CONTRACT
/// whose constructor admin differs from the current governance's.
fn cutover_manifest(dir: &std::path::Path) -> std::path::PathBuf {
    let state = dir.join("manifest.json");
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
    pending.wasm_hash = format!("{:x}", Sha256::digest(b"fake governance wasm"));
    manifest.contracts.insert(
        crate::commands::cutover::GOVERNANCE_PENDING_KEY.to_string(),
        pending,
    );
    manifest.save(&state).expect("save manifest");
    state
}

fn confirm_cli(dir: &std::path::Path, state: std::path::PathBuf) -> Cli {
    Cli {
        yes: true,
        workspace_path: dir.to_path_buf(),
        command: Commands::Upgrade(UpgradeArgs {
            command: UpgradeCommand::ConfirmGovernanceCutover,
        }),
        ..base_cli(state, Commands::Status)
    }
}

#[test]
fn confirm_refuses_while_vault_is_still_bound_to_old_governance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cutover_manifest(dir.path());
    let executor = CutoverChainExecutor::bound_to(OTHER_CONTRACT, Some(CONTRACT.to_string()));
    let err = run(&confirm_cli(dir.path(), state.clone()), &executor)
        .expect_err("promotion must wait for the on-chain binding");
    let message = format!("{err:#}");
    assert!(
        message.contains("still bound to governance") && message.contains(OTHER_CONTRACT),
        "{message}"
    );
    let after = Manifest::load_or_new(&state, "testnet", None).expect("reload");
    assert!(after
        .contracts
        .contains_key(crate::commands::cutover::GOVERNANCE_PENDING_KEY));
}

#[test]
fn confirm_fails_closed_when_governance_binding_is_unreadable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cutover_manifest(dir.path());
    let executor = CutoverChainExecutor::bound_to(OTHER_CONTRACT, None);
    let err = run(&confirm_cli(dir.path(), state.clone()), &executor)
        .expect_err("unreadable binding must fail closed");
    let message = format!("{err:#}");
    assert!(
        message.contains("cannot read the governance binding"),
        "{message}"
    );
    let after = Manifest::load_or_new(&state, "testnet", None).expect("reload");
    assert!(after
        .contracts
        .contains_key(crate::commands::cutover::GOVERNANCE_PENDING_KEY));
}

#[test]
fn confirm_promotes_after_vault_is_bound_to_pending_governance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cutover_manifest(dir.path());
    let executor = CutoverChainExecutor::bound_to(OTHER_CONTRACT, Some(OTHER_CONTRACT.to_string()));
    run(&confirm_cli(dir.path(), state.clone()), &executor).expect("cutover confirms");
    let after = Manifest::load_or_new(&state, "testnet", None).expect("reload");
    assert!(!after
        .contracts
        .contains_key(crate::commands::cutover::GOVERNANCE_PENDING_KEY));
    assert_eq!(
        after.contracts["governance"].contract_id, OTHER_CONTRACT,
        "active governance must be the promoted pending one"
    );
    assert_eq!(
        after.contracts[crate::commands::cutover::GOVERNANCE_PREVIOUS_KEY].contract_id,
        CONTRACT,
        "previous governance must be preserved for audit"
    );
}

#[test]
fn confirm_matches_explicit_admin_and_wasm_hash_from_pending_record() {
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
    pending.wasm_hash = format!("{:x}", Sha256::digest(b"fake governance wasm"));
    manifest.contracts.insert(
        crate::commands::cutover::GOVERNANCE_PENDING_KEY.to_string(),
        pending,
    );
    manifest.save(&state).expect("save manifest");
    // The current governance reports the default (unrelated) address as its
    // admin: an admin rotated during the cutover must not block
    // confirmation, because the expected values come from the pending
    // record, not from the old governance.
    let executor = CutoverChainExecutor::bound_to(OTHER_CONTRACT, Some(OTHER_CONTRACT.to_string()));
    run(&confirm_cli(dir.path(), state.clone()), &executor)
        .expect("cutover confirms despite rotated admin");
    let after = Manifest::load_or_new(&state, "testnet", None).expect("reload");
    assert_eq!(after.contracts["governance"].contract_id, OTHER_CONTRACT);
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
