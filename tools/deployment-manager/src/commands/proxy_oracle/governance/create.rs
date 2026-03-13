use near_fetch::ops::Function;
use near_sdk::json_types::U64;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use near_sdk::NearToken;
use templar_common::oracle::proxy::governance::Operation;
use templar_common::oracle::proxy::Proxy;

use crate::commands::proxy_oracle::proxy::CliPriceIdentifier;
use crate::commands::SignerArgs;
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
}

#[derive(clap::Subcommand, Debug)]
pub enum OperationCommand {
    /// Set or remove a proxy for a price identifier
    SetProxy(SetProxyArgs),
    /// Set the governance action TTL
    SetTtl(SetTtlArgs),
}

#[derive(clap::Args, Debug)]
pub struct SetProxyArgs {
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    price_id: CliPriceIdentifier,
    /// JSON-encoded Proxy value. Omit to remove the proxy for this price ID.
    #[arg(long)]
    proxy: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct SetTtlArgs {
    /// New TTL in milliseconds
    #[arg(long)]
    pub ttl_ms: u64,
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
            OperationCommand::SetProxy(args) => {
                let proxy: Option<Proxy> = args
                    .proxy
                    .as_ref()
                    .map(|v| serde_json::from_str(v))
                    .transpose()?;
                Operation::SetProxy {
                    id: args.price_id.into_inner(),
                    proxy,
                }
            }
            OperationCommand::SetTtl(args) => Operation::SetActionTtl {
                new_ttl_ms: U64(args.ttl_ms),
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

        tracing::info!(id, "Proposal created");
        Ok(())
    }
}
