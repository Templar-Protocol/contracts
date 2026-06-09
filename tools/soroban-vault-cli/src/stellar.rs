use std::{
    collections::BTreeSet,
    fmt::Write as _,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use anyhow::Context;
use serde_json::Value;
use zeroize::Zeroize;
use zeroize::Zeroizing;

use crate::cli::Cli;
use crate::types::SourceAccount;

const SUBMITTED_TX_CONFIRMATION_TIMEOUT_SECONDS: u64 = 300;
const SUBMITTED_TX_CONFIRMATION_POLL_SECONDS: u64 = 5;

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
            "command failed: {}\nstdout: {}\nstderr: {}",
            display_command(program, args, redacted_args, env),
            stdout,
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
        let confirm_transaction = should_confirm_transaction(&args);
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
        let result = if confirm_transaction && !self.cli.dry_run {
            self.confirm_transaction_result(result)
        } else {
            result
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
        let mut args = vec![
            "contract".to_string(),
            "build".to_string(),
            "--manifest-path".to_string(),
            format!("{workspace_path}/Cargo.toml"),
            "--package".to_string(),
            package.to_string(),
            "--optimize".to_string(),
        ];
        if let Some(source_repo) = self
            .cli
            .contract_source_repo
            .as_deref()
            .map(str::trim)
            .filter(|source_repo| !source_repo.is_empty())
        {
            args.extend(["--meta".to_string(), format!("source_repo={source_repo}")]);
        }
        args.extend(["--out-dir".to_string(), out_dir.to_string()]);
        let _ = self.run(args, &[], Vec::new())?;
        Ok(())
    }

    fn confirm_transaction_result(
        &self,
        result: anyhow::Result<CommandOutput>,
    ) -> anyhow::Result<CommandOutput> {
        match result {
            Ok(mut output) => {
                if let Some(hash) = first_tx_hash(&output.stdout, &output.stderr) {
                    self.wait_for_transaction_success(&hash)?;
                    append_reconciled_tx_hash(&mut output, &hash);
                }
                Ok(output)
            }
            Err(error) => {
                let message = error.to_string();
                let Some(hash) = first_tx_hash(&message, "") else {
                    return Err(error);
                };
                match self.wait_for_transaction_success(&hash) {
                    Ok(()) => Ok(CommandOutput {
                        stdout: format!("tx hash: {hash}"),
                        stderr: format!(
                            "stellar command returned an error, but RPC confirmed transaction success: {error}"
                        ),
                    }),
                    Err(wait_error) => Err(error).with_context(|| {
                        format!("could not confirm submitted transaction {hash}: {wait_error}")
                    }),
                }
            }
        }
    }

    fn wait_for_transaction_success(&self, tx_hash: &str) -> anyhow::Result<()> {
        let started = Instant::now();
        let timeout = Duration::from_secs(SUBMITTED_TX_CONFIRMATION_TIMEOUT_SECONDS);
        let poll = Duration::from_secs(SUBMITTED_TX_CONFIRMATION_POLL_SECONDS);
        let mut last_status = "not_found".to_string();
        let mut last_error = None;

        while started.elapsed() < timeout {
            match self.fetch_transaction_status(tx_hash) {
                Ok(TransactionConfirmationStatus::Success) => return Ok(()),
                Ok(TransactionConfirmationStatus::Failed) => {
                    anyhow::bail!("transaction {tx_hash} failed after submission")
                }
                Ok(TransactionConfirmationStatus::NotFound) => {
                    last_status = "not_found".to_string();
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                }
            }
            thread::sleep(poll);
        }

        if let Some(error) = last_error {
            anyhow::bail!(
                "transaction {tx_hash} was not confirmed before timeout; last status: {last_status}; last error: {error}"
            );
        }
        anyhow::bail!(
            "transaction {tx_hash} was not confirmed before timeout; last status: {last_status}"
        )
    }

    fn fetch_transaction_status(
        &self,
        tx_hash: &str,
    ) -> anyhow::Result<TransactionConfirmationStatus> {
        let mut args = vec![
            "tx".to_string(),
            "fetch".to_string(),
            "--hash".to_string(),
            tx_hash.to_string(),
            "--output".to_string(),
            "json".to_string(),
        ];
        args.extend(self.network_args());
        match self.executor.run("stellar", &args, &[], &[]) {
            Ok(output) => Ok(transaction_status_from_output(&output.stdout)
                .unwrap_or(TransactionConfirmationStatus::Success)),
            Err(error) => {
                let message = error.to_string();
                if looks_not_found(&message) {
                    Ok(TransactionConfirmationStatus::NotFound)
                } else {
                    Err(error).with_context(|| format!("fetch transaction {tx_hash}"))
                }
            }
        }
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

fn append_reconciled_tx_hash(output: &mut CommandOutput, tx_hash: &str) {
    if parse_tx_hashes(&output.stdout)
        .into_iter()
        .chain(parse_tx_hashes(&output.stderr))
        .any(|hash| hash == tx_hash)
    {
        return;
    }
    if output.stdout.is_empty() {
        output.stdout = format!("tx hash: {tx_hash}");
    } else {
        let _ = write!(output.stdout, "\ntx hash: {tx_hash}");
    }
}

fn should_confirm_transaction(args: &[String]) -> bool {
    match args {
        [first, second, ..] if first == "tx" && second == "send" => true,
        [first, second, ..]
            if first == "contract" && matches!(second.as_str(), "deploy" | "invoke" | "upload") =>
        {
            true
        }
        [first, second, third, ..]
            if first == "contract" && second == "asset" && third == "deploy" =>
        {
            true
        }
        _ => false,
    }
}

fn first_tx_hash(stdout: &str, stderr: &str) -> Option<String> {
    parse_tx_hashes(stdout)
        .into_iter()
        .chain(parse_tx_hashes(stderr))
        .next()
}

fn parse_tx_hashes(value: &str) -> Vec<String> {
    value
        .split(|c: char| !c.is_ascii_hexdigit())
        .filter(|token| token.len() == 64)
        .map(str::to_ascii_lowercase)
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransactionConfirmationStatus {
    Success,
    Failed,
    NotFound,
}

fn transaction_status_from_output(output: &str) -> Option<TransactionConfirmationStatus> {
    serde_json::from_str::<Value>(output)
        .ok()
        .and_then(|value| find_transaction_status(&value))
        .or_else(|| transaction_status_from_text(output))
}

fn find_transaction_status(value: &Value) -> Option<TransactionConfirmationStatus> {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if key.eq_ignore_ascii_case("status") {
                    if let Some(status) = value.as_str().and_then(transaction_status_from_text) {
                        return Some(status);
                    }
                }
                if let Some(status) = find_transaction_status(value) {
                    return Some(status);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(find_transaction_status),
        Value::String(text) => transaction_status_from_text(text),
        _ => None,
    }
}

fn transaction_status_from_text(text: &str) -> Option<TransactionConfirmationStatus> {
    let normalized = text.to_ascii_uppercase();
    if normalized.contains("SUCCESS") {
        Some(TransactionConfirmationStatus::Success)
    } else if normalized.contains("FAILED") || normalized.contains("ERROR") {
        Some(TransactionConfirmationStatus::Failed)
    } else if normalized.contains("NOT_FOUND") || normalized.contains("NOT FOUND") {
        Some(TransactionConfirmationStatus::NotFound)
    } else {
        None
    }
}

fn looks_not_found(message: &str) -> bool {
    transaction_status_from_text(message) == Some(TransactionConfirmationStatus::NotFound)
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

    #[test]
    fn detects_write_commands_that_can_emit_transaction_hashes() {
        assert!(should_confirm_transaction(&[
            "contract".to_string(),
            "invoke".to_string()
        ]));
        assert!(should_confirm_transaction(&[
            "contract".to_string(),
            "deploy".to_string()
        ]));
        assert!(should_confirm_transaction(&[
            "contract".to_string(),
            "asset".to_string(),
            "deploy".to_string()
        ]));
        assert!(should_confirm_transaction(&[
            "tx".to_string(),
            "send".to_string()
        ]));
        assert!(!should_confirm_transaction(&[
            "contract".to_string(),
            "fetch".to_string()
        ]));
    }

    #[test]
    fn parses_transaction_hashes_from_command_text() {
        let hashes = parse_tx_hashes(
            "tx hash: 0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef",
        );

        assert_eq!(
            hashes,
            vec!["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]
        );
    }

    #[test]
    fn parses_transaction_status_from_json() {
        let status = transaction_status_from_output(r#"{"status":"SUCCESS"}"#);
        assert_eq!(status, Some(TransactionConfirmationStatus::Success));

        let status = transaction_status_from_output(r#"{"result":{"status":"FAILED"}}"#);
        assert_eq!(status, Some(TransactionConfirmationStatus::Failed));
    }

    #[test]
    fn appends_reconciled_tx_hash_when_send_output_has_no_hash() {
        let mut output = CommandOutput {
            stdout: "submitted".to_string(),
            stderr: String::new(),
        };

        append_reconciled_tx_hash(
            &mut output,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        );

        assert!(output
            .stdout
            .contains("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"));
    }
}
