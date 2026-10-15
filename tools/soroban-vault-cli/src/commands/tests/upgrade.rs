use super::*;

use crate::{
    cli::{UpgradeArgs, UpgradeCommand, UpgradePreflightArgs},
    commands::{
        output::PreflightStatus,
        upgrade::{
            adapter_bindings_from, decode_fee_recipients, migration_flag_from,
            parse_contract_read_instance, parse_instance_storage, parse_pending_ids,
            parse_proxy_view,
        },
    },
};

#[test]
fn preflight_reports_unverifiable_checks_for_v1_1_1_vault() {
    // RecordingExecutor has no instance-storage or proxy_view handling for
    // this vault, so every check that needs those reads comes back
    // unverifiable rather than passing silently.
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Upgrade(UpgradeArgs {
            command: UpgradeCommand::Preflight(UpgradePreflightArgs {
                vault: None,
                attest_empty_withdrawal_queue: false,
                allow_unverifiable: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    // Human output mode keeps the error propagatable; JSON mode would
    // print the envelope and exit the process.
    let cli = Cli { json: false, ..cli };
    let err = run(&cli, &executor).expect_err("unverifiable checks must block");

    let message = format!("{err:#}");
    for check in [
        "migration_flag",
        "fee_recipients",
        "withdrawal_queue",
        "adapter_bindings",
    ] {
        assert!(message.contains(check), "{check} must be reported");
    }
}

#[test]
fn preflight_finding_classification_is_pure() {
    let facts = crate::commands::upgrade::VaultFacts {
        vault_id: CONTRACT.to_string(),
        asset_token_id: Some(ASSET_CONTRACT.to_string()),
        share_token_id: Some(OTHER_CONTRACT.to_string()),
        runtime_version: Some("1.2.0".to_string()),
        governance_view: Some(CONTRACT.to_string()),
        governance_manifest: Some(CONTRACT.to_string()),
        paused: Some(false),
        virtual_assets: Some(0),
        external_assets_present: Some(false),
        max_deposit_allowed: Some(1000),
        migration_flag: Some(false),
        fee_recipients: Some([(0u128, kernel_hex("GADMIN")), (0u128, kernel_hex("GADMIN"))]),
        adapter_bindings: Some(vec![(1, OTHER_CONTRACT.to_string())]),
        pending_proposals: Some(Vec::new()),
        stale_proposals: Some(0),
        attest_empty_withdrawal_queue: false,
    };
    let report = crate::commands::upgrade::evaluate_vault_preflight(&facts);
    assert!(report.ok, "clean vault must pass: {report:?}");
}

#[test]
fn preflight_flags_fee_recipient_and_duplicate_adapters() {
    let facts = crate::commands::upgrade::VaultFacts {
        vault_id: CONTRACT.to_string(),
        asset_token_id: Some(ASSET_CONTRACT.to_string()),
        share_token_id: Some(OTHER_CONTRACT.to_string()),
        runtime_version: Some("1.2.0".to_string()),
        governance_view: Some(CONTRACT.to_string()),
        governance_manifest: Some(CONTRACT.to_string()),
        paused: Some(false),
        virtual_assets: Some(5),
        external_assets_present: Some(true),
        max_deposit_allowed: Some(0),
        migration_flag: Some(false),
        fee_recipients: Some([(1u128, kernel_hex(CONTRACT)), (0u128, kernel_hex("GADMIN"))]),
        adapter_bindings: Some(vec![
            (1, OTHER_CONTRACT.to_string()),
            (2, OTHER_CONTRACT.to_string()),
        ]),
        pending_proposals: Some(Vec::new()),
        stale_proposals: Some(0),
        attest_empty_withdrawal_queue: true,
    };
    let report = crate::commands::upgrade::evaluate_vault_preflight(&facts);
    let status = |name: &str| {
        report
            .findings
            .iter()
            .find(|f| f.check == name)
            .map(|f| f.status)
    };
    assert_eq!(status("fee_recipients"), Some(PreflightStatus::Blocking));
    assert_eq!(status("adapter_bindings"), Some(PreflightStatus::Blocking));
    assert_eq!(status("op_state_idle"), Some(PreflightStatus::Warn));
    assert_eq!(status("external_assets"), Some(PreflightStatus::Warn));
    assert_eq!(status("withdrawal_queue"), Some(PreflightStatus::Warn));
    assert!(!report.ok);
}

#[test]
fn kernel_address_matches_runtime_kernel_address_from_sdk() {
    const CURATOR: &str = "GCXDS2RBUD7QFKXD5BY4UNYBKVQ37LPBFV7WQTCLN6RUR2SQSPJCSNBH";
    // Runtime-derived vector: the live v1.1.1 vault stored its curator as
    // the fee recipient in the `fees` instance entry; the runtime's
    // `kernel_address_from_sdk` is sha256(domain || strkey), with no
    // version byte. Recipient bytes hex-decoded from the raw stored blob
    // captured live.
    const STORED_RECIPIENT: &str =
        "47d68178f253c8218cc7b4fe6e990d765b807ce4a4630ac1fa50e352bfe91cce";
    assert_eq!(kernel_hex(CURATOR), STORED_RECIPIENT);
}

#[test]
fn duplicate_adapter_detection() {
    assert!(
        crate::commands::upgrade::duplicate_adapters(&[(1, OTHER_CONTRACT.to_string())]).is_empty()
    );
    assert_eq!(
        crate::commands::upgrade::duplicate_adapters(&[
            (1, OTHER_CONTRACT.to_string()),
            (2, OTHER_CONTRACT.to_string())
        ]),
        vec![OTHER_CONTRACT.to_string()]
    );
}

#[test]
fn instance_storage_requires_init_marker() {
    assert!(parse_instance_storage(r#"{"contract_instance":{"storage":[]}}"#).is_none());
    let map = parse_instance_storage(
        r#"{"contract_instance":{"storage":[{"key":{"symbol":"init"},"val":{"bool":true}}]}}"#,
    )
    .expect("init marker");
    assert!(map.contains_key("init"));
}

#[test]
fn fee_recipient_decode_handles_full_spec() {
    // Byte layout captured from a live v1.1.1 vault (little-endian WADs):
    // fee(16 LE) recipient(32) fee(16 LE) recipient(32) growth-cap flag(1).
    let bytes = hex::decode("00008a5d78456301000000000000000047d68178f253c8218cc7b4fe6e990d765b807ce4a4630ac1fa50e352bfe91cce0000000000000000000000000000000047d68178f253c8218cc7b4fe6e990d765b807ce4a4630ac1fa50e352bfe91cce00").expect("blob");
    assert_eq!(bytes.len(), 97);
    let value = serde_json::json!({ "bytes": hex::encode(bytes) });
    let spec = decode_fee_recipients(&value).expect("decode fee spec");
    assert_eq!(spec[0].0, 0x0163_4578_5d8a_0000);
    assert_eq!(
        spec[0].1,
        "47d68178f253c8218cc7b4fe6e990d765b807ce4a4630ac1fa50e352bfe91cce"
    );
    assert_eq!(spec[1].0, 0);
}

#[test]
fn fee_recipient_decode_rejects_bad_frames() {
    // 96 and 98 bytes are both malformed (valid frames are 97/113).
    for len in [96usize, 98] {
        let value = serde_json::json!({ "bytes": hex::encode(vec![0u8; len]) });
        assert!(decode_fee_recipients(&value).is_none());
    }
}

#[test]
fn instance_storage_parses_live_contract_read_csv() {
    // Fixture: `stellar contract read --id <vault> --key-xdr AAAAFA==
    // --output json` output on a v1.1.1 vault (JSON value quoted per CSV).
    let value = serde_json::json!({
        "contract_instance": {
            "executable": {"wasm": "4d24790f3ea2a02e521b84d583dab00bfa246cdfd06ee858f1f656a831cccc83"},
            "storage": [
                {"key": {"symbol": "init"}, "val": {"bool": true}},
                {"key": {"symbol": "adapmap"}, "val": {"map": [
                    {"key": {"u32": 0}, "val": {"address": OTHER_CONTRACT}}
                ]}},
                {"key": {"symbol": "fees"}, "val": {"bytes": "00008a5d78456301000000000000000047d68178f253c8218cc7b4fe6e990d765b807ce4a4630ac1fa50e352bfe91cce0000000000000000000000000000000047d68178f253c8218cc7b4fe6e990d765b807ce4a4630ac1fa50e352bfe91cce00"}}
            ]
        }
    });
    let stdout = format!(
        "\"\"ledger_key_contract_instance\"\",\"{}\",269,3110491",
        value.to_string().replace('"', "\"\"")
    );
    let entries = parse_contract_read_instance(&stdout).expect("entries");
    assert_eq!(migration_flag_from(&entries), Some(false));
    assert_eq!(
        adapter_bindings_from(&entries),
        Some(vec![(0, OTHER_CONTRACT.to_string())])
    );
    let fees = decode_fee_recipients(&entries["fees"]).expect("fees");
    assert_eq!(
        fees[0].1,
        kernel_hex("GCXDS2RBUD7QFKXD5BY4UNYBKVQ37LPBFV7WQTCLN6RUR2SQSPJCSNBH")
    );
}

#[test]
fn migration_flag_absent_means_false_present_tagged_bool_parsed() {
    let mut entries = serde_json::Map::new();
    assert_eq!(migration_flag_from(&entries), Some(false));
    entries.insert("migrate".to_string(), serde_json::json!({"bool": true}));
    assert_eq!(migration_flag_from(&entries), Some(true));
    entries.insert("migrate".to_string(), serde_json::json!({"u32": 1}));
    assert_eq!(migration_flag_from(&entries), None);
}

#[test]
fn proxy_view_parses_live_sample() {
    // Fixture: real `stellar contract invoke ... -- proxy_view` output
    // (including the simulation banner line a read invoke prints).
    let stdout = "ℹ️ Simulation identified as read-only. Send by rerunning with `--send=yes`.\n[[[\"GCXDS2RBUD7QFKXD5BY4UNYBKVQ37LPBFV7WQTCLN6RUR2SQSPJCSNBH\",\"CDZWIF2T7MVJKS6S6KYT3GYGFWQ2IUOHW4N2BNCHB6UDOS6ANXO2IOC7\",\"CDMLFMKMMD7MWZP3FKUBZPVHTUEDLSX4BYGYKH4GCESXYHS3IHQ4EIG4\",\"CDHKSAVRTWJ7KPMY3PTNJGUTWMLZUOUQE3QBXUBKBJL5YNCRNVW3VBC3\"],[\"0\",\"0\",false],[\"1000000000\",\"500000000\",\"500000000\",\"1000000000\"],[\"1000000000\",1792072734000000000,\"0\",\"100000000000000000\",\"0\"]],[[0],[]],[\"0\",\"0\",\"170141183460469231731687303715884105727\",\"170141183460469231731687303715884105727\",\"100000000\",\"100000000\",\"0\",\"0\"]]";
    let mut facts = crate::commands::upgrade::VaultFacts::default();
    parse_proxy_view(stdout, &mut facts);
    assert_eq!(
        facts.governance_view.as_deref(),
        Some("CDZWIF2T7MVJKS6S6KYT3GYGFWQ2IUOHW4N2BNCHB6UDOS6ANXO2IOC7")
    );
    assert_eq!(
        facts.asset_token_id.as_deref(),
        Some("CDMLFMKMMD7MWZP3FKUBZPVHTUEDLSX4BYGYKH4GCESXYHS3IHQ4EIG4")
    );
    assert_eq!(
        facts.share_token_id.as_deref(),
        Some("CDHKSAVRTWJ7KPMY3PTNJGUTWMLZUOUQE3QBXUBKBJL5YNCRNVW3VBC3")
    );
    assert_eq!(facts.virtual_assets, Some(0));
    assert_eq!(facts.paused, Some(false));
    // The live sample's external-assets slot [0][2][2] is non-zero.
    assert_eq!(facts.external_assets_present, Some(true));
    assert_eq!(
        facts.max_deposit_allowed,
        Some(i128::MAX),
        "max_deposit is element 2 of tuple 2"
    );
}

#[test]
fn adapter_map_absent_means_no_bindings() {
    assert_eq!(
        adapter_bindings_from(&serde_json::Map::new()),
        Some(Vec::new())
    );
}

#[test]
fn attested_queue_and_paused_vault_warn_instead_of_blocking() {
    let mut facts = crate::commands::upgrade::VaultFacts {
        vault_id: CONTRACT.to_string(),
        runtime_version: Some("1.1.1".to_string()),
        asset_token_id: Some(ASSET_CONTRACT.to_string()),
        share_token_id: Some(OTHER_CONTRACT.to_string()),
        governance_view: Some(CONTRACT.to_string()),
        governance_manifest: Some(CONTRACT.to_string()),
        paused: Some(true),
        virtual_assets: Some(5),
        external_assets_present: Some(false),
        max_deposit_allowed: Some(0),
        migration_flag: Some(false),
        fee_recipients: Some([(0u128, kernel_hex("GADMIN")), (0u128, kernel_hex("GADMIN"))]),
        adapter_bindings: Some(vec![]),
        pending_proposals: Some(Vec::new()),
        stale_proposals: Some(0),
        attest_empty_withdrawal_queue: true,
    };
    let report = crate::commands::upgrade::evaluate_vault_preflight(&facts);
    assert!(
        report.ok,
        "paused vault with attested queue must pass: {report:?}"
    );
    facts.attest_empty_withdrawal_queue = false;
    let report = crate::commands::upgrade::evaluate_vault_preflight(&facts);
    assert!(!report.ok, "unattested non-empty queue stays unverifiable");
}

#[test]
fn zero_fee_slot_with_banned_recipient_blocks() {
    let facts = crate::commands::upgrade::VaultFacts {
        vault_id: CONTRACT.to_string(),
        runtime_version: Some("1.1.1".to_string()),
        asset_token_id: Some(ASSET_CONTRACT.to_string()),
        share_token_id: Some(OTHER_CONTRACT.to_string()),
        governance_view: Some(CONTRACT.to_string()),
        governance_manifest: Some(CONTRACT.to_string()),
        paused: Some(false),
        virtual_assets: Some(0),
        external_assets_present: Some(false),
        max_deposit_allowed: Some(1000),
        migration_flag: Some(false),
        fee_recipients: Some([
            (0u128, kernel_hex(OTHER_CONTRACT)),
            (0u128, kernel_hex("GADMIN")),
        ]),
        adapter_bindings: Some(vec![]),
        pending_proposals: Some(Vec::new()),
        stale_proposals: Some(0),
        attest_empty_withdrawal_queue: false,
    };
    let report = crate::commands::upgrade::evaluate_vault_preflight(&facts);
    let fee = report
        .findings
        .iter()
        .find(|f| f.check == "fee_recipients")
        .expect("fee finding");
    assert_eq!(fee.status, PreflightStatus::Blocking);
}

#[test]
fn pending_id_parse_accepts_json_arrays_and_lists() {
    assert_eq!(parse_pending_ids("[1, 2]"), vec![1, 2]);
    assert_eq!(parse_pending_ids("1\n2\n"), vec![1, 2]);
}

fn kernel_hex(strkey: &str) -> String {
    crate::commands::upgrade::kernel_address(strkey)
}
