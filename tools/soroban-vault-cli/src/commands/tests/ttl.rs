use super::*;

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the topology fixture and all instance/code TTL assertions belong in one scenario"
)]
fn extend_ttl_supports_default_contract_admin_topology() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    for (key, contract_id) in [
        ("vault", "CVAULT"),
        ("governance", "CGOVERNANCE"),
        ("proxy_4626", "CPROXY4626"),
        ("curator_proxy", "CCURATORPROXY"),
        ("asset_token", "CASSET"),
    ] {
        manifest
            .contracts
            .insert(key.to_string(), imported_record(contract_id));
    }
    manifest.contracts.insert(
        "share_token".to_string(),
        ContractRecord {
            wasm_hash: "1111111111111111111111111111111111111111111111111111111111111111"
                .to_string(),
            constructor_args: map_args([("admin", "CVAULT"), ("vault", "CVAULT")]),
            ..imported_record("CSHARE")
        },
    );
    manifest.contracts.insert(
        "blend_adapter_0".to_string(),
        ContractRecord {
            wasm_hash: "2222222222222222222222222222222222222222222222222222222222222222"
                .to_string(),
            constructor_args: map_args([
                ("admin", "CGOVERNANCE"),
                ("vault", "CVAULT"),
                ("pool", "CPOOL"),
            ]),
            ..imported_record("CADAPTER0")
        },
    );
    manifest.contracts.insert(
        "blend_adapter_1".to_string(),
        ContractRecord {
            wasm_hash: "3333333333333333333333333333333333333333333333333333333333333333"
                .to_string(),
            constructor_args: map_args([
                ("admin", "CGOVERNANCE"),
                ("vault", "CVAULT"),
                ("pool", "CPOOL2"),
            ]),
            ..imported_record("CADAPTER1")
        },
    );
    manifest.contracts.insert(
        "custodial_adapter_0".to_string(),
        ContractRecord {
            constructor_args: map_args([("admin", ACCOUNT)]),
            ..imported_record("CCUSTODIAL0")
        },
    );
    manifest.save(&state).expect("save manifest");
    let cli = base_cli(
        state,
        Commands::ExtendTtl(ExtendTtlArgs {
            caller: Some(ACCOUNT.parse().expect("caller")),
        }),
    );
    let executor = TtlRecordingExecutor::new();

    run(&cli, &executor).expect("extend ttl");

    let calls = submitted_calls(&executor.calls());
    assert_eq!(calls.len(), 14);
    assert!(calls.iter().any(
        |(_, args)| args.windows(2).any(|pair| pair == ["--id", "CVAULT"])
            && args.iter().any(|arg| arg == "execute")
    ));
    for contract_id in ["CGOVERNANCE", "CPROXY4626", "CCURATORPROXY", "CCUSTODIAL0"] {
        assert!(calls.iter().any(|(_, args)| args
            .windows(2)
            .any(|pair| pair == ["--id", contract_id])
            && args.iter().any(|arg| arg == "extend_ttl")));
    }
    for contract_id in ["CSHARE", "CADAPTER0", "CADAPTER1"] {
        assert_protocol_ttl_call(&calls, "--id", contract_id);
    }
    for wasm_hash in [
        format!("{:x}", Sha256::digest(b"CVAULT")),
        format!("{:x}", Sha256::digest(b"CGOVERNANCE")),
        format!("{:x}", Sha256::digest(b"CPROXY4626")),
        format!("{:x}", Sha256::digest(b"CCURATORPROXY")),
        format!("{:x}", Sha256::digest(b"CSHARE")),
        format!("{:x}", Sha256::digest(b"shared blend adapter wasm")),
    ] {
        assert_protocol_ttl_call(&calls, "--wasm-hash", &wasm_hash);
    }
    assert_eq!(
        calls
            .iter()
            .filter(|(_, args)| args.iter().any(|arg| arg == "--wasm-hash"))
            .count(),
        6
    );
    assert!(!calls
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "--caller")));
    assert!(!calls
        .iter()
        .any(|(_, args)| args.windows(2).any(|pair| pair == ["--id", "CASSET"])));
}

#[test]
fn extend_ttl_runs_for_governance_admin_custodial_adapter() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("governance".to_string(), imported_record("CGOVERNANCE"));
    manifest.contracts.insert(
        "custodial_adapter_0".to_string(),
        ContractRecord {
            constructor_args: map_args([("admin", "CGOVERNANCE")]),
            ..imported_record("CCUSTODIAL0")
        },
    );
    manifest.save(&state).expect("save manifest");
    let cli = base_cli(
        state,
        Commands::ExtendTtl(ExtendTtlArgs {
            caller: Some(ACCOUNT.parse().expect("caller")),
        }),
    );
    let executor = TtlRecordingExecutor::new();

    run(&cli, &executor).expect("extend ttl");

    let calls = submitted_calls(&executor.calls());
    assert!(calls.iter().any(|(_, args)| args
        .windows(2)
        .any(|pair| pair == ["--id", "CGOVERNANCE"])
        && args.iter().any(|arg| arg == "extend_ttl")));
    assert!(calls.iter().any(|(_, args)| args
        .windows(2)
        .any(|pair| pair == ["--id", "CCUSTODIAL0"])
        && args.iter().any(|arg| arg == "extend_ttl")
        && !args.iter().any(|arg| arg == "--caller")));
}
