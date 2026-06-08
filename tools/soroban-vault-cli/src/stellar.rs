use std::{collections::BTreeSet, process::Command};

use anyhow::Context;
use zeroize::Zeroize;
use zeroize::Zeroizing;

use crate::cli::Cli;
use crate::types::SourceAccount;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}

pub struct CommandEnv {
    key: &'static str,
    value: Zeroizing<String>,
    redact: bool,
}

impl CommandEnv {
    fn redacted(key: &'static str, value: String) -> Self {
        Self {
            key,
            value: Zeroizing::new(value),
            redact: true,
        }
    }
}

pub trait CommandExecutor {
    fn run(
        &self,
        program: &str,
        args: &[String],
        redacted_args: &[usize],
        env: &[CommandEnv],
    ) -> anyhow::Result<CommandOutput>;
}

pub struct RealExecutor;

impl CommandExecutor for RealExecutor {
    fn run(
        &self,
        program: &str,
        args: &[String],
        redacted_args: &[usize],
        env: &[CommandEnv],
    ) -> anyhow::Result<CommandOutput> {
        let mut command = Command::new(program);
        command.args(args);
        for var in env {
            command.env(var.key, var.value.as_str());
        }
        let output = command.output().with_context(|| {
            format!("run {}", display_command(program, args, redacted_args, env))
        })?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::ensure!(
            output.status.success(),
            "command failed: {}\n{}",
            display_command(program, args, redacted_args, env),
            stderr
        );
        Ok(CommandOutput { stdout, stderr })
    }
}

pub struct Stellar<'a, E: CommandExecutor> {
    cli: &'a Cli,
    executor: &'a E,
}

impl<'a, E: CommandExecutor> Stellar<'a, E> {
    pub fn new(cli: &'a Cli, executor: &'a E) -> Self {
        Self { cli, executor }
    }

    pub fn run(
        &self,
        mut args: Vec<String>,
        redacted_args: &[usize],
        mut env: Vec<CommandEnv>,
    ) -> anyhow::Result<CommandOutput> {
        let result = if self.cli.dry_run {
            println!(
                "dry-run: {}",
                display_command("stellar", &args, redacted_args, &env)
            );
            Ok(CommandOutput {
                stdout: String::new(),
                stderr: String::new(),
            })
        } else {
            self.executor.run("stellar", &args, redacted_args, &env)
        };
        zeroize_redacted_args(&mut args, redacted_args);
        zeroize_env(&mut env);
        result
    }

    pub fn network_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        args.extend(["--network".to_string(), self.cli.network.clone()]);
        args.extend([
            "--network-passphrase".to_string(),
            self.cli.network_passphrase.clone(),
        ]);
        if let Some(rpc_url) = &self.cli.rpc_url {
            args.extend(["--rpc-url".to_string(), rpc_url.clone()]);
        }
        if let Some(config_dir) = &self.cli.config_dir {
            args.extend(["--config-dir".to_string(), config_dir.display().to_string()]);
        }
        args
    }

    pub fn source_env(&self) -> Vec<CommandEnv> {
        self.cli
            .source_account
            .as_ref()
            .map(|source| CommandEnv::redacted("STELLAR_ACCOUNT", source.clone_secret()))
            .into_iter()
            .collect()
    }

    pub fn keys_address_source_account(&self) -> anyhow::Result<String> {
        let (args, redacted_args) = keys_address_source_account_args(
            self.cli.source_account.as_ref(),
            std::env::var_os("STELLAR_ACCOUNT").is_some(),
        )?;
        let out = self.run(args, &redacted_args, Vec::new())?;
        if self.cli.dry_run {
            return Ok("GDRYRUNSOURCEACCOUNT".to_string());
        }
        anyhow::ensure!(
            !out.stdout.is_empty(),
            "stellar keys address returned no address"
        );
        Ok(out.stdout)
    }

    pub fn invoke(
        &self,
        contract_id: &str,
        function: &str,
        function_args: Vec<String>,
    ) -> anyhow::Result<CommandOutput> {
        let mut args = vec!["contract".to_string(), "invoke".to_string()];
        args.extend(["--id".to_string(), contract_id.to_string()]);
        args.extend(self.network_args());
        args.push("--".to_string());
        args.push(function.to_string());
        args.extend(function_args);
        self.run(args, &[], self.source_env())
    }

    pub fn deploy(&self, wasm_hash: &str, constructor_args: Vec<String>) -> anyhow::Result<String> {
        let mut args = vec!["contract".to_string(), "deploy".to_string()];
        args.extend(["--wasm-hash".to_string(), wasm_hash.to_string()]);
        args.extend(self.network_args());
        if !constructor_args.is_empty() {
            args.push("--".to_string());
            args.extend(constructor_args);
        }
        let out = self.run(args, &[], self.source_env())?;
        if self.cli.dry_run {
            return Ok(format!("CDRYRUN{}", &wasm_hash[..8.min(wasm_hash.len())]));
        }
        parse_contract_id(&out.stdout)
    }

    pub fn upload(&self, wasm_path: &str) -> anyhow::Result<String> {
        let mut args = vec!["contract".to_string(), "upload".to_string()];
        args.extend(["--wasm".to_string(), wasm_path.to_string()]);
        args.extend(self.network_args());
        let out = self.run(args, &[], self.source_env())?;
        if self.cli.dry_run {
            return Ok(
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            );
        }
        parse_hash(&out.stdout)
    }

    pub fn fetch_wasm_hash(&self, wasm_hash: &str) -> anyhow::Result<bool> {
        let mut args = vec!["contract".to_string(), "fetch".to_string()];
        args.extend(["--wasm-hash".to_string(), wasm_hash.to_string()]);
        args.extend(self.network_args());
        Ok(self.run(args, &[], Vec::new()).is_ok())
    }

    pub fn deploy_native_asset(&self) -> anyhow::Result<()> {
        let mut args = vec![
            "contract".to_string(),
            "asset".to_string(),
            "deploy".to_string(),
            "--asset".to_string(),
            "native".to_string(),
        ];
        args.extend(self.network_args());
        let _ = self.run(args, &[], self.source_env())?;
        Ok(())
    }

    pub fn native_asset_id(&self) -> anyhow::Result<String> {
        let mut args = vec![
            "contract".to_string(),
            "id".to_string(),
            "asset".to_string(),
            "--asset".to_string(),
            "native".to_string(),
        ];
        args.extend(self.network_args());
        let out = self.run(args, &[], Vec::new())?;
        if self.cli.dry_run {
            return Ok("CDRYRUNNATIVEASSET".to_string());
        }
        parse_contract_id(&out.stdout)
    }

    pub fn build_package(
        &self,
        workspace_path: &str,
        package: &str,
        out_dir: &str,
    ) -> anyhow::Result<()> {
        let args = vec![
            "contract".to_string(),
            "build".to_string(),
            "--manifest-path".to_string(),
            format!("{workspace_path}/Cargo.toml"),
            "--package".to_string(),
            package.to_string(),
            "--optimize".to_string(),
            "--out-dir".to_string(),
            out_dir.to_string(),
        ];
        let _ = self.run(args, &[], Vec::new())?;
        Ok(())
    }
}

