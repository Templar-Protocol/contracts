use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Args, Subcommand};
use near_account_id::AccountId;
use near_sdk::json_types::{Base58CryptoHash, Base64VecU8, U128};
use near_sdk::Gas;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_common::upgrade::UpgradeSource;
use templar_common::Nanoseconds;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;
use templar_gateway_types::NearToken;
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{
    AcceptedHistorySource, CircuitBreaker, CircuitBreakerSetConfig,
};
use templar_proxy_oracle_kernel::proxy::Proxy;
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_governance_common::{
    LegacyOperation, MethodPolicy, Operation, ReflexiveOperation,
};

use super::{decode_base64, load_json_file, ReflexiveKind, Role};
use crate::commands::duration::parse_duration;
use crate::commands::proxy_oracle::parse_price_identifier;
use crate::commands::signer::SignerArgs;
use crate::proxy::load_proxy_file;
use crate::resolve::GovernanceTarget;

#[derive(Args, Debug)]
pub struct CreateProposal {
    #[command(flatten)]
    pub(crate) target: GovernanceTarget,
    /// Proposal id; fetched from the governance contract's next id when omitted
    #[arg(long, value_name = "ID")]
    id: Option<u32>,
    /// Requested TTL, clamped up to the operation's minimum (e.g. `10s`, `100ns`).
    #[arg(long, value_name = "DURATION", default_value = "0ns", value_parser = parse_duration)]
    requested_ttl: Nanoseconds,
    /// After creating, wait for the proposal's TTL to elapse, then execute it.
    /// Blocks for the full (effective) TTL, so it is only practical for short ones.
    #[arg(long, conflicts_with = "print")]
    execute_when_ready: bool,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
    #[command(subcommand)]
    operation: ProposalOperation,
}

impl CreateProposal {
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
    pub fn unresolved_breaker_price_id(&self) -> Option<PriceIdentifier> {
        match &self.operation {
            ProposalOperation::AddCircuitBreaker(a) if a.breaker_id.is_none() => Some(a.price_id),
            _ => None,
        }
    }

    /// Fill in an auto-fetched breaker id for an `add-circuit-breaker` proposal.
    pub fn set_breaker_id(&mut self, id: u32) {
        if let ProposalOperation::AddCircuitBreaker(a) = &mut self.operation {
            a.breaker_id.get_or_insert(id);
        }
    }

    /// Build the gateway spec with the resolved governance account and proposal id.
    pub fn try_into_spec(
        self,
        governance_id: AccountId,
        id: u32,
    ) -> anyhow::Result<spec::CreateProposal> {
        Ok(spec::CreateProposal {
            governance_id,
            id,
            operation: self.operation.into_operation()?,
            requested_ttl: self.requested_ttl,
        })
    }
}

/// The typed proposal subcommands. Target ops build the pre-restructure [`LegacyOperation`] and map it
/// to the generic `TargetFunctionCall` form (baking in the correct `admin_*` method name and a sane
/// gas default); reflexive policy edits construct the [`ReflexiveOperation`] directly. Complex nested
/// payloads (circuit breakers, history sources) are supplied as JSON files that deserialize into the
/// real kernel types.
#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProposalOperation {
    /// Set or clear a feed's proxy configuration.
    SetProxy(SetProxyArgs),
    /// Configure a feed's circuit-breaker sampling.
    ConfigureCircuitBreakers(ConfigureCircuitBreakersArgs),
    /// Add a circuit breaker to a feed.
    AddCircuitBreaker(AddCircuitBreakerArgs),
    /// Remove a circuit breaker from a feed.
    RemoveCircuitBreaker(RemoveCircuitBreakerArgs),
    /// Manually trip or reset a feed.
    SetManualTrip(SetManualTripArgs),
    /// Re-arm a tripped circuit breaker.
    Rearm(RearmArgs),
    /// Enable or disable enforcement of a circuit breaker.
    SetEnforced(SetEnforcedArgs),
    /// Set a reflexive operation kind's timelock.
    SetReflexiveTtl(SetReflexiveTtlArgs),
    /// Set the conservative default policy for unlisted target methods.
    SetTargetDefault(SetTargetDefaultArgs),
    /// Add, update, or reset a per-method policy override.
    SetMethodPolicy(SetMethodPolicyArgs),
    /// Grant or revoke a governance role.
    SetRole(SetRoleArgs),
    /// Upgrade the proxy oracle's contract code.
    AdminUpgrade(UpgradeArgs),
    /// Call an arbitrary method on the proxy oracle.
    AdminFunctionCall(AdminFunctionCallArgs),
    /// Upgrade the governance contract itself.
    SelfUpgrade(UpgradeArgs),
}

