use std::path::PathBuf;

use anyhow::Context;
use serde::{de::DeserializeOwned, Serialize};

pub trait ArgsProvider<T: DeserializeOwned + Serialize>: clap::Args {
    fn parse(&self) -> anyhow::Result<T>;

    fn load_vec(&self) -> anyhow::Result<Vec<u8>> {
        let parsed = self.parse().context("args deserialization")?;
        serde_json::to_vec(&parsed).context("args serialization")
    }
}

#[derive(clap::Args, Debug)]
#[group(multiple = false, required = true)]
pub struct StandardArgsProvider {
    /// JSON string
    #[arg(long)]
    pub args: Option<String>,
    /// JSON file path
    #[arg(long)]
    pub args_file: Option<PathBuf>,
}

impl<T: DeserializeOwned + Serialize> ArgsProvider<T> for StandardArgsProvider {
    fn parse(&self) -> anyhow::Result<T> {
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

#[derive(clap::Args)]
pub struct EmptyArgsProvider {}

impl ArgsProvider<()> for EmptyArgsProvider {
    fn parse(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
