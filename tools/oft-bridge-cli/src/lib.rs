pub mod artifacts;
pub mod canary;
pub mod cli;
pub mod codec;
pub mod domain;
pub mod environment;
pub mod error;
pub mod evm;
pub mod governance;
pub mod layerzero;
pub mod output;
pub mod process;
pub mod reconcile;
pub mod state;
pub mod stellar;

use std::process::ExitCode;

use clap::Parser as _;

use crate::{cli::Cli, error::Error};

pub fn main_entry() -> ExitCode {
    let cli = Cli::parse();
    let command = cli.command_name();
    match cli.run() {
        Ok(data) => match output::success(&command, data) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => error.exit_code(),
        },
        Err(error) => {
            if let Err(output_error) = output::failure(&command, &error) {
                eprintln!("failed to emit error envelope: {output_error}");
            }
            error.exit_code()
        }
    }
}

pub fn canonical_sha256<T: serde::Serialize>(value: &T) -> error::Result<String> {
    use sha2::{Digest as _, Sha256};
    let bytes = serde_json_canonicalizer::to_vec(value)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub fn policy_error(message: impl Into<String>) -> Error {
    Error::Policy(message.into())
}
