use std::path::PathBuf;

use anyhow::Context;
use near_sdk::json_types::Base64VecU8;
use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use near_sdk::Gas;
use near_sdk::NearToken;
use templar_common::Nanoseconds;
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{CircuitBreaker, CircuitBreakerSet, CircuitBreakerSetConfig},
    Proxy,
};
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_governance_common::{Operation, OperationKind, Proposal, Role};
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
    /// Requested TTL in seconds for this proposal (will be clamped to the configured minimum).
    #[arg(long)]
    pub requested_ttl_secs: Option<u64>,
    /// Execute the proposal immediately after creation. Requires the created proposal TTL to be zero.
    #[arg(long)]
    pub execute_immediately: bool,
}

#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OperationCommand {
    /// Set or remove a proxy for a price identifier
    Proxy(ProxyArgs),
    /// Add a circuit breaker to a price identifier
    AddCircuitBreaker(AddCircuitBreakerArgs),
    /// Set shared circuit breaker sampling configuration for a price identifier
    CircuitBreakerConfig(CircuitBreakerConfigArgs),
    /// Set or clear the manual trip override for a price identifier
    CircuitBreakerManualTrip(CircuitBreakerManualTripArgs),
    /// Rearm a circuit breaker (clear manual trips and set lifecycle)
    Rearm(RearmArgs),
    /// Set or clear the enforced block for a price identifier
    SetEnforced(SetEnforcedArgs),
    /// Remove a circuit breaker from a price identifier
    RemoveCircuitBreaker(RemoveCircuitBreakerArgs),
    /// Set the minimum TTL for one operation kind
    SetActionTtl(SetActionTtlArgs),
    SetRole(SetRoleArgs),
    /// Propose a contract upgrade for the proxy oracle
    AdminUpgrade(AdminUpgradeArgs),
    /// Propose an Admin-only function call on the configured proxy oracle
    AdminFunctionCall(AdminFunctionCallArgs),
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OperationKindArg {
    SetProxy,
    ConfigureCircuitBreakers,
    AddCircuitBreaker,
    RemoveCircuitBreaker,
    SetManualTrip,
    Rearm,
    SetEnforced,
    SetActionTtl,
    SetRole,
    AdminUpgrade,
    AdminFunctionCall,
}

