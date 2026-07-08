use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Args, Subcommand};
use near_account_id::AccountId;
use near_sdk::json_types::{Base64VecU8, U128};
use near_sdk::Gas;
use templar_common::Nanoseconds;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;
use templar_gateway_types::NearToken;
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{
    AcceptedHistorySource, CircuitBreaker, CircuitBreakerSetConfig,
};
use templar_proxy_oracle_kernel::proxy::Proxy;
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_governance_common::Operation;

use super::{decode_base64, load_json_file, OperationKindArg, RoleArg};
use crate::commands::proxy_oracle::parse_price_identifier;
use crate::proxy::load_proxy_file;

#[derive(Args, Debug)]
pub struct CreateProposal {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    /// Proposal id; fetched from the governance contract's next id when omitted
    #[arg(long, value_name = "ID")]
    id: Option<u32>,
    /// Requested TTL in nanoseconds (clamped up to the operation's minimum)
    #[arg(long, value_name = "NANOSECONDS", default_value = "0")]
    requested_ttl: u64,
    /// After creating, wait for the proposal's TTL to elapse, then execute it.
    /// Blocks for the full (effective) TTL, so it is only practical for short ones.
    #[arg(long)]
    execute_when_ready: bool,
    #[command(subcommand)]
    operation: ProposalOperation,
}

impl CreateProposal {
    pub fn governance_id(&self) -> &AccountId {
        &self.governance_id
    }

    /// The explicit `--id`, or `None` when it should be auto-fetched.
    pub fn id(&self) -> Option<u32> {
        self.id
    }

    /// Whether to wait for maturity and execute after creating.
    pub fn execute_when_ready(&self) -> bool {
        self.execute_when_ready
    }

    /// When this is an `add-circuit-breaker` proposal with no explicit
    /// `--breaker-id`, the price id whose next breaker id must be fetched.
    pub fn unresolved_breaker_price_id(&self) -> Option<&str> {
        match &self.operation {
            ProposalOperation::AddCircuitBreaker(a) if a.breaker_id.is_none() => {
                Some(a.price_id.as_str())
            }
            _ => None,
        }
    }

    /// Fill in an auto-fetched breaker id for an `add-circuit-breaker` proposal.
    pub fn set_breaker_id(&mut self, id: u32) {
        if let ProposalOperation::AddCircuitBreaker(a) = &mut self.operation {
            a.breaker_id.get_or_insert(id);
        }
    }

    /// Build the gateway spec with the resolved proposal id.
    pub fn into_spec(self, id: u32) -> anyhow::Result<spec::CreateProposal> {
        Ok(spec::CreateProposal {
            governance_id: self.governance_id,
            id,
            operation: self.operation.into_operation()?,
            requested_ttl: Nanoseconds::from_ns(self.requested_ttl),
        })
    }
}

/// One variant per `templar_proxy_oracle_near_governance_common::Operation`.
/// Complex nested payloads (circuit breakers, history sources) are supplied as
/// JSON files that deserialize into the real kernel types.
#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProposalOperation {
    SetProxy(SetProxyArgs),
    ConfigureCircuitBreakers(ConfigureCircuitBreakersArgs),
    AddCircuitBreaker(AddCircuitBreakerArgs),
    RemoveCircuitBreaker(RemoveCircuitBreakerArgs),
    SetManualTrip(SetManualTripArgs),
    Rearm(RearmArgs),
    SetEnforced(SetEnforcedArgs),
    SetActionTtl(SetActionTtlArgs),
    SetRole(SetRoleArgs),
    AdminUpgrade(AdminUpgradeArgs),
    AdminFunctionCall(AdminFunctionCallArgs),
}

