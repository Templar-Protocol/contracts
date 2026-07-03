use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{value_parser, Arg, ArgAction, ArgGroup, Command};
use near_account_id::AccountId;
use templar_gateway_client::Network;
use templar_gateway_types::{MethodKind, RpcMethodMeta};

#[derive(Clone, Copy)]
struct MethodEntry {
    namespace: &'static str,
    name: &'static str,
    rpc_method: &'static str,
    kind: MethodKind,
    summary: &'static str,
    description: &'static str,
}

impl MethodEntry {
    fn of<Spec: RpcMethodMeta>() -> Self {
        let (namespace, name) = match Spec::RPC_METHOD.split_once('.') {
            Some((namespace, name)) => (namespace, name),
            None => (Spec::RPC_METHOD, Spec::RPC_METHOD),
        };

        Self {
            namespace,
            name,
            rpc_method: Spec::RPC_METHOD,
            kind: Spec::KIND,
            summary: Spec::SUMMARY,
            description: Spec::DESCRIPTION,
        }
    }
}

pub(super) fn command() -> Command {
    let mut namespaces = BTreeMap::<&'static str, Vec<MethodEntry>>::new();
    for entry in method_entries() {
        namespaces.entry(entry.namespace).or_default().push(entry);
    }

    let mut command = Command::new("tmplrmgr")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Gateway-native CLI frontend for Templar operations")
        .arg_required_else_help(true)
        .subcommand_required(true)
        .arg(
            Arg::new("network")
                .short('n')
                .long("network")
                .env("NETWORK")
                .default_value("testnet")
                .global(true)
                .value_parser(value_parser!(Network))
                .help("NEAR network to connect to"),
        )
        .arg(
            Arg::new("rpc-url")
                .long("rpc-url")
                .env("RPC_URL")
                .hide_env_values(true)
                .global(true)
                .value_name("URL")
                .help("Override the default RPC URL for the selected network"),
        )
        .arg(
            Arg::new("rpc-api-key")
                .long("rpc-api-key")
                .env("RPC_API_KEY")
                .hide_env_values(true)
                .global(true)
                .value_name("KEY")
                .help("API key for the RPC endpoint"),
        )
        .arg(
            Arg::new("signer-id")
                .long("signer-id")
                .env("SIGNER_ID")
                .global(true)
                .value_name("ACCOUNT_ID")
                .value_parser(value_parser!(AccountId))
                .help("Gateway signer account for write methods"),
        )
        .arg(
            Arg::new("secret-key")
                .long("secret-key")
                .env("SECRET_KEY")
                .hide_env_values(true)
                .global(true)
                .value_name("SECRET_KEY")
                .help("Secret key for --signer-id"),
        )
        .arg(
            Arg::new("gateway-store-url")
                .long("gateway-store-url")
                .env("GATEWAY_DATABASE_URL")
                .hide_env_values(true)
                .global(true)
                .value_name("URL")
                .help("Postgres URL for durable gateway operation storage"),
        )
        .arg(
            Arg::new("migrate-gateway-store")
                .long("migrate-gateway-store")
                .env("GATEWAY_DATABASE_MIGRATE")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("Run gateway operation store migrations before dispatch"),
        )
        .arg(
            Arg::new("idempotency-key")
                .long("idempotency-key")
                .global(true)
                .value_name("KEY")
                .requires("gateway-store-url")
                .help("Durable gateway idempotency key for write methods"),
        )
        .arg(
            Arg::new("quiet")
                .short('q')
                .long("quiet")
                .global(true)
                .action(ArgAction::Count)
                .conflicts_with("verbose")
                .help("Reduce console log verbosity"),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .global(true)
                .action(ArgAction::Count)
                .conflicts_with("quiet")
                .help("Increase console log verbosity"),
        );

    for (namespace, entries) in namespaces {
        let mut namespace_command = Command::new(namespace)
            .about(format!("{namespace}.* gateway methods"))
            .subcommand_required(true)
            .arg_required_else_help(true);

        for entry in entries {
            namespace_command = namespace_command.subcommand(method_command(entry));
        }

        command = command.subcommand(namespace_command);
    }

    command
}

fn method_command(entry: MethodEntry) -> Command {
    let command = Command::new(entry.name)
        .about(entry.summary)
        .long_about(entry.description)
        .after_help(format!(
            "Gateway method: {} ({:?})",
            entry.rpc_method, entry.kind
        ))
        .arg(
            Arg::new("json")
                .long("json")
                .value_name("JSON")
                .help("Method parameters as a JSON object"),
        )
        .arg(
            Arg::new("json-file")
                .long("json-file")
                .value_name("PATH")
                .value_parser(value_parser!(PathBuf))
                .help("Path to a JSON file containing method parameters; use '-' for stdin"),
        );

    if super::typed::supports_typed_args(entry.rpc_method) {
        let command = super::typed::apply_typed_args(command, entry.rpc_method);
        command.group(
            ArgGroup::new("params")
                .args(["json", "json-file"])
                .required(false)
                .multiple(false),
        )
    } else {
        command.arg_required_else_help(true).group(
            ArgGroup::new("params")
                .args(["json", "json-file"])
                .required(true)
                .multiple(false),
        )
    }
}

fn method_entries() -> Vec<MethodEntry> {
    let mut entries = Vec::new();
    macro_rules! push {
        ($spec:ty) => {
            entries.push(MethodEntry::of::<$spec>());
        };
    }
    templar_gateway_methods_spec::for_each_read_method!(push);
    templar_gateway_methods_spec::for_each_write_method!(push);
    push!(templar_gateway_methods_spec::op::Get);
    entries.sort_by(|left, right| left.rpc_method.cmp(right.rpc_method));
    entries
}
