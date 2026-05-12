use std::path::PathBuf;

use near_sdk::serde_json::json;
use near_sdk::AccountId;
use near_sdk::NearToken;
use templar_common::governance::Proposal;
use templar_common::Nanoseconds;
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{CircuitBreaker, CircuitBreakerSet, CircuitBreakerSetConfig},
    Proxy,
};
use templar_proxy_oracle_near_common::{
    governance::{CircuitBreakerStatusUpdate, Operation},
    input::Source,
};
use templar_tools_common::near::{self, Function};

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
    /// Add a circuit breaker to a price identifier
    AddCircuitBreaker(AddCircuitBreakerArgs),
    /// Set shared circuit breaker sampling configuration for a price identifier
    CircuitBreakerConfig(CircuitBreakerConfigArgs),
    /// Set or clear the manual trip override for a price identifier
    CircuitBreakerManualTrip(CircuitBreakerManualTripArgs),
    /// Remove a circuit breaker from a price identifier
    RemoveCircuitBreaker(RemoveCircuitBreakerArgs),
    /// Update one circuit breaker's lifecycle status
    CircuitBreakerStatus(CircuitBreakerStatusArgs),
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

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AddCircuitBreakerArgs {
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    price_id: CliPriceIdentifier,
    /// Expected next breaker ID within the set (auto-fetched if omitted).
    #[arg(long)]
    breaker_id: Option<u32>,
    /// JSON-encoded `CircuitBreaker` value.
    #[arg(long)]
    breaker: Option<String>,
    /// Path to a JSON file containing `CircuitBreaker`.
    #[arg(long)]
    breaker_file: Option<PathBuf>,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CircuitBreakerConfigArgs {
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    price_id: CliPriceIdentifier,
    /// Shared sample interval in nanoseconds
    #[arg(long, alias = "nanos", alias = "nanoseconds")]
    sample_interval_ns: u64,
    /// Maximum number of persisted observations retained by the set
    #[arg(long)]
    history_len: u32,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RemoveCircuitBreakerArgs {
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    price_id: CliPriceIdentifier,
    /// Stable circuit breaker ID within the set.
    #[arg(long)]
    breaker_id: u32,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CircuitBreakerStatusArgs {
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    price_id: CliPriceIdentifier,
    /// Stable circuit breaker ID within the set.
    #[arg(long)]
    breaker_id: u32,
    /// JSON-encoded `CircuitBreakerStatusUpdate` value.
    #[arg(long)]
    status: Option<String>,
    /// Path to a JSON file containing `CircuitBreakerStatusUpdate`.
    #[arg(long)]
    status_file: Option<PathBuf>,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CircuitBreakerManualTripArgs {
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    price_id: CliPriceIdentifier,
    /// Whether the circuit breaker set should always block this feed.
    #[arg(long)]
    is_manually_tripped: bool,
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

impl AddCircuitBreakerArgs {
    async fn operation(
        &self,
        ctx: &CliContext,
        oracle_id: &AccountId,
    ) -> anyhow::Result<Operation> {
        let price_id = self.price_id.into();
        let breaker_id = if let Some(breaker_id) = self.breaker_id {
            breaker_id
        } else {
            let set: Option<CircuitBreakerSet> = near::view(
                &ctx.near,
                oracle_id,
                "get_proxy_circuit_breaker_set",
                json!({ "id": price_id }),
            )
            .await?;
            let next_id = set.unwrap_or_else(CircuitBreakerSet::empty).next_id;
            tracing::info!(breaker_id = next_id, "Auto-fetched next breaker ID");
            next_id
        };
        let breaker: CircuitBreaker = serde_json::from_str(&load_text(
            self.breaker.as_deref(),
            self.breaker_file.as_deref(),
            "breaker",
        )?)?;

        Ok(Operation::AddCircuitBreaker {
            id: price_id,
            breaker_id,
            breaker,
        })
    }
}

impl CreateProposal {
    #[tracing::instrument(skip_all, name = "governance_create", fields(oracle_id = %self.oracle_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let id = if let Some(id) = self.id {
            id
        } else {
            let next_id: u32 =
                near::view(&ctx.near, &self.oracle_id, "gov_next_id", json!({})).await?;
            tracing::info!(id = next_id, "Auto-fetched next proposal ID");
            next_id
        };

        let operation = match &self.operation {
            OperationCommand::Proxy(args) => {
                let proxy: Option<Proxy<Source>> = args
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
            OperationCommand::AddCircuitBreaker(args) => {
                args.operation(ctx, &self.oracle_id).await?
            }
            OperationCommand::CircuitBreakerConfig(args) => Operation::ConfigureCircuitBreakers {
                id: args.price_id.into(),
                config: CircuitBreakerSetConfig {
                    sample_interval_ns: Nanoseconds::from_ns(args.sample_interval_ns),
                    history_len: args.history_len,
                },
            },
            OperationCommand::CircuitBreakerManualTrip(args) => {
                Operation::SetCircuitBreakerManualTrip {
                    id: args.price_id.into(),
                    is_manually_tripped: args.is_manually_tripped,
                }
            }
            OperationCommand::RemoveCircuitBreaker(args) => Operation::RemoveCircuitBreaker {
                id: args.price_id.into(),
                breaker_id: args.breaker_id,
            },
            OperationCommand::CircuitBreakerStatus(args) => {
                let status: CircuitBreakerStatusUpdate = serde_json::from_str(&load_text(
                    args.status.as_deref(),
                    args.status_file.as_deref(),
                    "status",
                )?)?;
                Operation::SetCircuitBreakerStatus {
                    id: args.price_id.into(),
                    breaker_id: args.breaker_id,
                    status,
                }
            }
        };

        let signer = self.signer.signer();

        ctx.batch(&signer, &self.oracle_id)
            .call(
                Function::new("gov_create")
                    .args_json(json!({
                        "id": id,
                        "operation": operation,
                    }))?
                    .deposit(NearToken::from_yoctonear(1))
                    .max_gas(),
            )
            .transact()
            .await?;

        if self.execute_immediately {
            let proposal: Option<Proposal<Operation>> =
                near::view(&ctx.near, &self.oracle_id, "gov_get", json!({ "id": id })).await?;

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
