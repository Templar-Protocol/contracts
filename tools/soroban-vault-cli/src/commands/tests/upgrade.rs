use super::*;

use base64::Engine as _;

use crate::{
    cli::{UpgradeArgs, UpgradeCommand, UpgradePreflightArgs},
    commands::{
        output::PreflightStatus,
        upgrade::{decode_fee_recipients, parse_instance_storage, parse_pending_ids},
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
    let cli = Cli {
        json: false,
        ..cli
    };
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
        fee_recipients: Some([
            (0u128, kernel_hex("GADMIN")),
            (0u128, kernel_hex("GADMIN")),
        ]),
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
        fee_recipients: Some([
            (1u128, kernel_hex(CONTRACT)),
            (0u128, kernel_hex("GADMIN")),
        ]),
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
    assert_eq!(status("op_state_idle"), Some(PreflightStatus::Blocking));
    assert_eq!(status("external_assets"), Some(PreflightStatus::Warn));
    assert_eq!(status("withdrawal_queue"), Some(PreflightStatus::Unverifiable));
    assert!(!report.ok);
}

#[test]
fn kernel_address_matches_runtime_domain_separation() {
    // Runtime stores sha256(domain || 0x01 || strkey) for fee recipients.
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"templar:soroban:address");
    hasher.update([1u8]);
    hasher.update(ACCOUNT.as_bytes());
    assert_eq!(
        kernel_hex(ACCOUNT),
        hex::encode(hasher.finalize())
    );
}

#[test]
fn duplicate_adapter_detection() {
    assert!(crate::commands::upgrade::duplicate_adapters(&[
        (1, OTHER_CONTRACT.to_string())
    ])
    .is_empty());
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
    let fee = u128::to_be_bytes(42u128);
    let recipient = kernel_hex("GADMIN");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&fee);
    bytes.extend_from_slice(&hex::decode(&recipient).expect("recipient hex"));
    bytes.extend_from_slice(&u128::to_be_bytes(0u128));
    bytes.extend_from_slice(&hex::decode(&recipient).expect("recipient hex"));
    let spec = decode_fee_recipients(
        &base64::engine::GeneralPurpose::new(
            &base64::alphabet::STANDARD,
            base64::engine::general_purpose::PAD,
        )
        .encode(bytes),
    )
    .expect("decode fee spec");
    assert_eq!(spec[0].0, 42);
    assert_eq!(spec[0].1, recipient);
}

#[test]
fn pending_id_parse_accepts_json_arrays_and_lists() {
    assert_eq!(parse_pending_ids("[1, 2]"), vec![1, 2]);
    assert_eq!(parse_pending_ids("1\n2\n"), vec![1, 2]);
}

fn kernel_hex(strkey: &str) -> String {
    crate::commands::upgrade::kernel_address(strkey)
}
