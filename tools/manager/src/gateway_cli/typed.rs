use clap::{value_parser, Arg, ArgMatches, Command};
use near_account_id::AccountId;
use serde_json::{json, Map, Value};

mod ft;
mod registry;
mod storage;

pub(super) fn supports_typed_args(rpc_method: &str) -> bool {
    registry::supports(rpc_method) || storage::supports(rpc_method) || ft::supports(rpc_method)
}

pub(super) fn apply_typed_args(command: Command, rpc_method: &str) -> Command {
    if registry::supports(rpc_method) {
        return registry::apply_typed_args(command, rpc_method);
    }
    if storage::supports(rpc_method) {
        return storage::apply_typed_args(command, rpc_method);
    }
    if ft::supports(rpc_method) {
        return ft::apply_typed_args(command, rpc_method);
    }

    command
}

pub(super) fn load_typed_params(
    matches: &ArgMatches,
    rpc_method: &str,
) -> anyhow::Result<Option<Value>> {
    if registry::supports(rpc_method) {
        return registry::load_typed_params(matches, rpc_method);
    }
    if storage::supports(rpc_method) {
        return storage::load_typed_params(matches, rpc_method);
    }
    if ft::supports(rpc_method) {
        return ft::load_typed_params(matches, rpc_method);
    }

    Ok(None)
}

fn add_account_id(matches: &ArgMatches, params: &mut Map<String, Value>) {
    if let Some(account_id) = matches.get_one::<AccountId>("account-id") {
        params.insert("account_id".to_owned(), json!(account_id));
    }
}

fn account_id_arg() -> Arg {
    Arg::new("account-id")
        .long("account-id")
        .value_name("ACCOUNT_ID")
        .value_parser(value_parser!(AccountId))
        .required_unless_present_any(["json", "json-file"])
        .help("Account ID")
}

fn contract_id_arg() -> Arg {
    Arg::new("contract-id")
        .long("contract-id")
        .value_name("ACCOUNT_ID")
        .value_parser(value_parser!(AccountId))
        .required_unless_present_any(["json", "json-file"])
        .help("Contract account ID")
}
