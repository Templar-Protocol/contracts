use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::Deserialize;

const PROFILE_DIR_ENV: &str = "TEMPLAR_SOROBAN_VAULT_PROFILE_DIR";
const PROFILE_ENV: &str = "TEMPLAR_SOROBAN_VAULT_PROFILE";

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProfileConfig {
    pub network: Option<String>,
    pub rpc_url: Option<String>,
    pub network_passphrase: Option<String>,
    pub state: Option<PathBuf>,
    pub workspace_path: Option<PathBuf>,
    pub contract_source_repo: Option<String>,
    pub config_dir: Option<PathBuf>,
    pub admin: Option<String>,
    pub caller: Option<String>,
    pub operator: Option<String>,
}

pub fn expand_args(raw_args: &[String]) -> anyhow::Result<Vec<String>> {
    let profile_dir = profile_dir()?;
    expand_args_with_dir(raw_args, &profile_dir)
}

pub fn init_profile(name: &str, force: bool) -> anyhow::Result<PathBuf> {
    validate_profile_name(name)?;
    let dir = profile_dir()?;
    fs::create_dir_all(&dir)
        .with_context(|| format!("create profile directory {}", dir.display()))?;
    let path = profile_path(&dir, name);
    if path.exists() && !force {
        anyhow::bail!(
            "profile {} already exists at {}; pass --force to overwrite",
            name,
            path.display()
        );
    }
    fs::write(&path, profile_template(name))
        .with_context(|| format!("write profile {}", path.display()))?;
    Ok(path)
}

fn expand_args_with_dir(raw_args: &[String], profile_dir: &Path) -> anyhow::Result<Vec<String>> {
    let Some(name) = profile_name(raw_args) else {
        return Ok(raw_args.to_vec());
    };
    validate_profile_name(&name)?;
    if profile_load_is_skipped(raw_args) {
        return Ok(raw_args.to_vec());
    }

    let profile = load_profile(profile_dir, &name)?;
    let mut expanded = Vec::with_capacity(raw_args.len() + 16);
    let Some(program) = raw_args.first() else {
        return Ok(raw_args.to_vec());
    };
    expanded.push(program.clone());
    push_global_profile_args(&mut expanded, raw_args, &profile);
    expanded.extend(raw_args.iter().skip(1).cloned());
    push_leaf_profile_args(&mut expanded, raw_args, &profile);
    Ok(expanded)
}

fn load_profile(dir: &Path, name: &str) -> anyhow::Result<ProfileConfig> {
    let path = profile_path(dir, name);
    let raw =
        fs::read_to_string(&path).with_context(|| format!("read profile {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parse profile {}", path.display()))
}

fn profile_dir() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(PROFILE_DIR_ENV).map(PathBuf::from) {
        return Ok(dir);
    }
    let base = dirs::config_dir().context("could not determine user config directory")?;
    Ok(base
        .join("templar")
        .join("soroban-vault-cli")
        .join("profiles"))
}

fn profile_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.toml"))
}

fn profile_name(raw_args: &[String]) -> Option<String> {
    flag_value(raw_args, "--profile")
        .or_else(|| std::env::var(PROFILE_ENV).ok())
        .filter(|value| !value.trim().is_empty())
}

fn validate_profile_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')),
        "profile names may only contain ASCII letters, digits, '-' and '_'"
    );
    Ok(())
}

fn profile_load_is_skipped(raw_args: &[String]) -> bool {
    matches!(
        primary_command(raw_args).as_deref(),
        Some("profile" | "completions" | "man")
    )
}

fn push_global_profile_args(
    expanded: &mut Vec<String>,
    raw_args: &[String],
    profile: &ProfileConfig,
) {
    push_profile_arg(
        expanded,
        raw_args,
        "--network",
        "SOROBAN_NETWORK",
        profile.network.as_deref(),
    );
    push_profile_arg(
        expanded,
        raw_args,
        "--rpc-url",
        "SOROBAN_RPC_URL",
        profile.rpc_url.as_deref(),
    );
    push_profile_arg(
        expanded,
        raw_args,
        "--network-passphrase",
        "SOROBAN_NETWORK_PASSPHRASE",
        profile.network_passphrase.as_deref(),
    );
    push_profile_arg(
        expanded,
        raw_args,
        "--state",
        "TEMPLAR_SOROBAN_VAULT_STATE",
        profile.state.as_deref().and_then(Path::to_str),
    );
    push_profile_arg(
        expanded,
        raw_args,
        "--workspace-path",
        "WORKSPACE_PATH",
        profile.workspace_path.as_deref().and_then(Path::to_str),
    );
    push_profile_arg(
        expanded,
        raw_args,
        "--contract-source-repo",
        "SOROBAN_CONTRACT_SOURCE_REPO",
        profile.contract_source_repo.as_deref(),
    );
    push_profile_arg(
        expanded,
        raw_args,
        "--config-dir",
        "STELLAR_CONFIG_DIR",
        profile.config_dir.as_deref().and_then(Path::to_str),
    );
}