impl ProposalOperation {
    fn into_operation(self) -> anyhow::Result<Operation> {
        Ok(match self {
            Self::SetProxy(a) => {
                let proxy: Option<Proxy<Source>> = match a.proxy_file {
                    Some(path) => Some(
                        serde_json::from_value(load_proxy_file(&path)?)
                            .context("parse proxy configuration")?,
                    ),
                    None => None,
                };
                Operation::SetProxy {
                    id: parse_price_identifier(&a.price_id)?,
                    proxy,
                }
            }
            Self::ConfigureCircuitBreakers(a) => Operation::ConfigureCircuitBreakers {
                id: parse_price_identifier(&a.price_id)?,
                config: CircuitBreakerSetConfig {
                    sample_interval_ns: Nanoseconds::from_ns(a.sample_interval_ns),
                    history_len: a.history_len,
                },
            },
            Self::AddCircuitBreaker(a) => Operation::AddCircuitBreaker {
                id: parse_price_identifier(&a.price_id)?,
                // Resolved to the set's next id by the dispatcher when omitted.
                breaker_id: a.breaker_id.unwrap_or(0),
                breaker: load_json_file::<CircuitBreaker>(&a.breaker_file)
                    .context("parse circuit breaker")?,
            },
            Self::RemoveCircuitBreaker(a) => Operation::RemoveCircuitBreaker {
                id: parse_price_identifier(&a.price_id)?,
                breaker_id: a.breaker_id,
            },
            Self::SetManualTrip(a) => Operation::SetManualTrip {
                id: parse_price_identifier(&a.price_id)?,
                is_manually_tripped: a.tripped,
                metadata: a.metadata_base64.map(decode_base64).transpose()?,
            },
            Self::Rearm(a) => Operation::Rearm {
                id: parse_price_identifier(&a.price_id)?,
                breaker_id: a.breaker_id,
                armed_after_ns: Nanoseconds::from_ns(a.armed_after_ns),
                accepted_history_source: load_json_file::<AcceptedHistorySource>(
                    &a.history_source_file,
                )
                .context("parse accepted history source")?,
            },
            Self::SetEnforced(a) => Operation::SetEnforced {
                id: parse_price_identifier(&a.price_id)?,
                breaker_id: a.breaker_id,
                is_enforced: a.enforced,
            },
            Self::SetActionTtl(a) => Operation::SetActionTtl {
                kind: a.kind.into(),
                new_ttl: Nanoseconds::from_ns(a.new_ttl),
            },
            Self::SetRole(a) => Operation::SetRole {
                account_id: a.account_id,
                role: a.role.into(),
                set: !a.revoke,
            },
            Self::AdminUpgrade(a) => Operation::AdminUpgrade {
                code: Base64VecU8(
                    std::fs::read(&a.code_file)
                        .with_context(|| format!("read WASM from {}", a.code_file.display()))?,
                ),
                migrate_args: Base64VecU8(match a.migrate_args_file {
                    Some(path) => std::fs::read(&path)
                        .with_context(|| format!("read migrate args from {}", path.display()))?,
                    None => Vec::new(),
                }),
            },
            Self::AdminFunctionCall(a) => Operation::AdminFunctionCall {
                method_name: a.method,
                args: Base64VecU8(a.args.into_bytes()),
                attached_deposit: U128(a.deposit.as_yoctonear()),
                gas: Gas::from_tgas(a.gas_tgas),
            },
        })
    }
}

#[derive(Args, Debug)]
pub struct SetProxyArgs {
    #[arg(long, value_name = "HEX")]
    price_id: String,
    /// Proxy definition JSON; omit to clear the feed
    #[arg(long, value_name = "PATH")]
    proxy_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct ConfigureCircuitBreakersArgs {
    #[arg(long, value_name = "HEX")]
    price_id: String,
    #[arg(long, value_name = "NANOSECONDS")]
    sample_interval_ns: u64,
    #[arg(long, value_name = "N")]
    history_len: u32,
}

#[derive(Args, Debug)]
pub struct AddCircuitBreakerArgs {
    #[arg(long, value_name = "HEX")]
    price_id: String,
    /// Stable breaker id within the set. Auto-fetched (the set's next id) when
    /// omitted.
    #[arg(long, value_name = "ID")]
    breaker_id: Option<u32>,
    /// CircuitBreaker definition JSON
    #[arg(long, value_name = "PATH")]
    breaker_file: PathBuf,
}

#[derive(Args, Debug)]
pub struct RemoveCircuitBreakerArgs {
    #[arg(long, value_name = "HEX")]
    price_id: String,
    #[arg(long, value_name = "ID")]
    breaker_id: u32,
}

#[derive(Args, Debug)]
pub struct SetManualTripArgs {
    #[arg(long, value_name = "HEX")]
    price_id: String,
    /// Whether the feed is manually tripped
    #[arg(long)]
    tripped: bool,
    #[arg(long, value_name = "BASE64")]
    metadata_base64: Option<String>,
}

#[derive(Args, Debug)]
pub struct RearmArgs {
    #[arg(long, value_name = "HEX")]
    price_id: String,
    #[arg(long, value_name = "ID")]
    breaker_id: u32,
    #[arg(long, value_name = "NANOSECONDS")]
    armed_after_ns: u64,
    /// AcceptedHistorySource definition JSON
    #[arg(long, value_name = "PATH")]
    history_source_file: PathBuf,
}

#[derive(Args, Debug)]
pub struct SetEnforcedArgs {
    #[arg(long, value_name = "HEX")]
    price_id: String,
    #[arg(long, value_name = "ID")]
    breaker_id: u32,
    #[arg(long)]
    enforced: bool,
}

#[derive(Args, Debug)]
pub struct SetActionTtlArgs {
    #[arg(long, value_enum)]
    kind: OperationKindArg,
    #[arg(long, value_name = "NANOSECONDS")]
    new_ttl: u64,
}

#[derive(Args, Debug)]
pub struct SetRoleArgs {
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
    #[arg(long, value_enum)]
    role: RoleArg,
    /// Revoke the role instead of granting it
    #[arg(long)]
    revoke: bool,
}

#[derive(Args, Debug)]
pub struct AdminUpgradeArgs {
    /// WASM file to deploy to the proxy oracle
    #[arg(long, value_name = "PATH")]
    code_file: PathBuf,
    /// Migrate args passed to the oracle's `migrate` (raw bytes); empty if omitted
    #[arg(long, value_name = "PATH")]
    migrate_args_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct AdminFunctionCallArgs {
    /// Method to call on the proxy oracle (e.g. `own_accept_owner`)
    #[arg(long, value_name = "NAME")]
    method: String,
    /// JSON argument string (raw bytes are what the oracle receives)
    #[arg(long, value_name = "JSON", default_value = "{}")]
    args: String,
    #[arg(long, value_name = "AMOUNT", default_value = "0 NEAR")]
    deposit: NearToken,
    #[arg(long = "gas", value_name = "TGAS", default_value_t = 30)]
    gas_tgas: u64,
}
