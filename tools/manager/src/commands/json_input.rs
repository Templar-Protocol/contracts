use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Args;
use serde::{de::DeserializeOwned, Serialize};

pub enum JsonSource<'a> {
    String(&'a str),
    File(&'a Path),
}

impl<'a> JsonSource<'a> {
    pub fn new(s: Option<&'a str>, f: Option<&'a Path>) -> anyhow::Result<Self> {
        match (s, f) {
            (Some(_), Some(_)) => anyhow::bail!("cannot specify both string and file"),
            (Some(s), _) => Ok(Self::String(s)),
            (_, Some(f)) => Ok(Self::File(f)),
            _ => anyhow::bail!("one of string or file must be provided"),
        }
    }

    pub fn parse<T: DeserializeOwned>(&self) -> anyhow::Result<T> {
        match self {
            Self::String(s) => Ok(serde_json::from_str(s)?),
            Self::File(path) => {
                let file = std::fs::File::open(path)
                    .with_context(|| format!("read json file `{}`", path.display()))?;
                Ok(serde_json::from_reader(file)?)
            }
        }
    }
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct ArgsSource {
    /// JSON-encoded value
    #[arg(long)]
    pub args: Option<String>,
    /// Path to a file containing JSON-encoded value
    #[arg(long)]
    pub args_file: Option<PathBuf>,
}

impl ArgsSource {
    pub fn parse<T: DeserializeOwned>(&self) -> anyhow::Result<T> {
        JsonSource::new(self.args.as_deref(), self.args_file.as_deref())?.parse()
    }

    pub fn load_vec<T: DeserializeOwned + Serialize>(&self) -> anyhow::Result<Vec<u8>> {
        let parsed = self.parse::<T>().context("args deserialization")?;
        serde_json::to_vec(&parsed).context("args serialization")
    }
}
