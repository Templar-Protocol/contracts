use std::path::PathBuf;

use anyhow::Context;
use serde::{de::DeserializeOwned, Serialize};

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
        match (&self.args, &self.args_file) {
            (Some(json), None) => serde_json::from_str(json).context("deserialize json string"),
            (None, Some(file)) => {
                serde_json::from_reader(std::fs::File::open(file).context("open json file")?)
                    .context("deserialize json file")
            }
            _ => anyhow::bail!("one of --args or --args-file must be provided"),
        }
    }
}

#[derive(clap::Args, Default)]
pub struct EmptyArgsLoader {}

impl LoadArgs<()> for EmptyArgsLoader {
    fn load(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
