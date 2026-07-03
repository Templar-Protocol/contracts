use clap::{value_parser, Arg, ArgMatches, Command};
use near_account_id::AccountId;
use serde_json::{json, Map, Value};

pub(super) fn supports(rpc_method: &str) -> bool {
    matches!(
        rpc_method,
        "registry.listVersions"
            | "registry.listDeployments"
            | "registry.listDeploymentsByKind"
            | "registry.getDeployment"
    )
}

pub(super) fn apply_typed_args(command: Command, rpc_method: &str) -> Command {
    match rpc_method {
        "registry.listVersions" => command
            .visible_alias("list-versions")
            .arg(registry_id_arg())
            .arg(offset_arg())
            .arg(limit_arg()),
        "registry.listDeployments" => command
            .visible_alias("list-deployments")
            .arg(registry_id_arg())
            .arg(offset_arg())
            .arg(limit_arg()),
        "registry.listDeploymentsByKind" => command
            .visible_alias("list-deployments-by-kind")
            .arg(registry_id_arg())
            .arg(offset_arg())
            .arg(limit_arg())
            .arg(kind_arg()),
        "registry.getDeployment" => command
            .visible_alias("get-deployment")
            .arg(registry_id_arg())
            .arg(super::account_id_arg()),
        _ => command,
    }
}

pub(super) fn load_typed_params(
    matches: &ArgMatches,
    rpc_method: &str,
) -> anyhow::Result<Option<Value>> {
    let Some(registry_id) = matches.get_one::<AccountId>("registry-id") else {
        return Ok(None);
    };

    let mut params = Map::new();
    params.insert("registry_id".to_owned(), json!(registry_id));

    match rpc_method {
        "registry.listVersions" | "registry.listDeployments" => {
            add_pagination(matches, &mut params)
        }
        "registry.listDeploymentsByKind" => add_deployments_by_kind(matches, &mut params),
        "registry.getDeployment" => super::add_account_id(matches, &mut params),
        _ => return Ok(None),
    }

    Ok(Some(Value::Object(params)))
}

fn add_deployments_by_kind(matches: &ArgMatches, params: &mut Map<String, Value>) {
    add_pagination(matches, params);
    if let Some(kind) = matches.get_one::<String>("kind") {
        params.insert("kind".to_owned(), json!(kind));
    }
}

fn add_pagination(matches: &ArgMatches, params: &mut Map<String, Value>) {
    if let Some(offset) = matches.get_one::<u32>("offset") {
        params.insert("offset".to_owned(), json!(offset));
    }
    if let Some(limit) = matches.get_one::<u32>("limit") {
        params.insert("limit".to_owned(), json!(limit));
    }
}

fn registry_id_arg() -> Arg {
    Arg::new("registry-id")
        .long("registry-id")
        .value_name("ACCOUNT_ID")
        .value_parser(value_parser!(AccountId))
        .required_unless_present_any(["json", "json-file"])
        .help("Registry contract account ID")
}

fn offset_arg() -> Arg {
    Arg::new("offset")
        .long("offset")
        .value_name("COUNT")
        .value_parser(value_parser!(u32))
        .help("Number of records to skip")
}

fn limit_arg() -> Arg {
    Arg::new("limit")
        .long("limit")
        .value_name("COUNT")
        .value_parser(value_parser!(u32))
        .help("Maximum number of records to return")
}

fn kind_arg() -> Arg {
    Arg::new("kind")
        .long("kind")
        .value_name("KIND")
        .value_parser([
            "unknown",
            "registry",
            "market",
            "vault",
            "proxy_oracle",
            "proxy_governance",
            "lst_oracle",
            "universal_account",
            "redstone_oracle",
            "pyth_oracle",
        ])
        .required_unless_present_any(["json", "json-file"])
        .help("Deployment contract kind")
}
