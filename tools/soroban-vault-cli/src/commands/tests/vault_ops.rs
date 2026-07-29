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
    let payload = calls
        .iter()
        .flat_map(|(_, args)| args.windows(2))
        .find_map(|pair| (pair[0] == "--payload").then_some(pair[1].as_str()))
        .expect("payload argument");
    let bytes = hex::decode(payload).expect("decode payload hex");
    let command = WireVaultCommand::decode(&bytes).expect("decode vault command");
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
