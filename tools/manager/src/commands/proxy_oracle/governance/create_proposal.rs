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
use templar_gateway_types::{NearToken, ProposalEncoding};
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{
    CircuitBreaker, CircuitBreakerSetConfig,
};
use templar_proxy_oracle_kernel::proxy::Proxy;
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_governance_common::{
    target, FunctionCall, MethodPolicy, Operation, ReflexiveOperation,
};

use super::{decode_base64, ReflexiveKind, Role};
use crate::commands::duration::parse_duration;
use crate::commands::load_json_file;
use crate::commands::proxy_oracle::parse_price_identifier;
use crate::commands::proxy_oracle::PreflightArgs;
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
    /// Argument encoding for the contract call. `borsh` is cheaper and carries larger payloads, but
    /// leaves an operation explorers and indexers cannot read — use it for wasm upgrades.
    #[arg(long, value_enum, default_value = "json")]
    encoding: ProposalEncoding,
    #[command(flatten)]
    pub(crate) preflight: PreflightArgs,
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

    /// Whether this proposal puts new code on the governed oracle, and so has to be preflighted
    /// against the state that code will load.
    pub fn is_oracle_upgrade(&self) -> bool {
        matches!(
            &self.operation,
            ProposalOperation::Oracle {
                op: OracleOp::Upgrade(_)
            }
        )
    }

    /// When this is an `oracle add-circuit-breaker` proposal with no explicit
    /// `--breaker-id`, the price id whose next breaker id must be fetched.
    pub fn unresolved_breaker_price_id(&self) -> Option<PriceIdentifier> {
        match &self.operation {
            ProposalOperation::Oracle {
                op: OracleOp::AddCircuitBreaker(a),
            } if a.breaker_id.is_none() => Some(a.price_id),
            _ => None,
        }
    }

    /// Fill in an auto-fetched breaker id for an `oracle add-circuit-breaker` proposal.
    pub fn set_breaker_id(&mut self, id: u32) {
        if let ProposalOperation::Oracle {
            op: OracleOp::AddCircuitBreaker(a),
        } = &mut self.operation
        {
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
            encoding: self.encoding,
        })
    }
}

/// The proposal target: an operation dispatched to the governed proxy `oracle`, or one that changes
/// governance itself (`self`). The two groups make the final target of a proposal explicit.
#[derive(Subcommand, Debug)]
pub enum ProposalOperation {
    /// Operation dispatched to the governed proxy oracle (a `TargetFunctionCall`).
    Oracle {
        #[command(subcommand)]
        op: OracleOp,
    },
    /// Operation that changes the governance contract itself (a reflexive operation).
    #[command(name = "self")]
    Governance {
        #[command(subcommand)]
        op: SelfOp,
    },
}

impl ProposalOperation {
    fn into_operation(self) -> anyhow::Result<Operation> {
        match self {
            ProposalOperation::Oracle { op } => op.into_operation(),
            ProposalOperation::Governance { op } => op.into_operation(),
        }
    }
}

/// Operations dispatched to the governed proxy oracle. Each builds the generic `TargetFunctionCall`
/// directly via the shared `target::admin_*` builders (correct `admin_*` method name + gas default).
/// Complex circuit-breaker payloads are supplied as JSON files that deserialize into kernel types.
#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum OracleOp {
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
    /// Upgrade the proxy oracle's contract code.
    Upgrade(UpgradeArgs),
    /// Call an arbitrary method on the proxy oracle.
    Call(OracleCallArgs),
}

impl OracleOp {
    fn into_operation(self) -> anyhow::Result<Operation> {
        Ok(Operation::TargetFunctionCall(match self {
            Self::SetProxy(a) => {
                let proxy: Option<Proxy<Source>> = match a.proxy_file {
                    Some(path) => Some(
                        serde_json::from_value(load_proxy_file(&path)?)
                            .context("parse proxy configuration")?,
                    ),
                    None => None,
                };
                target::admin_set_proxy(a.price_id, proxy, a.gas)?
            }
            Self::ConfigureCircuitBreakers(a) => target::admin_configure_circuit_breakers(
                a.price_id,
                CircuitBreakerSetConfig {
                    sample_interval_ns: a.sample_interval,
                    history_len: a.history_len,
                },
                a.gas,
            )?,
            Self::AddCircuitBreaker(a) => {
                let breaker_id = a.breaker_id.context(
                    "breaker id must be resolved before building an add-circuit-breaker proposal",
                )?;
                let breaker = load_json_file::<CircuitBreaker>(&a.breaker_file)
                    .context("parse circuit breaker")?;
                target::admin_add_circuit_breaker(a.price_id, breaker_id, breaker, a.gas)?
            }
            Self::RemoveCircuitBreaker(a) => {
                target::admin_remove_circuit_breaker(a.price_id, a.breaker_id, a.gas)?
            }
            Self::SetManualTrip(a) => {
                let metadata = a.metadata_base64.map(decode_base64).transpose()?;
                target::admin_set_manual_trip(a.price_id, a.tripped, metadata, a.gas)?
            }
            Self::Rearm(a) => target::admin_rearm(a.price_id, a.breaker_id, a.arming_delay, a.gas)?,
            Self::SetEnforced(a) => {
                target::admin_set_enforced(a.price_id, a.breaker_id, a.enforced, a.gas)?
            }
            Self::Upgrade(a) => {
                let (code, migrate_args) = a.parts()?;
                target::admin_upgrade(code, migrate_args, None)?
            }
            Self::Call(a) => {
                // Fail early on malformed args rather than sending garbage bytes. `IgnoredAny`
                // validates well-formedness without materializing the parsed tree.
                serde_json::from_str::<serde::de::IgnoredAny>(&a.args)
                    .context("oracle call --args must be valid JSON")?;
                FunctionCall {
                    method_name: a.method,
                    args: Base64VecU8(a.args.into_bytes()),
                    attached_deposit: U128(a.deposit.as_yoctonear()),
                    gas: a.gas,
                }
            }
        }))
    }
}