impl ProposalOperation {
    #[allow(clippy::too_many_lines)]
    fn into_operation(self) -> anyhow::Result<Operation> {
        // Target and role/self-upgrade ops route through the shared legacy → generic mapping.
        let legacy = match self {
            Self::SetProxy(a) => {
                let proxy: Option<Proxy<Source>> = match a.proxy_file {
                    Some(path) => Some(
                        serde_json::from_value(load_proxy_file(&path)?)
                            .context("parse proxy configuration")?,
                    ),
                    None => None,
                };
                LegacyOperation::SetProxy {
                    id: a.price_id,
                    proxy,
                }
            }
            Self::ConfigureCircuitBreakers(a) => LegacyOperation::ConfigureCircuitBreakers {
                id: a.price_id,
                config: CircuitBreakerSetConfig {
                    sample_interval_ns: a.sample_interval,
                    history_len: a.history_len,
                },
            },
            Self::AddCircuitBreaker(a) => {
                let breaker_id = a.breaker_id.context(
                    "breaker id must be resolved before building an add-circuit-breaker proposal",
                )?;
                LegacyOperation::AddCircuitBreaker {
                    id: a.price_id,
                    breaker_id,
                    breaker: load_json_file::<CircuitBreaker>(&a.breaker_file)
                        .context("parse circuit breaker")?,
                }
            }
            Self::RemoveCircuitBreaker(a) => LegacyOperation::RemoveCircuitBreaker {
                id: a.price_id,
                breaker_id: a.breaker_id,
            },
            Self::SetManualTrip(a) => LegacyOperation::SetManualTrip {
                id: a.price_id,
                is_manually_tripped: a.tripped,
                metadata: a.metadata_base64.map(decode_base64).transpose()?,
            },
            Self::Rearm(a) => LegacyOperation::Rearm {
                id: a.price_id,
                breaker_id: a.breaker_id,
                armed_after_ns: a.armed_after,
                accepted_history_source: load_json_file::<AcceptedHistorySource>(
                    &a.history_source_file,
                )
                .context("parse accepted history source")?,
            },
            Self::SetEnforced(a) => LegacyOperation::SetEnforced {
                id: a.price_id,
                breaker_id: a.breaker_id,
                is_enforced: a.enforced,
            },
            Self::SetRole(a) => LegacyOperation::SetRole {
                account_id: a.account_id,
                role: a.role,
                set: !a.revoke,
            },
            Self::AdminUpgrade(a) => {
                let (code, migrate_args) = a.parts()?;
                LegacyOperation::AdminUpgrade { code, migrate_args }
            }
            Self::SelfUpgrade(a) => {
                let (code, migrate_args) = a.parts()?;
                LegacyOperation::SelfUpgrade { code, migrate_args }
            }
            Self::AdminFunctionCall(a) => {
                // Fail early on malformed args rather than sending garbage bytes.
                serde_json::from_str::<serde_json::Value>(&a.args)
                    .context("admin-function-call --args must be valid JSON")?;
                LegacyOperation::AdminFunctionCall {
                    method_name: a.method,
                    args: Base64VecU8(a.args.into_bytes()),
                    attached_deposit: U128(a.deposit.as_yoctonear()),
                    gas: a.gas,
                }
            }
            // Reflexive policy edits construct the new operation directly.
            Self::SetReflexiveTtl(a) => {
                return Ok(Operation::Reflexive(ReflexiveOperation::SetReflexiveTtl {
                    kind: a.kind,
                    ttl: a.ttl,
                }))
            }
            Self::SetTargetDefault(a) => {
                return Ok(Operation::Reflexive(ReflexiveOperation::SetTargetDefault {
                    policy: MethodPolicy {
                        ttl: a.ttl,
                        role: a.role,
                    },
                }))
            }
            Self::SetMethodPolicy(a) => {
                let policy = if a.reset {
                    None
                } else {
                    let role = a
                        .role
                        .context("--role is required unless --reset is given")?;
                    let ttl = a.ttl.context("--ttl is required unless --reset is given")?;
                    Some(MethodPolicy { ttl, role })
                };
                return Ok(Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
                    method: a.method,
                    policy,
                }));
            }
        };
        Operation::try_from(legacy).context("build target operation from typed subcommand")
    }
}

