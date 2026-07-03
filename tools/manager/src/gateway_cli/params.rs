use std::io::Read as _;
use std::path::PathBuf;

use anyhow::Context as _;
use clap::ArgMatches;
use serde_json::Value;

pub(super) fn load_params(matches: &ArgMatches, rpc_method: &str) -> anyhow::Result<Value> {
    if let Some(json) = matches.get_one::<String>("json") {
        return serde_json::from_str(json).context("parse --json method parameters");
    }

    if let Some(path) = matches.get_one::<PathBuf>("json-file") {
        let mut input = String::new();
        if path == std::path::Path::new("-") {
            std::io::stdin()
                .read_to_string(&mut input)
                .context("read JSON parameters from stdin")?;
        } else {
            input = std::fs::read_to_string(path)
                .with_context(|| format!("read JSON parameters from {}", path.display()))?;
        }

        return serde_json::from_str(&input).context("parse JSON method parameters");
    }

    if let Some(params) = super::typed::load_typed_params(matches, rpc_method)? {
        return Ok(params);
    }

    anyhow::bail!("missing method parameters")
}
