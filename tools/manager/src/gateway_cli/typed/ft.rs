use clap::{value_parser, Arg, ArgMatches, Command};
use near_account_id::AccountId;
use serde_json::{json, Map, Value};

pub(super) fn supports(rpc_method: &str) -> bool {
    matches!(
        rpc_method,
        "ft.getBalanceOf" | "ft.transfer" | "ft.transferCall"
    )
}

pub(super) fn apply_typed_args(command: Command, rpc_method: &str) -> Command {
    match rpc_method {
        "ft.getBalanceOf" => command
            .visible_alias("get-balance-of")
            .arg(super::contract_id_arg())
            .arg(super::account_id_arg()),
        "ft.transfer" => command
            .arg(super::contract_id_arg())
            .arg(receiver_id_arg())
            .arg(amount_arg())
            .arg(memo_arg()),
        "ft.transferCall" => command
            .visible_alias("transfer-call")
            .arg(super::contract_id_arg())
            .arg(receiver_id_arg())
            .arg(amount_arg())
            .arg(msg_arg())
            .arg(memo_arg()),
        _ => command,
    }
}

pub(super) fn load_typed_params(
    matches: &ArgMatches,
    rpc_method: &str,
) -> anyhow::Result<Option<Value>> {
    let Some(contract_id) = matches.get_one::<AccountId>("contract-id") else {
        return Ok(None);
    };

    let mut params = Map::new();
    params.insert("contract_id".to_owned(), json!(contract_id));

    match rpc_method {
        "ft.getBalanceOf" => super::add_account_id(matches, &mut params),
        "ft.transfer" => add_transfer_params(matches, &mut params),
        "ft.transferCall" => add_transfer_call_params(matches, &mut params),
        _ => return Ok(None),
    }

    Ok(Some(Value::Object(params)))
}

fn add_transfer_params(matches: &ArgMatches, params: &mut Map<String, Value>) {
    add_receiver_id(matches, params);
    add_amount(matches, params);
    add_memo(matches, params);
}

fn add_transfer_call_params(matches: &ArgMatches, params: &mut Map<String, Value>) {
    add_transfer_params(matches, params);
    if let Some(msg) = matches.get_one::<String>("msg") {
        params.insert("msg".to_owned(), json!(msg));
    }
}

fn add_receiver_id(matches: &ArgMatches, params: &mut Map<String, Value>) {
    if let Some(receiver_id) = matches.get_one::<AccountId>("receiver-id") {
        params.insert("receiver_id".to_owned(), json!(receiver_id));
    }
}

fn add_amount(matches: &ArgMatches, params: &mut Map<String, Value>) {
    if let Some(amount) = matches.get_one::<u128>("amount") {
        params.insert("amount".to_owned(), json!(amount.to_string()));
    }
}

fn add_memo(matches: &ArgMatches, params: &mut Map<String, Value>) {
    if let Some(memo) = matches.get_one::<String>("memo") {
        params.insert("memo".to_owned(), json!(memo));
    }
}

fn receiver_id_arg() -> Arg {
    Arg::new("receiver-id")
        .long("receiver-id")
        .value_name("ACCOUNT_ID")
        .value_parser(value_parser!(AccountId))
        .required_unless_present_any(["json", "json-file"])
        .help("Receiver account ID")
}

fn amount_arg() -> Arg {
    Arg::new("amount")
        .long("amount")
        .value_name("AMOUNT")
        .value_parser(value_parser!(u128))
        .required_unless_present_any(["json", "json-file"])
        .help("Token amount as a base-unit integer")
}

fn memo_arg() -> Arg {
    Arg::new("memo")
        .long("memo")
        .value_name("TEXT")
        .help("Optional transfer memo")
}

fn msg_arg() -> Arg {
    Arg::new("msg")
        .long("msg")
        .value_name("TEXT")
        .required_unless_present_any(["json", "json-file"])
        .help("Receiver callback message")
}
