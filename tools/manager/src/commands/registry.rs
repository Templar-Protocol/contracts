use anyhow::Context as _;
use clap::{Args, Subcommand};
use near_account_id::AccountId;
use templar_gateway_methods_spec::registry as spec;
use templar_gateway_types::{common::Pagination, Base64Bytes, ContractKind, NearToken};

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum RegistryNs {
    ListVersions(ListVersions),
    ListDeployments(ListDeployments),
    ListDeploymentsByKind(ListDeploymentsByKind),
    GetDeployment(GetDeployment),
    Deploy(Deploy),
}

#[derive(Args, Debug)]
pub struct ListVersions {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    limit: Option<u32>,
}

impl ListVersions {
    pub fn parse(self) -> spec::ListVersions {
        spec::ListVersions {
            registry_id: self.registry_id,
            args: Pagination {
                offset: self.offset,
                limit: self.limit,
            },
        }
    }
}

#[derive(Args, Debug)]
pub struct ListDeployments {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    limit: Option<u32>,
}

impl ListDeployments {
    pub fn parse(self) -> spec::ListDeployments {
        spec::ListDeployments {
            registry_id: self.registry_id,
            args: Pagination {
                offset: self.offset,
                limit: self.limit,
            },
        }
    }
}

#[derive(Args, Debug)]
pub struct ListDeploymentsByKind {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long, value_enum)]
    kind: ContractKind,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    limit: Option<u32>,
}

impl ListDeploymentsByKind {
    pub fn parse(self) -> spec::ListDeploymentsByKind {
        spec::ListDeploymentsByKind {
            registry_id: self.registry_id,
            args: Pagination {
                offset: self.offset,
                limit: self.limit,
            },
            kind: self.kind,
        }
    }
}

#[derive(Args, Debug)]
pub struct GetDeployment {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
}

impl GetDeployment {
    pub fn parse(self) -> spec::GetDeployment {
        spec::GetDeployment {
            registry_id: self.registry_id,
            account_id: self.account_id,
        }
    }
}

#[derive(Args, Debug)]
pub struct Deploy {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long, value_name = "NAME")]
    name: String,
    #[arg(long, value_name = "KEY")]
    version_key: String,
    #[arg(long, value_name = "PATH")]
    init_args_file: Option<std::path::PathBuf>,
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

impl Deploy {
    pub fn parse(self) -> anyhow::Result<spec::Deploy> {
        let init_bytes = match self.init_args_file {
            Some(path) => std::fs::read(&path)
                .map_err(anyhow::Error::from)
                .with_context(|| format!("read init args from {}", path.display()))?,
            None => b"null".to_vec(),
        };

        Ok(spec::Deploy {
            registry_id: self.registry_id,
            name: self.name,
            version_key: self.version_key,
            init_args: Base64Bytes(init_bytes),
            full_access_keys: None,
            deposit: self.deposit,
        })
    }
}