/// Reflexive operations that mutate the governance contract itself — the policy table, roles, and its
/// own upgrade.
#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum SelfOp {
    /// Set a reflexive operation kind's timelock.
    SetReflexiveTtl(SetReflexiveTtlArgs),
    /// Set the conservative default policy for unlisted target methods.
    SetTargetDefault(SetTargetDefaultArgs),
    /// Add, update, or reset a per-method policy override.
    SetMethodPolicy(SetMethodPolicyArgs),
    /// Grant or revoke a governance role.
    SetRole(SetRoleArgs),
    /// Upgrade the governance contract itself.
    Upgrade(UpgradeArgs),
}

impl SelfOp {
    fn into_operation(self) -> anyhow::Result<Operation> {
        Ok(Operation::Reflexive(match self {
            Self::SetReflexiveTtl(a) => ReflexiveOperation::SetReflexiveTtl {
                kind: a.kind,
                ttl: a.ttl,
            },
            Self::SetTargetDefault(a) => ReflexiveOperation::SetTargetDefault {
                policy: MethodPolicy {
                    ttl: a.ttl,
                    role: a.role,
                },
            },
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
                ReflexiveOperation::SetMethodPolicy {
                    method: a.method,
                    policy,
                }
            }
            Self::SetRole(a) => ReflexiveOperation::SetRole {
                account_id: a.account_id,
                role: a.role,
                set: !a.revoke,
            },
            Self::Upgrade(a) => {
                let (code, migrate_args) = a.parts()?;
                ReflexiveOperation::SelfUpgrade { code, migrate_args }
            }
        }))
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
    /// Gas to attach to the dispatched proxy-oracle call (e.g. `100 Tgas`); defaults to 30 Tgas.
    #[arg(long, value_name = "GAS")]
    gas: Option<Gas>,
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
    /// Gas to attach to the dispatched proxy-oracle call (e.g. `100 Tgas`); defaults to 30 Tgas.
    #[arg(long, value_name = "GAS")]
    gas: Option<Gas>,
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
    /// Gas to attach to the dispatched proxy-oracle call (e.g. `100 Tgas`); defaults to 30 Tgas.
    #[arg(long, value_name = "GAS")]
    gas: Option<Gas>,
}

#[derive(Args, Debug)]
pub struct RemoveCircuitBreakerArgs {
    /// Price identifier (32-byte hex, optional `0x` prefix).
    #[arg(long, value_name = "HEX", value_parser = parse_price_identifier)]
    price_id: PriceIdentifier,
    /// Breaker id to remove.
    #[arg(long, value_name = "ID")]
    breaker_id: u32,
    /// Gas to attach to the dispatched proxy-oracle call (e.g. `100 Tgas`); defaults to 30 Tgas.
    #[arg(long, value_name = "GAS")]
    gas: Option<Gas>,
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
    /// Gas to attach to the dispatched proxy-oracle call (e.g. `100 Tgas`); defaults to 30 Tgas.
    #[arg(long, value_name = "GAS")]
    gas: Option<Gas>,
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
    arming_delay: Nanoseconds,
    /// Gas to attach to the dispatched proxy-oracle call (e.g. `100 Tgas`); defaults to 30 Tgas.
    #[arg(long, value_name = "GAS")]
    gas: Option<Gas>,
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
    /// Gas to attach to the dispatched proxy-oracle call (e.g. `100 Tgas`); defaults to 30 Tgas.
    #[arg(long, value_name = "GAS")]
    gas: Option<Gas>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rearm_serializes_relative_delay_under_its_public_field_name() {
        let operation = OracleOp::Rearm(RearmArgs {
            price_id: PriceIdentifier([0; 32]),
            breaker_id: 7,
            arming_delay: Nanoseconds::from_secs(30),
            gas: None,
        })
        .into_operation()
        .unwrap();
        let Operation::TargetFunctionCall(call) = operation else {
            panic!("expected target function call");
        };
        let args: serde_json::Value = serde_json::from_slice(&call.args.0).unwrap();

        assert_eq!(args["arming_delay_ns"], "30000000000");
        assert!(args.get("armed_after_ns").is_none());
    }
}

#[derive(Args, Debug)]
pub struct OracleCallArgs {
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
    #[arg(long, value_name = "GAS", default_value_t = target::GAS_FOR_TARGET_DEFAULT)]
    gas: Gas,
}