impl From<OperationKindArg> for OperationKind {
    fn from(value: OperationKindArg) -> Self {
        match value {
            OperationKindArg::SetProxy => Self::SetProxy,
            OperationKindArg::ConfigureCircuitBreakers => Self::ConfigureCircuitBreakers,
            OperationKindArg::AddCircuitBreaker => Self::AddCircuitBreaker,
            OperationKindArg::RemoveCircuitBreaker => Self::RemoveCircuitBreaker,
            OperationKindArg::SetManualTrip => Self::SetManualTrip,
            OperationKindArg::Rearm => Self::Rearm,
            OperationKindArg::SetEnforced => Self::SetEnforced,
            OperationKindArg::SetActionTtl => Self::SetActionTtl,
            OperationKindArg::SetRole => Self::SetRole,
            OperationKindArg::AdminUpgrade => Self::AdminUpgrade,
            OperationKindArg::AdminFunctionCall => Self::AdminFunctionCall,
        }
    }
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoleArg {
    ManualTripper,
    CircuitBreakerOperator,
    ProxyConfigurationManager,
    Admin,
}

impl From<RoleArg> for Role {
    fn from(value: RoleArg) -> Self {
        match value {
            RoleArg::ManualTripper => Self::ManualTripper,
            RoleArg::CircuitBreakerOperator => Self::CircuitBreakerOperator,
            RoleArg::ProxyConfigurationManager => Self::ProxyConfigurationManager,
            RoleArg::Admin => Self::Admin,
        }
    }
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SetRoleArgs {
    #[arg(long)]
    account_id: AccountId,
    #[arg(long)]
    role: RoleArg,
    #[arg(long)]
    revoke: bool,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SetActionTtlArgs {
    /// Operation kind whose minimum proposal TTL should change.
    #[arg(long)]
    kind: OperationKindArg,
    /// New minimum TTL in seconds.
    #[arg(long)]
    secs: u64,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AdminUpgradeArgs {
    /// Path to the WASM contract file.
    #[arg(long)]
    code_file: PathBuf,
    /// JSON string for migration arguments.
    #[arg(long, conflicts_with = "migrate_args_file")]
    migrate_args: Option<String>,
    /// Path to a file containing JSON migration arguments.
    #[arg(long, conflicts_with = "migrate_args")]
    migrate_args_file: Option<PathBuf>,
}

impl AdminUpgradeArgs {
    fn resolve(&self) -> anyhow::Result<(Base64VecU8, Base64VecU8)> {
        let code = std::fs::read(&self.code_file)
            .with_context(|| format!("read code file `{}`", self.code_file.display()))?;
        let args_text = load_text(
            self.migrate_args.as_deref(),
            self.migrate_args_file.as_deref(),
            "migrate-args",
        )?;
        Ok((Base64VecU8(code), Base64VecU8(args_text.into_bytes())))
    }
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AdminFunctionCallArgs {
    /// Method to call on the configured proxy oracle account.
    #[arg(long)]
    method_name: String,
    /// JSON string for method arguments.
    #[arg(long, conflicts_with = "args_file")]
    args: Option<String>,
    /// Path to a file containing JSON method arguments.
    #[arg(long, conflicts_with = "args")]
    args_file: Option<PathBuf>,
    /// Attached deposit in yoctoNEAR.
    #[arg(long, default_value_t = 0)]
    attached_deposit_yocto: u128,
    /// Gas to attach to the call, in raw NEAR gas units.
    #[arg(long, required_unless_present = "tgas", conflicts_with = "tgas")]
    gas: Option<u64>,
    /// Gas to attach to the call, in TGas.
    #[arg(long, required_unless_present = "gas", conflicts_with = "gas")]
    tgas: Option<u64>,
}

impl AdminFunctionCallArgs {
    fn resolve(&self) -> anyhow::Result<(Base64VecU8, U128, Gas)> {
        let args_text = load_text(self.args.as_deref(), self.args_file.as_deref(), "args")?;
        let gas = match (self.gas, self.tgas) {
            (Some(gas), None) => Gas::from_gas(gas),
            (None, Some(tgas)) => Gas::from_tgas(tgas),
            _ => anyhow::bail!("specify exactly one of --gas or --tgas"),
        };
        Ok((
            Base64VecU8(args_text.into_bytes()),
            U128(self.attached_deposit_yocto),
            gas,
        ))
    }
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SetEnforcedArgs {
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    price_id: CliPriceIdentifier,
    /// Stable circuit breaker ID within the set.
    #[arg(long)]
    breaker_id: u32,
    /// Whether the circuit breaker set should always block this feed.
    #[arg(long)]
    is_enforced: bool,
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
pub struct CircuitBreakerManualTripArgs {
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    price_id: CliPriceIdentifier,
    /// Whether the circuit breaker set should always block this feed.
    #[arg(long)]
    is_manually_tripped: bool,
}

#[derive(clap::Args, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RearmArgs {
    /// Hex-encoded 32-byte price identifier
    #[arg(long)]
    price_id: CliPriceIdentifier,
    /// Stable circuit breaker ID within the set.
    #[arg(long)]
    breaker_id: u32,
    /// Time after which the circuit breaker is considered armed (in nanoseconds).
    #[arg(long)]
    armed_after_ns: u64,
    /// Source for accepted history after rearm (empty or observed)
    #[arg(long)]
    accepted_history_source: AcceptedHistorySourceArg,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AcceptedHistorySourceArg {
    Empty,
    Observed,
}

impl From<AcceptedHistorySourceArg>
    for templar_proxy_oracle_kernel::proxy::circuit_breaker::AcceptedHistorySource
{
    fn from(value: AcceptedHistorySourceArg) -> Self {
        match value {
            AcceptedHistorySourceArg::Empty => Self::Empty,
            AcceptedHistorySourceArg::Observed => Self::Observed,
        }
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
            let next_id = set.unwrap_or_else(CircuitBreakerSet::empty).next_id();
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
                near::view(&ctx.near, &self.oracle_id, "next_proposal_id", json!({})).await?;
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
            OperationCommand::CircuitBreakerManualTrip(args) => Operation::SetManualTrip {
                id: args.price_id.into(),
                is_manually_tripped: args.is_manually_tripped,
                metadata: None,
            },
            OperationCommand::Rearm(args) => Operation::Rearm {
                id: args.price_id.into(),
                breaker_id: args.breaker_id,
                armed_after_ns: Nanoseconds::from_ns(args.armed_after_ns),
                accepted_history_source: args.accepted_history_source.clone().into(),
            },
            OperationCommand::SetEnforced(args) => Operation::SetEnforced {
                id: args.price_id.into(),
                breaker_id: args.breaker_id,
                is_enforced: args.is_enforced,
            },
            OperationCommand::RemoveCircuitBreaker(args) => Operation::RemoveCircuitBreaker {
                id: args.price_id.into(),
                breaker_id: args.breaker_id,
            },
            OperationCommand::SetActionTtl(args) => Operation::SetActionTtl {
                kind: args.kind.into(),
                new_ttl: Nanoseconds::from_secs(args.secs),
            },
            OperationCommand::SetRole(args) => Operation::SetRole {
                account_id: args.account_id.clone(),
                role: args.role.into(),
                set: !args.revoke,
            },
            OperationCommand::AdminUpgrade(args) => {
                let (code, migrate_args) = args.resolve()?;
                Operation::AdminUpgrade { code, migrate_args }
            }
            OperationCommand::AdminFunctionCall(args) => {
                let (call_args, attached_deposit, gas) = args.resolve()?;
                Operation::AdminFunctionCall {
                    method_name: args.method_name.clone(),
                    args: call_args,
                    attached_deposit,
                    gas,
                }
            }
        };

        let signer = self.signer.signer();

        let requested_ttl = Nanoseconds::from_secs(self.requested_ttl_secs.unwrap_or(0));

        ctx.batch(&signer, &self.oracle_id)
            .call(
                Function::new("create_proposal")
                    .args_json(json!({
                        "id": id,
                        "operation": operation,
                        "requested_ttl": requested_ttl,
                    }))?
                    .deposit(NearToken::from_yoctonear(1))
                    .max_gas(),
            )
            .transact()
            .await?;

        if self.execute_immediately {
            let proposal: Option<Proposal<Operation>> = near::view(
                &ctx.near,
                &self.oracle_id,
                "get_proposal",
                json!({ "id": id }),
            )
            .await?;

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