#[derive(Args, Debug)]
pub struct SetProxyArgs {
    /// Price identifier (32-byte hex, optional `0x` prefix).
    #[arg(long, value_name = "HEX", value_parser = parse_price_identifier)]
    price_id: PriceIdentifier,
    /// Proxy definition JSON; omit to clear the feed
    #[arg(long, value_name = "PATH")]
    proxy_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct ConfigureCircuitBreakersArgs {
    /// Price identifier (32-byte hex, optional `0x` prefix).
    #[arg(long, value_name = "HEX", value_parser = parse_price_identifier)]
    price_id: PriceIdentifier,
    /// Sampling interval between circuit-breaker observations (e.g. `1s`, `1000ns`).
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    sample_interval: Nanoseconds,
    /// Number of samples to retain in the breaker's history.
    #[arg(long, value_name = "N")]
    history_len: u32,
}

#[derive(Args, Debug)]
pub struct AddCircuitBreakerArgs {
    /// Price identifier (32-byte hex, optional `0x` prefix).
    #[arg(long, value_name = "HEX", value_parser = parse_price_identifier)]
    price_id: PriceIdentifier,
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
    /// Price identifier (32-byte hex, optional `0x` prefix).
    #[arg(long, value_name = "HEX", value_parser = parse_price_identifier)]
    price_id: PriceIdentifier,
    /// Breaker id to remove.
    #[arg(long, value_name = "ID")]
    breaker_id: u32,
}

#[derive(Args, Debug)]
pub struct SetManualTripArgs {
    /// Price identifier (32-byte hex, optional `0x` prefix).
    #[arg(long, value_name = "HEX", value_parser = parse_price_identifier)]
    price_id: PriceIdentifier,
    /// Whether the feed is manually tripped
    #[arg(long)]
    tripped: bool,
    /// Optional base64 metadata recorded with the trip.
    #[arg(long, value_name = "BASE64")]
    metadata_base64: Option<String>,
}

#[derive(Args, Debug)]
pub struct RearmArgs {
    /// Price identifier (32-byte hex, optional `0x` prefix).
    #[arg(long, value_name = "HEX", value_parser = parse_price_identifier)]
    price_id: PriceIdentifier,
    /// Breaker id to re-arm.
    #[arg(long, value_name = "ID")]
    breaker_id: u32,
    /// Delay before the breaker re-arms (e.g. `30s`, `1000ns`).
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    armed_after: Nanoseconds,
    /// AcceptedHistorySource definition JSON
    #[arg(long, value_name = "PATH")]
    history_source_file: PathBuf,
}

#[derive(Args, Debug)]
pub struct SetEnforcedArgs {
    /// Price identifier (32-byte hex, optional `0x` prefix).
    #[arg(long, value_name = "HEX", value_parser = parse_price_identifier)]
    price_id: PriceIdentifier,
    /// Breaker id to update.
    #[arg(long, value_name = "ID")]
    breaker_id: u32,
    /// Whether the breaker is enforced.
    #[arg(long)]
    enforced: bool,
}

#[derive(Args, Debug)]
pub struct SetReflexiveTtlArgs {
    /// Reflexive kind to set the timelock for.
    #[arg(long, value_enum)]
    kind: ReflexiveKind,
    /// New timelock (e.g. `1h`, `86400000000000ns`).
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    ttl: Nanoseconds,
}

#[derive(Args, Debug)]
pub struct SetTargetDefaultArgs {
    /// Conservative default timelock for unlisted target methods.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    ttl: Nanoseconds,
    /// Role required to invoke an unlisted target method.
    #[arg(long, value_enum)]
    role: Role,
}

#[derive(Args, Debug)]
pub struct SetMethodPolicyArgs {
    /// Target method name (e.g. `admin_set_proxy`).
    #[arg(long, value_name = "NAME")]
    method: String,
    /// Timelock for this method. Must be `<=` the target default. Required unless `--reset`.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration, required_unless_present = "reset")]
    ttl: Option<Nanoseconds>,
    /// Role required to invoke this method. Required unless `--reset`.
    #[arg(long, value_enum, required_unless_present = "reset")]
    role: Option<Role>,
    /// Remove the override, resetting the method to the target default.
    #[arg(long, conflicts_with_all = ["role", "ttl"])]
    reset: bool,
}

#[derive(Args, Debug)]
pub struct SetRoleArgs {
    /// Account to grant or revoke the role on.
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
    /// Role to set.
    #[arg(long, value_enum)]
    role: Role,
    /// Revoke the role instead of granting it
    #[arg(long)]
    revoke: bool,
}

/// The new code for an upgrade: exactly one of the two sources.
#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct UpgradeSourceArgs {
    /// WASM file to deploy.
    #[arg(long, value_name = "PATH")]
    code_file: Option<PathBuf>,
    /// Global contract code hash (base58) to deploy.
    #[arg(long, value_name = "BASE58")]
    global_hash: Option<Base58CryptoHash>,
}