pub fn parse_contract_id(stdout: &str) -> anyhow::Result<String> {
    stdout
        .split_whitespace()
        .rev()
        .find(|token| token.starts_with('C') && token.len() >= 56)
        .map(ToString::to_string)
        .context("no contract id found in stellar output")
}

pub fn parse_hash(stdout: &str) -> anyhow::Result<String> {
    stdout
        .split_whitespace()
        .rev()
        .map(|token| token.trim_start_matches("0x"))
        .find(|token| token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_lowercase)
        .context("no wasm hash found in stellar output")
}

pub fn display_command(
    program: &str,
    args: &[String],
    redacted_args: &[usize],
    env: &[CommandEnv],
) -> String {
    let redacted_args = redacted_args.iter().copied().collect::<BTreeSet<_>>();
    env.iter()
        .map(|var| {
            let value = if var.redact {
                "<redacted>".to_string()
            } else {
                shell_escape(var.value.as_str())
            };
            format!("{}={value}", var.key)
        })
        .chain(std::iter::once(program.to_string()))
        .chain(args.iter().enumerate().map(|(index, arg)| {
            if redacted_args.contains(&index) {
                "<redacted>".to_string()
            } else {
                shell_escape(arg)
            }
        }))
        .collect::<Vec<_>>()
        .join(" ")
}

fn zeroize_redacted_args(args: &mut [String], redacted_args: &[usize]) {
    for index in redacted_args {
        if let Some(arg) = args.get_mut(*index) {
            arg.zeroize();
        }
    }
}

fn zeroize_env(env: &mut [CommandEnv]) {
    for var in env {
        if var.redact {
            var.value.zeroize();
        }
    }
}

fn shell_escape(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:=,@".contains(c))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn keys_address_source_account_args(
    source_account: Option<&SourceAccount>,
    stellar_account_env_is_set: bool,
) -> anyhow::Result<(Vec<String>, Vec<usize>)> {
    let mut args = vec!["keys".to_string(), "address".to_string()];
    let mut redacted_args = Vec::new();
    if let Some(source) = source_account {
        redacted_args.push(args.len());
        args.push(source.clone_secret());
    } else if stellar_account_env_is_set {
        anyhow::bail!(
            "cannot derive a public address from STELLAR_ACCOUNT without exposing it to child argv; pass --admin/--caller explicitly or use a Stellar keystore/default identity"
        );
    }
    Ok((args, redacted_args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_contract_id_from_noisy_output() {
        let id =
            parse_contract_id("logs\nCDY3B7IXFN5L4OY4UFFS2FA4MAQWJZLJD76LW37S7HFVWRS3RPQ2SIXX")
                .expect("parse id");
        assert_eq!(
            id,
            "CDY3B7IXFN5L4OY4UFFS2FA4MAQWJZLJD76LW37S7HFVWRS3RPQ2SIXX"
        );
    }

    #[test]
    fn parses_hash_from_output() {
        let hash = parse_hash(
            "installed 0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("parse hash");
        assert_eq!(
            hash,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn display_command_redacts_sensitive_arguments() {
        let args = vec![
            "contract".to_string(),
            "invoke".to_string(),
            "--source-account".to_string(),
            "SAUCE SECRET SEED".to_string(),
            "--network".to_string(),
            "testnet".to_string(),
        ];

        let display = display_command("stellar", &args, &[3], &[]);

        assert!(display.contains("--source-account <redacted>"));
        assert!(!display.contains("SAUCE SECRET SEED"));
    }

    #[test]
    fn display_command_redacts_sensitive_environment() {
        let display = display_command(
            "stellar",
            &["contract".to_string(), "invoke".to_string()],
            &[],
            &[CommandEnv::redacted(
                "STELLAR_ACCOUNT",
                "SAUCE SECRET SEED".to_string(),
            )],
        );

        assert!(display.contains("STELLAR_ACCOUNT=<redacted>"));
        assert!(!display.contains("SAUCE SECRET SEED"));
    }

    #[test]
    fn refuses_env_secret_for_source_address_derivation() {
        let err = keys_address_source_account_args(None, true)
            .expect_err("STELLAR_ACCOUNT should not be converted into argv");

        assert!(err
            .to_string()
            .contains("without exposing it to child argv"));
    }
}
