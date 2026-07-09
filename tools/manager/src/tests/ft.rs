use clap::Parser;
use serde_json::json;

use super::CREDS;
use crate::cli::{Cli, Command};
use crate::commands::ft::FtNs;

#[test]
fn parses_ft_get_balance_of_typed_args() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "ft",
        "get-balance-of",
        "--contract-id",
        "token.testnet",
        "--account-id",
        "alice.testnet",
    ])
    .expect("get-balance-of should parse");

    let params = match cli.command {
        Command::Ft {
            command: FtNs::GetBalanceOf(cmd),
        } => cmd.into_spec(),
        _ => panic!("expected Ft::GetBalanceOf"),
    };

    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "contract_id": "token.testnet", "account_id": "alice.testnet" })
    );
}

#[test]
fn parses_ft_transfer_typed_args() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "ft",
            "transfer",
            "--contract-id",
            "token.testnet",
            "--receiver-id",
            "bob.testnet",
            "--amount",
            "1234567890000000000000000",
            "--memo",
            "refund",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("transfer should parse");

    let params = match cli.command {
        Command::Ft {
            command: FtNs::Transfer(cmd),
        } => cmd.into_spec(),
        _ => panic!("expected Ft::Transfer"),
    };

    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "contract_id": "token.testnet", "receiver_id": "bob.testnet", "amount": "1234567890000000000000000", "memo": "refund" })
    );
}

#[test]
fn omits_ft_transfer_memo_when_absent() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "ft",
            "transfer",
            "--contract-id",
            "token.testnet",
            "--receiver-id",
            "bob.testnet",
            "--amount",
            "1",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("transfer without memo should parse");

    let params = match cli.command {
        Command::Ft {
            command: FtNs::Transfer(cmd),
        } => cmd.into_spec(),
        _ => panic!("expected Ft::Transfer"),
    };

    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "contract_id": "token.testnet", "receiver_id": "bob.testnet", "amount": "1" })
    );
}

#[test]
fn parses_ft_transfer_call_typed_args() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "ft",
            "transfer-call",
            "--contract-id",
            "token.testnet",
            "--receiver-id",
            "app.testnet",
            "--amount",
            "42",
            "--msg",
            r#"{"action":"stake"}"#,
            "--memo",
            "stake",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("transfer-call should parse");

    let params = match cli.command {
        Command::Ft {
            command: FtNs::TransferCall(cmd),
        } => cmd.into_spec(),
        _ => panic!("expected Ft::TransferCall"),
    };

    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "contract_id": "token.testnet", "receiver_id": "app.testnet", "amount": "42", "msg": r#"{"action":"stake"}"#, "memo": "stake" })
    );
}

#[test]
fn parses_ft_kebab_case_aliases() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "ft",
        "get-balance-of",
        "--contract-id",
        "token.testnet",
        "--account-id",
        "alice.testnet",
    ])
    .expect("get-balance-of should parse");
    match cli.command {
        Command::Ft { .. } => {}
        _ => panic!("expected Ft variant"),
    }

    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "ft",
            "transfer-call",
            "--contract-id",
            "token.testnet",
            "--receiver-id",
            "app.testnet",
            "--amount",
            "42",
            "--msg",
            "stake",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("transfer-call should parse");
    match cli.command {
        Command::Ft { .. } => {}
        _ => panic!("expected Ft variant"),
    }
}
