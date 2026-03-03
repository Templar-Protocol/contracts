use std::io::ErrorKind;

use console::{style, Term};

use crate::CliError;

pub fn handle_interrupted(err: &dialoguer::Error) -> bool {
    match err {
        dialoguer::Error::IO(io_err) if io_err.kind() == ErrorKind::Interrupted => {
            let _ = Term::stdout().show_cursor();
            println!("{}", style("\n Configuration aborted by user").yellow());
            true
        }
        dialoguer::Error::IO(_) => false,
    }
}

pub fn map_dialoguer_err(err: &dialoguer::Error) -> CliError {
    if handle_interrupted(err) {
        return CliError::Interrupted;
    }
    CliError::Prompt(err.to_string())
}
