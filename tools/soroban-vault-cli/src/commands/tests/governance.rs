use super::*;

#[test]
fn parses_supply_queue_entries_to_governance_json() {
    let entries = [
        format!("0:{CONTRACT}")
            .parse::<SupplyQueueEntryArg>()
            .expect("first entry"),
        format!("7:{CONTRACT}")
            .parse::<SupplyQueueEntryArg>()
            .expect("second entry"),
    ];
    let encoded = supply_queue_entries_json(&entries).expect("parse entries");
    let value: serde_json::Value = serde_json::from_str(&encoded).expect("json");
    assert_eq!(value[0]["target_id"], 0);
    assert_eq!(value[1]["adapter"], CONTRACT);
}

#[test]
fn parse_proposal_id_ignores_confirmed_tx_hash_suffix() {
    let proposal_id =
        parse_proposal_id("proposal 1\ntx hash: abcdef9876543210").expect("proposal id");

    assert_eq!(proposal_id, 1);
}

#[test]
fn governance_timelock_uses_typed_kind_and_direct_contract_method() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance(&state);
    let cli = base_cli(
        state,
        Commands::Governance(GovernanceArgs {
            command: GovernanceCommand::SubmitSetTimelock {
                admin: ACCOUNT.parse().expect("admin"),
                kind: "supply-queue".parse().expect("kind"),
                timelock_ns: 42,
            },
        }),
    );
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("run governance timelock");

    let calls = submitted_calls(&executor.calls());
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1.iter().any(|arg| arg == "submit_set_timelock"));
    assert!(calls[0]
        .1
        .windows(2)
        .any(|pair| pair == ["--kind", "SupplyQueue"]));
    assert!(calls[0]
        .1
        .windows(2)
        .any(|pair| pair == ["--new_timelock_ns", "42"]));
}

#[test]
fn governance_restrictions_use_typed_mode_and_address_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance(&state);
    let cli = base_cli(
        state,
        Commands::Governance(GovernanceArgs {
            command: GovernanceCommand::SubmitSetRestrictions {
                admin: ACCOUNT.parse().expect("admin"),
                mode: "blacklist".parse().expect("mode"),
                accounts: vec![ACCOUNT.parse().expect("account")],
            },
        }),
    );
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("run governance restrictions");

    let calls = executor.calls();
    assert!(calls[0]
        .1
        .iter()
        .any(|arg| arg == "submit_set_restrictions"));
    assert!(calls[0].1.windows(2).any(|pair| pair == ["--mode", "1"]));
    assert!(calls[0]
        .1
        .windows(2)
        .any(|pair| pair[0] == "--accounts" && pair[1].contains(ACCOUNT)));
}

#[test]
fn governance_accept_ready_accepts_only_ready_decoded_proposals() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance(&state);
    let cli = base_cli(
        state,
        Commands::Governance(GovernanceArgs {
            command: GovernanceCommand::AcceptReady {
                admin: ACCOUNT.parse().expect("admin"),
                kind: None,
                limit: None,
            },
        }),
    );
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("accept ready proposals");

    let calls = submitted_calls(&executor.calls());
    let accepted = calls
        .iter()
        .filter(|(_, args)| {
            args.iter().any(|arg| arg == "accept")
                && args.windows(2).any(|pair| pair == ["--proposal_id", "1"])
        })
        .count();
    assert_eq!(accepted, 1);
    assert!(!calls
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "accept")
            && args.windows(2).any(|pair| pair == ["--proposal_id", "2"])));
}

#[test]
fn governance_read_only_commands_use_view_invocation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance(&state);

    for command in [
        GovernanceCommand::Pending { proposal_id: None },
        GovernanceCommand::Pending {
            proposal_id: Some(1),
        },
        GovernanceCommand::Timelocks,
        GovernanceCommand::Queue { kind: None },
        GovernanceCommand::Explain { proposal_id: 1 },
    ] {
        let cli = base_cli(
            state.clone(),
            Commands::Governance(GovernanceArgs { command }),
        );
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("run governance view");

        let calls = executor.calls();
        assert_contract_invokes_are_views(&calls);
        assert!(submitted_calls(&calls).is_empty());
    }
}

#[test]
fn governance_submit_and_wait_submits_then_accepts_ready_proposal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance(&state);
    let cli = base_cli(
        state,
        Commands::Governance(GovernanceArgs {
            command: GovernanceCommand::SubmitAndWait(crate::cli::GovernanceSubmitAndWaitArgs {
                poll_seconds: 1,
                max_wait_seconds: 0,
                command: GovernanceSubmitAndWaitCommand::SetTimelock {
                    admin: ACCOUNT.parse().expect("admin"),
                    kind: "supply-queue".parse().expect("kind"),
                    timelock_ns: 42,
                },
            }),
        }),
    );
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("submit and wait");

    let calls = executor.calls();
    assert!(calls
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "submit_set_timelock")));
    assert!(calls
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "accept")
            && args.windows(2).any(|pair| pair == ["--proposal_id", "1"])));
}
