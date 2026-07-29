use super::*;

#[test]
fn curator_abort_withdrawing_encodes_vault_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let cli = base_cli(
        state,
        Commands::Curator(CuratorArgs {
            command: CuratorCommand::AbortWithdrawing {
                caller: ACCOUNT.parse().expect("caller"),
                op_id: 42,
            },
        }),
    );
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("abort withdrawing");

    let calls = submitted_calls(&executor.calls());
    let command = decoded_payload(&calls);
    assert_eq!(
        command,
        WireVaultCommand::AbortWithdrawing {
            caller: ACCOUNT.to_string(),
            op_id: 42,
        }
    );
}

#[test]
fn user_deposit_prefers_erc4626_proxy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest.contracts.insert(
        "proxy_4626".to_string(),
        ContractRecord {
            contract_id: "CPROXY".to_string(),
            wasm_hash: "hash".to_string(),
            salt: None,
            constructor_args: BTreeMap::new(),
            deploy_tx: None,
            initialized: true,
        },
    );
    manifest.save(&state).expect("save manifest");
    let cli = base_cli(
        state.clone(),
        Commands::User(UserArgs {
            command: UserCommand::Deposit {
                operator: ACCOUNT.parse().expect("operator"),
                receiver: Some(ACCOUNT.parse().expect("receiver")),
                assets: None,
                assets_raw: Some(11),
                asset_decimals: 7,
                min_shares_out: None,
                min_shares_out_raw: 7,
                share_decimals: ShareDecimalsArg::Manifest,
            },
        }),
    );
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("run deposit");

    let calls = submitted_calls(&executor.calls());
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "stellar");
    assert!(calls[0].1.windows(2).any(|pair| pair == ["--id", "CPROXY"]));
    assert!(calls[0].1.iter().any(|arg| arg == "deposit_with_min"));
    assert!(calls[0].1.windows(2).any(|pair| pair == ["--assets", "11"]));
    let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    let tx = loaded
        .transactions
        .last()
        .expect("transaction record should be written");
    assert_eq!(tx.command.as_deref(), Some("user"));
    assert_eq!(tx.contract_id.as_deref(), Some("CPROXY"));
    assert_eq!(tx.function.as_deref(), Some("deposit_with_min"));
    assert_eq!(tx.result_status.as_deref(), Some("success"));
}

#[test]
fn user_withdraw_uses_erc4626_async_entrypoint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("proxy_4626".to_string(), imported_record("CPROXY"));
    manifest.save(&state).expect("save manifest");
    let cli = base_cli(
        state,
        Commands::User(UserArgs {
            command: UserCommand::Withdraw {
                operator: ACCOUNT.parse().expect("operator"),
                receiver: Some(ACCOUNT.parse().expect("receiver")),
                owner: Some(ACCOUNT.parse().expect("owner")),
                assets: None,
                assets_raw: Some(11),
                asset_decimals: 7,
            },
        }),
    );
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("run async withdraw through proxy");

    let calls = submitted_calls(&executor.calls());
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1.windows(2).any(|pair| pair == ["--id", "CPROXY"]));
    assert!(calls[0].1.iter().any(|arg| arg == "withdraw"));
    assert!(!calls[0].1.iter().any(|arg| arg == "--max_shares_burned"));
}

#[test]
fn user_redeem_uses_erc4626_async_entrypoint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("proxy_4626".to_string(), imported_record("CPROXY"));
    manifest.save(&state).expect("save manifest");
    let cli = base_cli(
        state,
        Commands::User(UserArgs {
            command: UserCommand::Redeem {
                operator: ACCOUNT.parse().expect("operator"),
                receiver: Some(ACCOUNT.parse().expect("receiver")),
                owner: Some(ACCOUNT.parse().expect("owner")),
                shares: None,
                shares_raw: Some(11),
                share_decimals: ShareDecimalsArg::Manifest,
            },
        }),
    );
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("run async redeem through proxy");

    let calls = submitted_calls(&executor.calls());
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1.windows(2).any(|pair| pair == ["--id", "CPROXY"]));
    assert!(calls[0].1.iter().any(|arg| arg == "redeem"));
    assert!(!calls[0].1.iter().any(|arg| arg == "--min_assets_out"));
}