fn push_leaf_profile_args(
    expanded: &mut Vec<String>,
    raw_args: &[String],
    profile: &ProfileConfig,
) {
    if needs_admin(raw_args) {
        push_profile_arg(
            expanded,
            raw_args,
            "--admin",
            "SOROBAN_ADMIN",
            profile.admin.as_deref(),
        );
    }
    if needs_caller(raw_args) {
        push_profile_arg(
            expanded,
            raw_args,
            "--caller",
            "SOROBAN_CALLER",
            profile.caller.as_deref().or(profile.admin.as_deref()),
        );
    }
    if needs_operator(raw_args) {
        push_profile_arg(
            expanded,
            raw_args,
            "--operator",
            "SOROBAN_OPERATOR",
            profile.operator.as_deref(),
        );
    }
}

fn push_profile_arg(
    expanded: &mut Vec<String>,
    raw_args: &[String],
    flag: &str,
    env_var: &str,
    value: Option<&str>,
) {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    if has_flag(raw_args, flag) || std::env::var_os(env_var).is_some() {
        return;
    }
    expanded.push(flag.to_string());
    expanded.push(value.to_string());
}

fn needs_admin(raw_args: &[String]) -> bool {
    [
        "stack",
        "plan-accept",
        "plan-submit-set-supply-queue",
        "plan-submit-set-timelock",
        "accept-ready",
        "accept",
        "revoke",
        "submit-set-admin",
        "submit-set-curator",
        "submit-set-governance",
        "submit-set-paused",
        "submit-set-supply-queue",
        "submit-set-fees",
        "submit-set-restrictions",
        "submit-set-sentinel",
        "submit-set-allocators",
        "submit-set-allowed-adapters",
        "submit-set-timelock",
        "submit-set-cap",
        "submit-remove-market",
        "submit-set-group-cap",
        "submit-set-group-rel-cap",
        "submit-set-group-member",
        "submit-set-skim-recipient",
        "submit-skim",
        "submit-set-withdrawal-cooldown",
        "submit-set-idle-resync-cooldown",
        "submit-upgrade",
        "submit-migrate",
        "submit-cancel-migration",
        "abdicate",
        "set-allowed-adapters",
        "set-supply-queue",
    ]
    .into_iter()
    .any(|command| raw_args.iter().any(|arg| arg == command))
}

fn needs_caller(raw_args: &[String]) -> bool {
    [
        "extend-ttl",
        "allocate-supply",
        "allocate-withdraw",
        "refresh-markets",
    ]
    .into_iter()
    .any(|command| raw_args.iter().any(|arg| arg == command))
}

fn needs_operator(raw_args: &[String]) -> bool {
    ["deposit", "mint", "withdraw", "execute-withdraw"]
        .into_iter()
        .any(|command| raw_args.iter().any(|arg| arg == command))
}

fn has_flag(raw_args: &[String], flag: &str) -> bool {
    raw_args.iter().any(|arg| {
        arg == flag
            || arg
                .strip_prefix(flag)
                .is_some_and(|rest| rest.starts_with('='))
    })
}

fn flag_value(raw_args: &[String], flag: &str) -> Option<String> {
    raw_args.iter().enumerate().find_map(|(index, arg)| {
        if arg == flag {
            return raw_args.get(index + 1).cloned();
        }
        arg.strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
            .map(ToString::to_string)
    })
}

fn primary_command(raw_args: &[String]) -> Option<String> {
    let mut skip_next = false;
    for arg in raw_args.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if flag_takes_value(arg) {
            skip_next = !arg.contains('=');
            continue;
        }
        if !arg.starts_with('-') {
            return Some(arg.clone());
        }
    }
    None
}

