use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Args;
use serde::de::DeserializeOwned;

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

#[derive(Args, Debug, Clone)]
#[group(required = true, multiple = false)]
pub struct InitArgsSource {
    /// JSON-encoded value
    #[arg(long)]
    pub init_args: Option<String>,
    /// Path to a file containing JSON-encoded value
    #[arg(long)]
    pub init_args_file: Option<PathBuf>,
}

impl InitArgsSource {
    pub fn parse(&self) -> anyhow::Result<serde_json::Value> {
        JsonSource::new(self.init_args.as_deref(), self.init_args_file.as_deref())?.parse()
    }
}

#[derive(Args, Debug, Clone)]
#[group(required = true, multiple = false)]
pub struct ConfigurationSource {
    /// JSON configuration
    #[arg(long)]
    pub configuration: Option<String>,
    /// Path to a JSON configuration file
    #[arg(long)]
    pub configuration_file: Option<PathBuf>,
}

impl ConfigurationSource {
    pub fn parse<T: DeserializeOwned>(&self) -> anyhow::Result<T> {
        JsonSource::new(
            self.configuration.as_deref(),
            self.configuration_file.as_deref(),
        )?
        .parse()
    }
}