#[test]
fn user_atomic_withdraw_prefers_erc4626_entrypoint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let mut manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    manifest
        .contracts
        .insert("proxy_4626".to_string(), imported_record("CPROXY"));
    manifest.save(&state).expect("save proxy manifest");
    let cli = base_cli(
        state.clone(),
        Commands::User(UserArgs {
            command: UserCommand::AtomicWithdraw {
                operator: ACCOUNT.parse().expect("operator"),
                receiver: Some(ACCOUNT.parse().expect("receiver")),
                owner: Some(ACCOUNT.parse().expect("owner")),
                assets: None,
                assets_raw: Some(11),
                asset_decimals: 7,
                max_shares_burned: None,
                max_shares_burned_raw: Some(12),
                share_decimals: ShareDecimalsArg::Manifest,
            },
        }),
    );
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("run atomic withdraw through proxy");

    let calls = submitted_calls(&executor.calls());
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1.windows(2).any(|pair| pair == ["--id", "CPROXY"]));
    assert!(calls[0].1.iter().any(|arg| arg == "atomic_withdraw"));
    assert!(calls[0]
        .1
        .windows(2)
        .any(|pair| pair == ["--max_shares_burned", "12"]));
    let manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    let transaction = manifest.transactions.last().expect("transaction record");
    assert_eq!(transaction.contract_id.as_deref(), Some("CPROXY"));
    assert_eq!(transaction.function.as_deref(), Some("atomic_withdraw"));
}

#[test]
fn user_atomic_withdraw_rejects_legacy_erc4626_proxy_without_vault_fallback() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let mut manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    manifest
        .contracts
        .insert("proxy_4626".to_string(), imported_record("CPROXY"));
    manifest.save(&state).expect("save proxy manifest");
    let cli = base_cli(
        state,
        Commands::User(UserArgs {
            command: UserCommand::AtomicWithdraw {
                operator: ACCOUNT.parse().expect("operator"),
                receiver: Some(ACCOUNT.parse().expect("receiver")),
                owner: Some(ACCOUNT.parse().expect("owner")),
                assets: None,
                assets_raw: Some(11),
                asset_decimals: 7,
                max_shares_burned: None,
                max_shares_burned_raw: Some(12),
                share_decimals: ShareDecimalsArg::Manifest,
            },
        }),
    );
    let executor = RecordingExecutor::legacy_proxy();

    let error = run(&cli, &executor).expect_err("legacy proxy must be rejected");

    assert!(error
        .to_string()
        .contains("legacy and does not expose atomic_withdraw and atomic_redeem"));
    let calls = executor.calls();
    assert!(calls.iter().any(|(_, args)| {
        matches!(args.as_slice(), [contract, info, interface, ..] if contract == "contract" && info == "info" && interface == "interface")
    }));
    assert!(!calls.iter().any(|(_, args)| {
        args.iter()
            .any(|arg| arg == "atomic_withdraw" || arg == "execute")
    }));
}

#[test]
fn user_atomic_withdraw_falls_back_to_vault_command_with_share_guard() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let cli = base_cli(
        state.clone(),
        Commands::User(UserArgs {
            command: UserCommand::AtomicWithdraw {
                operator: ACCOUNT.parse().expect("operator"),
                receiver: Some(ACCOUNT.parse().expect("receiver")),
                owner: Some(ACCOUNT.parse().expect("owner")),
                assets: None,
                assets_raw: Some(11),
                asset_decimals: 7,
                max_shares_burned: None,
                max_shares_burned_raw: Some(12),
                share_decimals: ShareDecimalsArg::Manifest,
            },
        }),
    );
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("run atomic withdraw");

    let calls = submitted_calls(&executor.calls());
    assert_eq!(
        decoded_payload(&calls),
        WireVaultCommand::AtomicWithdraw {
            owner: ACCOUNT.to_string(),
            receiver: ACCOUNT.to_string(),
            operator: ACCOUNT.to_string(),
            assets: 11,
            max_shares_burned: 12,
        }
    );
    let manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    let transaction = manifest.transactions.last().expect("transaction record");
    assert_eq!(transaction.contract_id.as_deref(), Some(CONTRACT));
    assert_eq!(transaction.function.as_deref(), Some("execute"));
}

