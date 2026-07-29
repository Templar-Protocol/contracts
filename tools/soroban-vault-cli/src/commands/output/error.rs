//! Stable JSON success and error envelopes.

use serde::Serialize;

use crate::cli::Cli;

use super::Response;

pub fn print_error(cli: &Cli, error: &anyhow::Error) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string(&OutputEnvelope::error(cli, error))?
    );
    Ok(())
}

pub fn print_parse_error(raw_args: &[String], error: &clap::Error) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string(&ParseErrorEnvelope::new(raw_args, error))?
    );
    Ok(())
}

#[derive(Debug, Serialize)]
pub(in crate::commands) struct OutputEnvelope<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    ok: bool,
    network: &'a str,
    manifest: String,
    commands: Vec<String>,
    tx_hashes: Vec<String>,
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a Response>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorEnvelope>,
}

impl<'a> OutputEnvelope<'a> {
    pub(in crate::commands) fn success(cli: &'a Cli, response: &'a Response) -> Self {
        Self {
            kind: response.kind(),
            ok: true,
            network: &cli.network,
            manifest: cli.state.display().to_string(),
            commands: response.command_shapes(),
            tx_hashes: response.tx_hashes(),
            warnings: response.warnings(),
            data: Some(response),
            error: None,
        }
    }

    fn error(cli: &'a Cli, error: &anyhow::Error) -> Self {
        Self {
            kind: "error",
            ok: false,
            network: &cli.network,
            manifest: cli.state.display().to_string(),
            commands: Vec::new(),
            tx_hashes: Vec::new(),
            warnings: Vec::new(),
            data: None,
            error: Some(ErrorEnvelope {
                code: classify_error(error),
                message: error.to_string(),
            }),
        }
    }
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    code: &'static str,
    message: String,
}

fn classify_error(error: &anyhow::Error) -> &'static str {
    classify_error_message(&error.to_string())
}

fn classify_error_message(message: &str) -> &'static str {
    if message.contains("missing ") && message.contains(" contract id in manifest") {
        "missing_manifest_contract"
    } else if message.contains("mainnet write blocked") {
        "mainnet_guard"
    } else if message.contains("do not pass secret keys")
        || message.contains("without exposing it to child argv")
    {
        "secret_in_argv"
    } else {
        "command_failed"
    }
}

#[derive(Debug, Serialize)]
pub(in crate::commands) struct ParseErrorEnvelope {
    #[serde(rename = "type")]
    kind: &'static str,
    ok: bool,
    network: String,
    manifest: String,
    commands: Vec<String>,
    tx_hashes: Vec<String>,
    warnings: Vec<String>,
    error: ErrorEnvelope,
}

impl ParseErrorEnvelope {
    pub(in crate::commands) fn new(raw_args: &[String], error: &clap::Error) -> Self {
        let message = error.to_string();
        Self {
            kind: "error",
            ok: false,
            network: raw_arg_value(raw_args, "--network").unwrap_or_else(|| "testnet".to_string()),
            manifest: raw_arg_value(raw_args, "--state").unwrap_or_else(|| {
                "contract/vault/soroban/.deploy-state/manifest.json".to_string()
            }),
            commands: Vec::new(),
            tx_hashes: Vec::new(),
            warnings: Vec::new(),
            error: ErrorEnvelope {
                code: match classify_error_message(&message) {
                    "command_failed" => "invalid_args",
                    code => code,
                },
                message,
            },
        }
    }
}

fn raw_arg_value(raw_args: &[String], flag: &str) -> Option<String> {
    raw_args.iter().enumerate().find_map(|(index, arg)| {
        if arg == flag {
            return raw_args.get(index + 1).cloned();
        }
        arg.strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
            .map(ToString::to_string)
    })
}
