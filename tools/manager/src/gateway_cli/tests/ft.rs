use serde_json::json;

use super::parse_cli;

#[test]
fn parses_ft_get_balance_of_typed_args() {
    let cli = parse_cli([
        "tmplrmgr",
        "ft",
        "getBalanceOf",
        "--contract-id",
        "token.testnet",
        "--account-id",
        "alice.testnet",
    ]);

    assert_eq!(cli.rpc_method(), "ft.getBalanceOf");
    assert_eq!(
        cli.params(),
        &json!({ "contract_id": "token.testnet", "account_id": "alice.testnet" })
    );
}

#[test]
fn parses_ft_transfer_typed_args() {
    let cli = parse_cli([
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
    ]);

    assert_eq!(cli.rpc_method(), "ft.transfer");
    assert_eq!(
        cli.params(),
        &json!({ "contract_id": "token.testnet", "receiver_id": "bob.testnet", "amount": "1234567890000000000000000", "memo": "refund" })
    );
}

#[test]
fn omits_ft_transfer_memo_when_absent() {
    let cli = parse_cli([
        "tmplrmgr",
        "ft",
        "transfer",
        "--contract-id",
        "token.testnet",
        "--receiver-id",
        "bob.testnet",
        "--amount",
        "1",
    ]);

    assert_eq!(cli.rpc_method(), "ft.transfer");
    assert_eq!(
        cli.params(),
        &json!({ "contract_id": "token.testnet", "receiver_id": "bob.testnet", "amount": "1" })
    );
}

#[test]
fn parses_ft_transfer_call_typed_args() {
    let cli = parse_cli([
        "tmplrmgr",
        "ft",
        "transferCall",
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
    ]);

    assert_eq!(cli.rpc_method(), "ft.transferCall");
    assert_eq!(
        cli.params(),
        &json!({ "contract_id": "token.testnet", "receiver_id": "app.testnet", "amount": "42", "msg": r#"{"action":"stake"}"#, "memo": "stake" })
    );
}

#[test]
fn parses_ft_kebab_case_aliases() {
    let get_balance = parse_cli([
        "tmplrmgr",
        "ft",
        "get-balance-of",
        "--contract-id",
        "token.testnet",
        "--account-id",
        "alice.testnet",
    ]);
    let transfer_call = parse_cli([
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
    ]);

    assert_eq!(get_balance.rpc_method(), "ft.getBalanceOf");
    assert_eq!(transfer_call.rpc_method(), "ft.transferCall");
}

#[test]
fn json_params_take_precedence_over_ft_typed_args() {
    let cli = parse_cli([
        "tmplrmgr",
        "ft",
        "transfer",
        "--contract-id",
        "typed.testnet",
        "--receiver-id",
        "typed-receiver.testnet",
        "--amount",
        "1",
        "--json",
        r#"{"contract_id":"json.testnet","receiver_id":"json-receiver.testnet","amount":"99"}"#,
    ]);

    assert_eq!(cli.rpc_method(), "ft.transfer");
    assert_eq!(
        cli.params(),
        &json!({ "contract_id": "json.testnet", "receiver_id": "json-receiver.testnet", "amount": "99" })
    );
}
