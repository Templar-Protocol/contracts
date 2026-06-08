use std::{ffi::OsString, process::Command};

use anyhow::Context;

use crate::cli::Cli;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}

pub trait CommandExecutor {
    fn run(&self, program: &str, args: &[String]) -> anyhow::Result<CommandOutput>;
}

pub struct RealExecutor;

impl CommandExecutor for RealExecutor {
    fn run(&self, program: &str, args: &[String]) -> anyhow::Result<CommandOutput> {
        let output = Command::new(program)
            .args(args.iter().map(OsString::from))
            .output()
            .with_context(|| format!("run {}", display_command(program, args)))?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::ensure!(
            output.status.success(),
            "command failed: {}\n{}",
            display_command(program, args),
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

    pub fn run(&self, args: &[String]) -> anyhow::Result<CommandOutput> {
        if self.cli.dry_run {
            println!("dry-run: {}", display_command("stellar", args));
            Ok(CommandOutput {
                stdout: String::new(),
                stderr: String::new(),
            })
        } else {
            self.executor.run("stellar", args)
        }
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

    pub fn source_args(&self) -> Vec<String> {
        vec![
            "--source-account".to_string(),
            self.cli.source_account.clone(),
        ]
    }

    pub fn keys_address(&self, identity: &str) -> anyhow::Result<String> {
        let args = vec![
            "keys".to_string(),
            "address".to_string(),
            identity.to_string(),
        ];
        let out = self.run(&args)?;
        if self.cli.dry_run {
            return Ok(format!("GDRYRUN{identity}"));
        }
        anyhow::ensure!(
            !out.stdout.is_empty(),
            "stellar keys address returned no address"
        );
        Ok(out.stdout)
    }

    pub fn keys_address_source_account(&self) -> anyhow::Result<String> {
        self.keys_address(&self.cli.source_account)
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
        args.extend(self.source_args());
        args.push("--".to_string());
        args.push(function.to_string());
        args.extend(function_args);
        self.run(&args)
    }

    pub fn deploy(&self, wasm_hash: &str, constructor_args: Vec<String>) -> anyhow::Result<String> {
        let mut args = vec!["contract".to_string(), "deploy".to_string()];
        args.extend(["--wasm-hash".to_string(), wasm_hash.to_string()]);
        args.extend(self.network_args());
        args.extend(self.source_args());
        if !constructor_args.is_empty() {
            args.push("--".to_string());
            args.extend(constructor_args);
        }
        let out = self.run(&args)?;
        if self.cli.dry_run {
            return Ok(format!("CDRYRUN{}", &wasm_hash[..8.min(wasm_hash.len())]));
        }
        parse_contract_id(&out.stdout)
    }

    pub fn upload(&self, wasm_path: &str) -> anyhow::Result<String> {
        let mut args = vec!["contract".to_string(), "upload".to_string()];
        args.extend(["--wasm".to_string(), wasm_path.to_string()]);
        args.extend(self.network_args());
        args.extend(self.source_args());
        let out = self.run(&args)?;
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
        Ok(self.run(&args).is_ok())
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
        args.extend(self.source_args());
        let _ = self.run(&args)?;
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
        let out = self.run(&args)?;
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
        let _ = self.run(&args)?;
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

pub fn display_command(program: &str, args: &[String]) -> String {
    std::iter::once(program.to_string())
        .chain(args.iter().map(|arg| shell_escape(arg)))
        .collect::<Vec<_>>()
        .join(" ")
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
}