impl UpgradeSourceArgs {
    fn into_source(self) -> anyhow::Result<UpgradeSource> {
        if let Some(path) = self.code_file {
            Ok(UpgradeSource::Code(Base64VecU8(
                std::fs::read(&path)
                    .with_context(|| format!("read WASM from {}", path.display()))?,
            )))
        } else if let Some(hash) = self.global_hash {
            Ok(UpgradeSource::GlobalHash(hash))
        } else {
            // The clap group is `required`, so exactly one of the above is always set.
            anyhow::bail!("no upgrade source provided")
        }
    }
}

/// A standardized upgrade: an [`UpgradeSourceArgs`] plus optional migrate args. Shared by the
/// oracle-targeted `AdminUpgrade` and the governance-contract `SelfUpgrade`.
#[derive(Args, Debug)]
pub struct UpgradeArgs {
    #[command(flatten)]
    code: UpgradeSourceArgs,
    /// Migrate args passed to `migrate` (raw bytes); empty if omitted
    #[arg(long, value_name = "PATH")]
    migrate_args_file: Option<PathBuf>,
}

impl UpgradeArgs {
    fn parts(self) -> anyhow::Result<(UpgradeSource, Base64VecU8)> {
        let code = self.code.into_source()?;
        let migrate_args = Base64VecU8(match self.migrate_args_file {
            Some(path) => std::fs::read(&path)
                .with_context(|| format!("read migrate args from {}", path.display()))?,
            None => Vec::new(),
        });
        Ok((code, migrate_args))
    }
}

#[derive(Args, Debug)]
pub struct AdminFunctionCallArgs {
    /// Method to call on the proxy oracle (e.g. `own_accept_owner`)
    #[arg(long, value_name = "NAME")]
    method: String,
    /// JSON argument string (raw bytes are what the oracle receives)
    #[arg(long, value_name = "JSON", default_value = "{}")]
    args: String,
    /// Deposit to attach to the call.
    #[arg(long, value_name = "AMOUNT", default_value_t = NearToken::from_yoctonear(0))]
    deposit: NearToken,
    /// Gas to attach to the call (e.g. `30 Tgas`).
    #[arg(long, value_name = "GAS", default_value_t = Gas::from_tgas(30))]
    gas: Gas,
}
