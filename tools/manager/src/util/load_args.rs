use std::path::PathBuf;

use anyhow::Context;
use serde::{de::DeserializeOwned, Serialize};

use super::load_text;

pub trait LoadArgs<T: DeserializeOwned + Serialize>: clap::Args {
    fn load(&self) -> anyhow::Result<T>;

    fn load_vec(&self) -> anyhow::Result<Vec<u8>> {
        let parsed = self.load().context("args deserialization")?;
        serde_json::to_vec(&parsed).context("args serialization")
    }
}

#[derive(clap::Args, Debug)]
#[group(multiple = false, required = true)]
pub struct GeneralArgsLoader {
    /// JSON string
    #[arg(long)]
    pub args: Option<String>,
    /// JSON file path
    #[arg(long)]
    pub args_file: Option<PathBuf>,
}

impl GeneralArgsLoader {
    pub fn from_json_string(args: String) -> Self {
        Self {
            args: Some(args),
            args_file: None,
        }
    }

    pub fn from_file(args_file: PathBuf) -> Self {
        Self {
            args: None,
            args_file: Some(args_file),
        }
    }
}

impl<T: DeserializeOwned + Serialize> LoadArgs<T> for GeneralArgsLoader {
    fn load(&self) -> anyhow::Result<T> {
        serde_json::from_str(&load_text(
            self.args.as_deref(),
            self.args_file.as_deref(),
            "args",
        )?)
        .context("deserialize args json")
    }
}

#[derive(clap::Args, Default)]
pub struct EmptyArgsLoader {}

impl LoadArgs<()> for EmptyArgsLoader {
    fn load(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