fn flag_takes_value(arg: &str) -> bool {
    [
        "--profile",
        "--network",
        "--rpc-url",
        "--network-passphrase",
        "--source-account",
        "--config-dir",
        "--state",
        "--workspace-path",
        "--contract-source-repo",
    ]
    .into_iter()
    .any(|flag| arg == flag || arg.starts_with(&format!("{flag}=")))
}

fn profile_template(name: &str) -> String {
    let network = if name == "mainnet" {
        "mainnet"
    } else {
        "testnet"
    };
    let passphrase = if name == "mainnet" {
        "Public Global Stellar Network ; September 2015"
    } else {
        "Test SDF Network ; September 2015"
    };
    format!(
        r#"# Templar Soroban vault CLI profile.
# Store only public config and public addresses here. Do not put secret keys or seed phrases in profiles.
network = "{network}"
network_passphrase = "{passphrase}"
state = "contract/vault/soroban/.deploy-state/{name}.manifest.json"
workspace_path = "."

# Optional public config.
# rpc_url = "https://..."
# config_dir = "/home/operator/.config/stellar"
# contract_source_repo = "github:Templar-Protocol/contracts"
# admin = "G..."
# caller = "G..."
# operator = "G..."
"#
    )
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use crate::cli::Cli;

    use super::*;

    #[test]
    fn profile_expansion_injects_defaults_without_overriding_cli_flags() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("testnet.toml"),
            r#"
network = "testnet"
rpc_url = "https://rpc.example"
state = "state.json"
workspace_path = "/workspace"
contract_source_repo = "github:example/contracts"
admin = "GADMIN"
"#,
        )
        .expect("write profile");

        let expanded = expand_args_with_dir(
            &[
                "tmplr-soroban-vault".to_string(),
                "--profile".to_string(),
                "testnet".to_string(),
                "--network".to_string(),
                "local".to_string(),
                "governance".to_string(),
                "submit-set-admin".to_string(),
                "--new-admin".to_string(),
                "GNEW".to_string(),
            ],
            dir.path(),
        )
        .expect("expand args");

        assert!(expanded
            .windows(2)
            .any(|pair| pair == ["--network", "local"]));
        assert!(expanded
            .windows(2)
            .any(|pair| pair == ["--rpc-url", "https://rpc.example"]));
        assert!(expanded
            .windows(2)
            .any(|pair| pair == ["--contract-source-repo", "github:example/contracts"]));
        assert!(expanded
            .windows(2)
            .any(|pair| pair == ["--admin", "GADMIN"]));
    }

    #[test]
    fn profile_expansion_rejects_path_names() {
        let err = expand_args_with_dir(
            &[
                "tmplr-soroban-vault".to_string(),
                "--profile".to_string(),
                "../secret".to_string(),
                "status".to_string(),
            ],
            Path::new("."),
        )
        .expect_err("profile name should be rejected");

        assert!(err.to_string().contains("profile names"));
    }

    #[test]
    fn profile_expansion_does_not_parse_dash_prefixed_values_as_flags() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("testnet.toml"),
            r#"
network = "--allow-mainnet-write"
"#,
        )
        .expect("write profile");

        let expanded = expand_args_with_dir(
            &[
                "tmplr-soroban-vault".to_string(),
                "--profile".to_string(),
                "testnet".to_string(),
                "status".to_string(),
            ],
            dir.path(),
        )
        .expect("expand args");
        let parsed = Cli::try_parse_from(expanded);

        match parsed {
            Ok(cli) => assert!(
                !cli.allow_mainnet_write,
                "profile value was parsed as a security flag"
            ),
            Err(error) => assert!(
                !matches!(
                    error.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                ),
                "profile parsing should not route dash-prefixed values to meta flags"
            ),
        }
    }

    #[test]
    fn profile_init_template_warns_against_secrets() {
        let template = profile_template("testnet");
        assert!(template.contains("Do not put secret keys"));
        assert!(template.contains("network = \"testnet\""));
        assert!(template.contains("contract_source_repo"));
    }

    #[test]
    fn primary_command_skips_profile_value() {
        assert_eq!(
            primary_command(&[
                "tmplr-soroban-vault".to_string(),
                "--profile".to_string(),
                "testnet".to_string(),
                "status".to_string(),
            ])
            .as_deref(),
            Some("status")
        );
    }
}
