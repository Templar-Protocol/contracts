use clap::{value_parser, Arg, ArgAction, ArgMatches, Command};
use near_account_id::AccountId;
use serde_json::{json, Map, Value};

pub(super) fn supports(rpc_method: &str) -> bool {
    matches!(
        rpc_method,
        "storage.getBalanceBounds"
            | "storage.getBalanceOf"
            | "storage.deposit"
            | "storage.unregister"
            | "storage.ensureDeposit"
    )
}

pub(super) fn apply_typed_args(command: Command, rpc_method: &str) -> Command {
    match rpc_method {
        "storage.getBalanceBounds" => command
            .visible_alias("get-balance-bounds")
            .arg(super::contract_id_arg()),
        "storage.getBalanceOf" => command
            .visible_alias("get-balance-of")
            .arg(super::contract_id_arg())
            .arg(super::account_id_arg()),
        "storage.deposit" => command
            .arg(super::contract_id_arg())
            .arg(beneficiary_id_arg())
            .arg(registration_only_arg())
            .arg(deposit_arg()),
        "storage.unregister" => command.arg(super::contract_id_arg()).arg(force_arg()),
        "storage.ensureDeposit" => command
            .visible_alias("ensure-deposit")
            .arg(super::contract_id_arg())
            .arg(super::account_id_arg())
            .arg(mode_arg())
            .arg(amount_arg()),
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
        "storage.getBalanceBounds" => {}
        "storage.getBalanceOf" => super::add_account_id(matches, &mut params),
        "storage.deposit" => add_storage_deposit(matches, &mut params),
        "storage.unregister" => {
            params.insert("force".to_owned(), json!(matches.get_flag("force")));
        }
        "storage.ensureDeposit" => add_ensure_deposit(matches, &mut params)?,
        _ => return Ok(None),
    }

    Ok(Some(Value::Object(params)))
}

fn add_storage_deposit(matches: &ArgMatches, params: &mut Map<String, Value>) {
    if let Some(beneficiary_id) = matches.get_one::<AccountId>("beneficiary-id") {
        params.insert("beneficiary_id".to_owned(), json!(beneficiary_id));
    }
    params.insert(
        "registration_only".to_owned(),
        json!(matches.get_flag("registration-only")),
    );
    if let Some(deposit) = matches.get_one::<u128>("deposit") {
        params.insert("deposit".to_owned(), json!(deposit.to_string()));
    }
}

fn add_ensure_deposit(matches: &ArgMatches, params: &mut Map<String, Value>) -> anyhow::Result<()> {
    super::add_account_id(matches, params);

    let Some(mode) = matches.get_one::<String>("mode") else {
        return Ok(());
    };

    let mode = match mode.as_str() {
        "registered" => {
            if matches.get_one::<u128>("amount").is_some() {
                anyhow::bail!("--amount is only valid for minimum_total or minimum_available mode");
            }
            json!({ "mode": "registered" })
        }
        "minimum_total" | "minimum_available" => {
            let amount = matches
                .get_one::<u128>("amount")
                .ok_or_else(|| anyhow::anyhow!("--amount is required for {mode} mode"))?;
            json!({ "mode": mode, "amount": amount.to_string() })
        }
        _ => return Ok(()),
    };
    params.insert("mode".to_owned(), mode);

    Ok(())
}

fn beneficiary_id_arg() -> Arg {
    Arg::new("beneficiary-id")
        .long("beneficiary-id")
        .value_name("ACCOUNT_ID")
        .value_parser(value_parser!(AccountId))
        .help("Account ID to register; defaults to the signer")
}

fn registration_only_arg() -> Arg {
    Arg::new("registration-only")
        .long("registration-only")
        .action(ArgAction::SetTrue)
        .help("Only register the account; do not leave additional available balance")
}

fn deposit_arg() -> Arg {
    Arg::new("deposit")
        .long("deposit")
        .value_name("YOCTONEAR")
        .value_parser(value_parser!(u128))
        .required_unless_present_any(["json", "json-file"])
        .help("Storage deposit amount in yoctoNEAR")
}

fn force_arg() -> Arg {
    Arg::new("force")
        .long("force")
        .action(ArgAction::SetTrue)
        .help("Force storage unregistration")
}

fn mode_arg() -> Arg {
    Arg::new("mode")
        .long("mode")
        .value_name("MODE")
        .value_parser(["registered", "minimum_total", "minimum_available"])
        .required_unless_present_any(["json", "json-file"])
        .help("Storage ensure mode")
}

fn amount_arg() -> Arg {
    Arg::new("amount")
        .long("amount")
        .value_name("YOCTONEAR")
        .value_parser(value_parser!(u128))
        .help("Required yoctoNEAR amount for minimum_total or minimum_available")
}
