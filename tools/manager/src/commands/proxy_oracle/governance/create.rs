use std::path::PathBuf;

use near_fetch::ops::Function;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use near_sdk::NearToken;
use templar_common::governance::Proposal;
use templar_common::oracle::proxy::governance::Operation;
use templar_common::oracle::proxy::Proxy;
use templar_common::time::Nanoseconds;

use super::execute::execute_proposal;
use crate::commands::proxy_oracle::proxy::CliPriceIdentifier;
use crate::util::{load_text, SignerArgs};
use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct CreateProposal {
    #[command(flatten)]
    pub signer: SignerArgs,
    #[arg(long)]
    pub oracle_id: AccountId,
    /// Proposal ID (auto-fetched if omitted)
    #[arg(long)]
    pub id: Option<u32>,
    #[command(subcommand)]
    pub operation: OperationCommand,
    /// Execute the proposal immediately after creation. Requires the created proposal TTL to be zero.
    #[arg(long)]
    pub execute_immediately: bool,
}

#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OperationCommand {
    /// Set or remove a proxy for a price identifier
    Proxy(ProxyArgs),
    /// Set the governance action TTL
    SetTtl(SetTtlArgs),
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProxyArgs {
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    price_id: CliPriceIdentifier,

    #[command(flatten)]
    action: ProxyActionArgs,
}

impl ProxyArgs {
    pub fn insert(price_id: CliPriceIdentifier, proxy: String) -> Self {
        Self {
            price_id,
            action: ProxyActionArgs::insert(proxy),
        }
    }

    pub fn remove(price_id: CliPriceIdentifier) -> Self {
        Self {
            price_id,
            action: ProxyActionArgs::remove(),
        }
    }
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[group(required = true, multiple = false)]
pub struct ProxyActionArgs {
    /// JSON-encoded Proxy value to insert for this price ID.
    #[arg(long)]
    insert: Option<String>,
    /// Path to a JSON file containing the Proxy value to insert for this price ID.
    #[arg(long)]
    insert_file: Option<PathBuf>,
    /// Remove the proxy for this price ID.
    #[arg(long)]
    remove: bool,
}

impl ProxyActionArgs {
    pub fn insert(proxy: String) -> Self {
        Self {
            insert: Some(proxy),
            insert_file: None,
            remove: false,
        }
    }

    pub fn remove() -> Self {
        Self {
            insert: None,
            insert_file: None,
            remove: true,
        }
    }

    pub fn resolve(&self) -> anyhow::Result<Option<String>> {
        if self.remove {
            return Ok(None);
        }

        Ok(Some(load_text(
            self.insert.as_deref(),
            self.insert_file.as_deref(),
            "insert",
        )?))
    }
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[group(required = true, multiple = false)]
pub struct SetTtlArgs {
    /// New TTL in nanoseconds
    #[arg(long, alias = "nanos", alias = "nanoseconds")]
    pub ns: Option<u64>,
    /// New TTL in milliseconds
    #[arg(long, alias = "millis", alias = "milliseconds")]
    pub ms: Option<u64>,
    /// New TTL in seconds
    #[arg(long, alias = "seconds")]
    pub secs: Option<u64>,
}

impl SetTtlArgs {
    pub fn from_ns(ns: u64) -> Self {
        Self {
            ns: Some(ns),
            ms: None,
            secs: None,
        }
    }

    pub fn from_ms(ms: u64) -> Self {
        Self {
            ns: None,
            ms: Some(ms),
            secs: None,
        }
    }

    pub fn from_secs(secs: u64) -> Self {
        Self {
            ns: None,
            ms: None,
            secs: Some(secs),
        }
    }

    pub fn ttl(&self) -> Nanoseconds {
        self.ns
            .map(Nanoseconds::from_ns)
            .or_else(|| self.ms.map(Nanoseconds::from_ms))
            .or_else(|| self.secs.map(Nanoseconds::from_secs))
            .unwrap_or(Nanoseconds::zero())
    }
}

impl CreateProposal {
    #[tracing::instrument(skip_all, name = "governance_create", fields(oracle_id = %self.oracle_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let id = if let Some(id) = self.id {
            id
        } else {
            let next_id: u32 = ctx
                .near
                .view(&self.oracle_id, "gov_next_id")
                .await?
                .json()?;
            tracing::info!(id = next_id, "Auto-fetched next proposal ID");
            next_id
        };

        let operation = match &self.operation {
            OperationCommand::Proxy(args) => {
                let proxy: Option<Proxy> = args
                    .action
                    .resolve()?
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?;
                Operation::SetProxy {
                    id: args.price_id.into(),
                    proxy,
                }
            }
            OperationCommand::SetTtl(args) => Operation::SetActionTtl {
                new_ttl: args.ttl(),
            },
        };

        let signer = self.signer.signer();

        ctx.batch(&signer, &self.oracle_id)
            .call(
                Function::new("gov_create")
                    .args_json(json!({
                        "id": id,
                        "operation": operation,
                    }))
                    .deposit(NearToken::from_yoctonear(1))
                    .max_gas(),
            )
            .transact()
            .await?;

        if self.execute_immediately {
            let proposal: Option<Proposal<Operation>> = ctx
                .near
                .view(&self.oracle_id, "gov_get")
                .args_json(json!({ "id": id }))
                .await?
                .json()?;

            let proposal =
                proposal.ok_or_else(|| anyhow::anyhow!("created proposal {id} not found"))?;

            anyhow::ensure!(
                proposal.ttl == Nanoseconds::zero(),
                "cannot immediately execute proposal {id}: proposal TTL is {}",
                proposal.ttl,
            );

            execute_proposal(ctx, &self.signer, &self.oracle_id, id).await?;
            tracing::info!(id, "Proposal created and executed immediately");
        } else {
            tracing::info!(id, "Proposal created");
        }

        Ok(())
    }
}
