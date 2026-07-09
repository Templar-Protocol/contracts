use clap::Parser;
use serde_json::json;

use super::CREDS;
use crate::cli::{Cli, Command};
use crate::commands::storage::StorageNs;

#[test]
fn parses_storage_get_balance_bounds_typed_args() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "storage",
        "get-balance-bounds",
        "--contract-id",
        "storage.testnet",
    ])
    .expect("get-balance-bounds should parse");

    let params = match cli.command {
        Command::Storage {
            command: StorageNs::GetBalanceBounds(cmd),
        } => cmd.into_spec(),
        _ => panic!("expected Storage::GetBalanceBounds"),
    };
    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "contract_id": "storage.testnet" })
    );
}

#[test]
fn parses_storage_get_balance_of_typed_args() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "storage",
        "get-balance-of",
        "--contract-id",
        "storage.testnet",
        "--account-id",
        "alice.testnet",
    ])
    .expect("get-balance-of should parse");

    let params = match cli.command {
        Command::Storage {
            command: StorageNs::GetBalanceOf(cmd),
        } => cmd.into_spec(),
        _ => panic!("expected Storage::GetBalanceOf"),
    };
    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "contract_id": "storage.testnet", "account_id": "alice.testnet" })
    );
}

#[test]
fn parses_storage_deposit_typed_args() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "storage",
            "deposit",
            "--contract-id",
            "storage.testnet",
            "--beneficiary-id",
            "beneficiary.testnet",
            "--registration-only",
            "--deposit",
            "1.25 NEAR",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("deposit should parse");

    let params = match cli.command {
        Command::Storage {
            command: StorageNs::Deposit(cmd),
        } => cmd.into_spec(),
        _ => panic!("expected Storage::Deposit"),
    };
    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "contract_id": "storage.testnet", "beneficiary_id": "beneficiary.testnet", "registration_only": true, "deposit": "1250000000000000000000000" })
    );
}

#[test]
fn parses_storage_unregister_typed_args() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "storage",
            "unregister",
            "--contract-id",
            "storage.testnet",
            "--force",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("unregister should parse");

    let params = match cli.command {
        Command::Storage {
            command: StorageNs::Unregister(cmd),
        } => cmd.into_spec(),
        _ => panic!("expected Storage::Unregister"),
    };
    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "contract_id": "storage.testnet", "force": true })
    );
}

#[test]
fn parses_storage_ensure_deposit_typed_args() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "storage",
            "ensure-deposit",
            "--contract-id",
            "storage.testnet",
            "--account-id",
            "alice.testnet",
            "--mode",
            "minimum-total",
            "--amount",
            "2.5 NEAR",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("ensure-deposit should parse");

    let params = match cli.command {
        Command::Storage {
            command: StorageNs::EnsureDeposit(cmd),
        } => cmd.try_into_spec().expect("ensure-deposit should parse"),
        _ => panic!("expected Storage::EnsureDeposit"),
    };
    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "contract_id": "storage.testnet", "account_id": "alice.testnet", "mode": { "mode": "minimum_total", "amount": "2500000000000000000000000" } })
    );
}

#[test]
fn parses_storage_kebab_case_aliases() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "storage",
        "get-balance-bounds",
        "--contract-id",
        "storage.testnet",
    ])
    .expect("get-balance-bounds should parse");
    match cli.command {
        Command::Storage { .. } => {}
        _ => panic!("expected Storage variant"),
    }

    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "storage",
        "get-balance-of",
        "--contract-id",
        "storage.testnet",
        "--account-id",
        "alice.testnet",
    ])
    .expect("get-balance-of should parse");
    match cli.command {
        Command::Storage { .. } => {}
        _ => panic!("expected Storage variant"),
    }

    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "storage",
            "ensure-deposit",
            "--contract-id",
            "storage.testnet",
            "--account-id",
            "alice.testnet",
            "--mode",
            "registered",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("ensure-deposit should parse");
    match cli.command {
        Command::Storage { .. } => {}
        _ => panic!("expected Storage variant"),
    }
}

#[test]
fn registered_ensure_deposit_rejects_amount() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "storage",
            "ensure-deposit",
            "--contract-id",
            "storage.testnet",
            "--account-id",
            "alice.testnet",
            "--mode",
            "registered",
            "--amount",
            "1 yoctoNEAR",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("registered ensure-deposit should parse before typed validation");

    let error = match cli.command {
        Command::Storage {
            command: StorageNs::EnsureDeposit(cmd),
        } => cmd
            .try_into_spec()
            .expect_err("registered mode should reject --amount"),
        _ => panic!("expected Storage::EnsureDeposit"),
    };

    assert!(error
        .to_string()
        .contains("--amount is only valid for minimum_total or minimum_available mode"));
}
