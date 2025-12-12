use std::{io::ErrorKind, process};

use console::{style, Term};

use crate::CliError;

pub fn handle_interrupted(err: &dialoguer::Error) {
    match err {
        dialoguer::Error::IO(io_err) if io_err.kind() == ErrorKind::Interrupted => {
            let _ = Term::stdout().show_cursor();
            println!("{}", style("\n Configuration aborted").red());
            process::exit(130);
        }
        dialoguer::Error::IO(_) => {}
    }
}

pub fn map_dialoguer_err(err: dialoguer::Error) -> CliError {
    handle_interrupted(&err);
    CliError::Io(std::io::Error::other(err))
}
