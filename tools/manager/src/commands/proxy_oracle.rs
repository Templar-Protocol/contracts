use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Args, Subcommand, ValueEnum};
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_methods_spec::proxy_oracle as proxy_spec;
use templar_gateway_methods_spec::proxy_oracle_governance as governance_spec;
use templar_gateway_methods_spec::proxy_oracle_owner as owner_spec;
use templar_proxy_oracle_near_governance_common::Operation;

use super::super::proxy::load_proxy_file;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProxyOracleOwnerNs {
    ProposeOwner(ProposeOwner),
    AcceptOwner(AcceptOwner),
}

#[derive(Args, Debug)]
pub struct ProposeOwner {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: Option<AccountId>,
}

impl ProposeOwner {
    pub fn parse(self) -> owner_spec::ProposeOwner {
        owner_spec::ProposeOwner {
            oracle_id: self.oracle_id,
            account_id: self.account_id,
        }
    }
}

#[derive(Args, Debug)]
pub struct AcceptOwner {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
}

impl AcceptOwner {
    pub fn parse(self) -> owner_spec::AcceptOwner {
        owner_spec::AcceptOwner {
            oracle_id: self.oracle_id,
        }
    }
}

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProxyOracleGovernanceNs {
    CreateProposal(CreateProposal),
    CancelProposal(CancelProposal),
    ExecuteProposal(ExecuteProposal),
    GetProposal(GetProposal),
    ListProposals(ListProposals),
}

#[derive(Clone, Debug, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum OperationArg {
    SetProxy,
}

#[derive(Args, Debug)]
pub struct CreateProposal {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long, value_name = "ID")]
    id: u32,
    #[arg(long, value_enum)]
    operation: OperationArg,
    #[arg(long, value_name = "HEX")]
    price_id: Option<String>,
    #[arg(long, value_name = "PATH")]
    proxy_file: Option<PathBuf>,
    #[arg(long, value_name = "NANOSECONDS", default_value = "0")]
    requested_ttl: String,
}

impl CreateProposal {
    pub fn parse(self) -> anyhow::Result<governance_spec::CreateProposal> {
        let operation = match self.operation {
            OperationArg::SetProxy => {
                let price_id_hex = self.price_id.ok_or_else(|| {
                    anyhow::anyhow!("--price-id is required for --operation set-proxy")
                })?;
                let price_id = parse_price_identifier(&price_id_hex)?;
                let proxy_file = self.proxy_file.ok_or_else(|| {
                    anyhow::anyhow!("--proxy-file is required for --operation set-proxy")
                })?;
                let proxy_value = load_proxy_file(&proxy_file)?;
                let proxy: Option<
                    templar_proxy_oracle_kernel::proxy::Proxy<
                        templar_proxy_oracle_near_common::input::Source,
                    >,
                > = serde_json::from_value(proxy_value).context("parse proxy configuration")?;
                Operation::SetProxy {
                    id: price_id,
                    proxy,
                }
            }
        };

        let requested_ttl = self
            .requested_ttl
            .parse::<u64>()
            .map(templar_common::Nanoseconds::from_ns)
            .context("parse requested_ttl as nanoseconds")?;

        Ok(governance_spec::CreateProposal {
            governance_id: self.governance_id,
            id: self.id,
            operation,
            requested_ttl,
        })
    }
}

#[derive(Args, Debug)]
pub struct ExecuteProposal {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long, value_name = "ID")]
    id: u32,
}

impl ExecuteProposal {
    pub fn parse(self) -> governance_spec::ExecuteProposal {
        governance_spec::ExecuteProposal {
            governance_id: self.governance_id,
            id: self.id,
        }
    }
}

#[derive(Args, Debug)]
pub struct CancelProposal {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long, value_name = "ID")]
    id: u32,
}

impl CancelProposal {
    pub fn parse(self) -> governance_spec::CancelProposal {
        governance_spec::CancelProposal {
            governance_id: self.governance_id,
            id: self.id,
        }
    }
}

#[derive(Args, Debug)]
pub struct GetProposal {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long, value_name = "ID")]
    id: u32,
}

impl GetProposal {
    pub fn parse(self) -> governance_spec::GetProposal {
        governance_spec::GetProposal {
            governance_id: self.governance_id,
            id: self.id,
        }
    }
}

#[derive(Args, Debug)]
pub struct ListProposals {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    count: Option<u32>,
}

impl ListProposals {
    pub fn parse(self) -> governance_spec::ListProposals {
        governance_spec::ListProposals {
            governance_id: self.governance_id,
            offset: self.offset,
            count: self.count,
        }
    }
}

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProxyOracleNs {
    GetProxy(GetProxy),
    ListProxies(ListProxies),
}

#[derive(Args, Debug)]
pub struct GetProxy {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long, value_name = "HEX")]
    price_id: String,
}

impl GetProxy {
    pub fn parse(self) -> anyhow::Result<proxy_spec::GetProxy> {
        Ok(proxy_spec::GetProxy {
            oracle_id: self.oracle_id,
            id: parse_price_identifier(&self.price_id)?,
        })
    }
}

#[derive(Args, Debug)]
pub struct ListProxies {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    count: Option<u32>,
}

impl ListProxies {
    pub fn parse(self) -> proxy_spec::ListProxies {
        proxy_spec::ListProxies {
            oracle_id: self.oracle_id,
            offset: self.offset,
            count: self.count,
        }
    }
}

fn parse_price_identifier(hex: &str) -> anyhow::Result<PriceIdentifier> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    let bytes = hex::decode(hex).context("decode hex price identifier")?;
    if bytes.len() != 32 {
        anyhow::bail!("price identifier must be 32 bytes, got {}", bytes.len());
    }
    let mut id = [0u8; 32];
    id.copy_from_slice(&bytes);
    Ok(PriceIdentifier(id))
}
