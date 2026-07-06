use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Args, Subcommand, ValueEnum};
use near_account_id::AccountId;
use serde_json::Value;
use templar_common::oracle::redstone::FeedId;
use templar_gateway_methods_spec::redstone as spec;
use templar_gateway_types::Base64Bytes;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum RedstoneNs {
    GetConfig(GetConfig),
    ReadPriceData(ReadPriceData),
    ListRole(ListRole),
    SetRole(SetRole),
    WritePrices(WritePrices),
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