#[test]
fn user_atomic_redeem_prefers_erc4626_entrypoint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let mut manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    manifest
        .contracts
        .insert("proxy_4626".to_string(), imported_record("CPROXY"));
    manifest.save(&state).expect("save proxy manifest");
    let cli = base_cli(
        state.clone(),
        Commands::User(UserArgs {
            command: UserCommand::AtomicRedeem {
                operator: ACCOUNT.parse().expect("operator"),
                receiver: Some(ACCOUNT.parse().expect("receiver")),
                owner: Some(ACCOUNT.parse().expect("owner")),
                shares: None,
                shares_raw: Some(11),
                share_decimals: ShareDecimalsArg::Manifest,
                min_assets_out: None,
                min_assets_out_raw: 10,
                asset_decimals: 7,
            },
        }),
    );
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("run atomic redeem through proxy");

    let calls = submitted_calls(&executor.calls());
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1.windows(2).any(|pair| pair == ["--id", "CPROXY"]));
    assert!(calls[0].1.iter().any(|arg| arg == "atomic_redeem"));
    assert!(calls[0]
        .1
        .windows(2)
        .any(|pair| pair == ["--min_assets_out", "10"]));
    let manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    let transaction = manifest.transactions.last().expect("transaction record");
    assert_eq!(transaction.contract_id.as_deref(), Some("CPROXY"));
    assert_eq!(transaction.function.as_deref(), Some("atomic_redeem"));
}

#[test]
fn user_atomic_redeem_falls_back_to_vault_command_with_asset_guard() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let cli = base_cli(
        state,
        Commands::User(UserArgs {
            command: UserCommand::AtomicRedeem {
                operator: ACCOUNT.parse().expect("operator"),
                receiver: Some(ACCOUNT.parse().expect("receiver")),
                owner: Some(ACCOUNT.parse().expect("owner")),
                shares: None,
                shares_raw: Some(11),
                share_decimals: ShareDecimalsArg::Manifest,
                min_assets_out: None,
                min_assets_out_raw: 10,
                asset_decimals: 7,
            },
        }),
    );
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("run atomic redeem");

    let calls = submitted_calls(&executor.calls());
    assert_eq!(
        decoded_payload(&calls),
        WireVaultCommand::AtomicRedeem {
            owner: ACCOUNT.to_string(),
            receiver: ACCOUNT.to_string(),
            operator: ACCOUNT.to_string(),
            shares: 11,
            min_assets_out: 10,
        }
    );
}

#[test]
fn user_share_token_and_adapter_read_only_commands_use_view_invocation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_view_contracts(&state);

    let commands = [
        Commands::User(UserArgs {
            command: UserCommand::Balance {
                owner: ACCOUNT.parse().expect("owner"),
            },
        }),
        Commands::User(UserArgs {
            command: UserCommand::Preview {
                owner: ACCOUNT.parse().expect("owner"),
                assets: None,
                assets_raw: 0,
                asset_decimals: 7,
                shares: None,
                shares_raw: 0,
                share_decimals: "manifest".parse().expect("share decimals"),
            },
        }),
        Commands::ShareToken(ShareTokenArgs {
            command: ShareTokenCommand::Balance {
                account: ACCOUNT.parse().expect("account"),
            },
        }),
        Commands::ShareToken(ShareTokenArgs {
            command: ShareTokenCommand::Admin,
        }),
        Commands::Adapter(AdapterArgs {
            adapter_index: 0,
            adapter_key: None,
            adapter_pool: None,
            command: AdapterCommand::TotalAssets {
                asset: CONTRACT.parse().expect("asset"),
            },
        }),
        Commands::Adapter(AdapterArgs {
            adapter_index: 0,
            adapter_key: None,
            adapter_pool: None,
            command: AdapterCommand::Pool,
        }),
    ];

    for command in commands {
        let cli = base_cli(state.clone(), command);
        let executor = RecordingExecutor::new();

        run(&cli, &executor).expect("run read-only command");

        let calls = executor.calls();
        assert_contract_invokes_are_views(&calls);
        assert!(submitted_calls(&calls).is_empty());
    }
}
