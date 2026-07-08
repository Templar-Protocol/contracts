use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Args, Subcommand, ValueEnum};
use near_account_id::AccountId;
use serde_json::{json, Value};
use templar_common::oracle::redstone::{config, FeedId};
use templar_gateway_methods_spec::{redstone as spec, registry as registry_spec};
use templar_gateway_types::{Base64Bytes, NearToken};

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum RedstoneNs {
    /// Deploy a RedStone adapter from a registry (with `--prod`/`--test` config
    /// presets).
    Create(Create),
    GetConfig(GetConfig),
    ReadPriceData(ReadPriceData),
    ListRole(ListRole),
    SetRole(SetRole),
    WritePrices(WritePrices),
    /// Fetch signed prices from the RedStone bridge and write them on-chain.
    UpdatePrices(UpdatePrices),
}

#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("redstone_config")
        .args(["prod", "test", "init_args", "init_args_file"])
        .required(true)
        .multiple(false)
))]
pub struct Create {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long, value_name = "NAME")]
    name: String,
    #[arg(long, value_name = "KEY")]
    version_key: String,
    /// Use the built-in production RedStone configuration.
    #[arg(long)]
    prod: bool,
    /// Use the built-in test RedStone configuration.
    #[arg(long)]
    test: bool,
    /// Full init args JSON (`{"config": ...}`).
    #[arg(long, value_name = "JSON")]
    init_args: Option<String>,
    /// Path to a full init args JSON file.
    #[arg(long, value_name = "PATH")]
    init_args_file: Option<PathBuf>,
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

impl Create {
    pub fn parse(self) -> anyhow::Result<registry_spec::Deploy> {
        let init_bytes = if self.prod {
            serde_json::to_vec(&json!({ "config": config::prod() }))
                .context("serialize prod config")?
        } else if self.test {
            serde_json::to_vec(&json!({ "config": config::test() }))
                .context("serialize test config")?
        } else if let Some(args) = self.init_args {
            args.into_bytes()
        } else if let Some(path) = self.init_args_file {
            std::fs::read(&path)
                .with_context(|| format!("read RedStone init args from {}", path.display()))?
        } else {
            anyhow::bail!("provide --prod, --test, --init-args, or --init-args-file");
        };

        Ok(registry_spec::Deploy {
            registry_id: self.registry_id,
            name: self.name,
            version_key: self.version_key,
            init_args: Base64Bytes(init_bytes),
            full_access_keys: None,
            deposit: self.deposit,
        })
    }
}

#[derive(Clone, Debug, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum RoleArg {
    ModifyRoles,
    TrustedUpdater,
}

impl From<RoleArg> for spec::RoleValue {
    fn from(arg: RoleArg) -> Self {
        match arg {
            RoleArg::ModifyRoles => Self::ModifyRoles,
            RoleArg::TrustedUpdater => Self::TrustedUpdater,
        }
    }
}

#[derive(Args, Debug)]
pub struct GetConfig {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
}

impl GetConfig {
    pub fn parse(self) -> spec::GetConfig {
        spec::GetConfig {
            oracle_id: self.oracle_id,
        }
    }
}

#[derive(Args, Debug)]
pub struct ReadPriceData {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long = "feed-id", value_name = "FEED_ID", required = true)]
    feed_ids: Vec<FeedId>,
}

impl ReadPriceData {
    pub fn parse(self) -> spec::ReadPriceData {
        spec::ReadPriceData {
            oracle_id: self.oracle_id,
            feed_ids: self.feed_ids,
        }
    }
}

#[derive(Args, Debug)]
pub struct ListRole {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long, value_enum)]
    role: RoleArg,
}

impl ListRole {
    pub fn parse(self) -> spec::ListRole {
        spec::ListRole {
            oracle_id: self.oracle_id,
            role: self.role.into(),
        }
    }
}

#[derive(Args, Debug)]
pub struct SetRole {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
    #[arg(long, value_enum)]
    role: RoleArg,
    /// Revoke the role instead of granting it.
    #[arg(long)]
    revoke: bool,
}

impl SetRole {
    pub fn parse(self) -> spec::SetRole {
        spec::SetRole {
            oracle_id: self.oracle_id,
            account_id: self.account_id,
            role: self.role.into(),
            set: !self.revoke,
        }
    }
}

#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("payload").args(["payload_base64", "payload_base64_file"]).required(true)
))]
pub struct WritePrices {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long = "feed-id", value_name = "FEED_ID", required = true)]
    feed_ids: Vec<FeedId>,
    /// Base64-encoded RedStone payload.
    #[arg(long, value_name = "BASE64")]
    payload_base64: Option<String>,
    /// Path to a file containing a base64-encoded RedStone payload.
    #[arg(long, value_name = "PATH")]
    payload_base64_file: Option<PathBuf>,
}

impl WritePrices {
    pub fn parse(self) -> anyhow::Result<spec::WritePrices> {
        let payload_base64 = match (self.payload_base64, self.payload_base64_file) {
            (Some(inline), _) => inline,
            (None, Some(path)) => std::fs::read_to_string(&path)
                .with_context(|| format!("read RedStone payload from {}", path.display()))?,
            (None, None) => {
                anyhow::bail!("provide --payload-base64 or --payload-base64-file")
            }
        };

        // Decode via `Base64Bytes`' own base64 deserialization to avoid a bespoke decoder.
        let payload: Base64Bytes =
            serde_json::from_value(Value::String(payload_base64.trim().to_owned()))
                .context("invalid base64 RedStone payload")?;

        Ok(spec::WritePrices {
            oracle_id: self.oracle_id,
            feed_ids: self.feed_ids,
            payload,
        })
    }
}

#[derive(Args, Debug)]
pub struct UpdatePrices {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Feed IDs to fetch and update (e.g. BTC, ETH, NEAR).
    #[arg(long = "feed-id", value_name = "FEED_ID", required = true)]
    feed_ids: Vec<FeedId>,
    /// Path to the Node.js binary that runs the RedStone bridge.
    #[arg(
        long,
        env = "REDSTONE_NODE_PATH",
        default_value = "node",
        value_name = "PATH"
    )]
    node_path: PathBuf,
}

impl UpdatePrices {
    pub fn feed_ids(&self) -> &[FeedId] {
        &self.feed_ids
    }

    pub fn node_path(&self) -> &std::path::Path {
        &self.node_path
    }

    /// Build the on-chain write spec from a bridge-fetched payload.
    pub fn write_spec(&self, payload: Vec<u8>) -> spec::WritePrices {
        spec::WritePrices {
            oracle_id: self.oracle_id.clone(),
            feed_ids: self.feed_ids.clone(),
            payload: Base64Bytes(payload),
        }
    }
}
