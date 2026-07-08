use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Args, Subcommand, ValueEnum};
use near_account_id::AccountId;
use near_sdk::json_types::{Base64VecU8, U128};
use near_sdk::Gas;
use serde::de::DeserializeOwned;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_common::Nanoseconds;
use templar_gateway_methods_spec::proxy_oracle as proxy_spec;
use templar_gateway_methods_spec::proxy_oracle_governance as governance_spec;
use templar_gateway_methods_spec::proxy_oracle_owner as owner_spec;
use templar_gateway_methods_spec::registry as registry_spec;
use templar_gateway_types::{Base64Bytes, NearToken};
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{
    AcceptedHistorySource, CircuitBreaker, CircuitBreakerSetConfig,
};
use templar_proxy_oracle_kernel::proxy::Proxy;
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_governance_common::{Operation, OperationKind, Role, TtlConfig};

use super::super::proxy::load_proxy_file;

// ---------------------------------------------------------------------------
// Owner (single-owner control of the proxy oracle account)
// ---------------------------------------------------------------------------

#[allow(clippy::enum_variant_names)] // these are the contract's own owner ops
#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProxyOracleOwnerNs {
    GetOwner(OracleIdArgs),
    GetProposedOwner(OracleIdArgs),
    ProposeOwner(ProposeOwner),
    AcceptOwner(OracleIdArgs),
    RenounceOwner(OracleIdArgs),
}

/// Shared argument for owner reads/writes keyed only by the oracle account.
#[derive(Args, Debug)]
pub struct OracleIdArgs {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
}

impl OracleIdArgs {
    pub fn get_owner(self) -> owner_spec::GetOwner {
        owner_spec::GetOwner {
            oracle_id: self.oracle_id,
        }
    }
    pub fn get_proposed_owner(self) -> owner_spec::GetProposedOwner {
        owner_spec::GetProposedOwner {
            oracle_id: self.oracle_id,
        }
    }
    pub fn accept_owner(self) -> owner_spec::AcceptOwner {
        owner_spec::AcceptOwner {
            oracle_id: self.oracle_id,
        }
    }
    pub fn renounce_owner(self) -> owner_spec::RenounceOwner {
        owner_spec::RenounceOwner {
            oracle_id: self.oracle_id,
        }
    }
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

// ---------------------------------------------------------------------------
// Governance (the separate contract that owns and administers the oracle)
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProxyOracleGovernanceNs {
    Create(GovernanceCreate),
    CreateProposal(CreateProposal),
    CancelProposal(ProposalRef),
    ExecuteProposal(ProposalRef),
    GetProposal(ProposalRef),
    ListProposals(ListProposals),
    NextProposalId(GovernanceIdArgs),
    ProposalCount(GovernanceIdArgs),
    GetOperationTtl(GetOperationTtl),
    GetProxyOracleId(GovernanceIdArgs),
    HasRole(HasRole),
    ListRole(ListRole),
    GetRoles(GetRoles),
}

/// Create (deploy-from-registry) a governance contract, building its
/// `new(proxy_oracle_id, admin_id, ttls)` init args from typed flags.
///
/// A governance contract administers exactly one proxy oracle and must be made
/// that oracle's owner after creation (propose-owner to it, then have it
/// execute an `admin-function-call own_accept_owner` proposal).
#[derive(Args, Debug)]
pub struct GovernanceCreate {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long, value_name = "NAME")]
    name: String,
    #[arg(long, value_name = "KEY")]
    version_key: String,
    /// The proxy oracle account this governance contract will administer
    #[arg(long, value_name = "ACCOUNT_ID")]
    proxy_oracle_id: AccountId,
    /// The account granted the Admin role
    #[arg(long, value_name = "ACCOUNT_ID")]
    admin_id: AccountId,
    /// Default proposal TTL (nanoseconds) applied to every operation kind
    #[arg(long, value_name = "NANOSECONDS", default_value = "0")]
    ttl_default: u64,
    /// Full TtlConfig JSON, overriding --ttl-default with per-operation TTLs
    #[arg(long, value_name = "PATH")]
    ttls_file: Option<PathBuf>,
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

/// Init args for the governance contract's `new(proxy_oracle_id, admin_id, ttls)`.
#[derive(serde::Serialize)]
struct GovernanceInit {
    proxy_oracle_id: AccountId,
    admin_id: AccountId,
    ttls: TtlConfig,
}

impl GovernanceCreate {
    pub fn parse(self) -> anyhow::Result<registry_spec::Deploy> {
        let ttls = match self.ttls_file {
            Some(path) => load_json_file(&path).context("parse TtlConfig")?,
            None => uniform_ttls(Nanoseconds::from_ns(self.ttl_default)),
        };

        let init = GovernanceInit {
            proxy_oracle_id: self.proxy_oracle_id,
            admin_id: self.admin_id,
            ttls,
        };
        let init_args = serde_json::to_vec(&init).context("encode governance init args")?;

        Ok(registry_spec::Deploy {
            registry_id: self.registry_id,
            name: self.name,
            version_key: self.version_key,
            init_args: Base64Bytes(init_args),
            full_access_keys: None,
            deposit: self.deposit,
        })
    }
}

/// A governance proposal keyed by id (cancel / execute / get).
#[derive(Args, Debug)]
pub struct ProposalRef {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long, value_name = "ID")]
    id: u32,
}

impl ProposalRef {
    pub fn cancel(self) -> governance_spec::CancelProposal {
        governance_spec::CancelProposal {
            governance_id: self.governance_id,
            id: self.id,
        }
    }
    pub fn execute(self) -> governance_spec::ExecuteProposal {
        governance_spec::ExecuteProposal {
            governance_id: self.governance_id,
            id: self.id,
        }
    }
    pub fn get(self) -> governance_spec::GetProposal {
        governance_spec::GetProposal {
            governance_id: self.governance_id,
            id: self.id,
        }
    }
}

#[derive(Args, Debug)]
pub struct GovernanceIdArgs {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
}

impl GovernanceIdArgs {
    pub fn next_proposal_id(self) -> governance_spec::NextProposalId {
        governance_spec::NextProposalId {
            governance_id: self.governance_id,
        }
    }
    pub fn proposal_count(self) -> governance_spec::ProposalCount {
        governance_spec::ProposalCount {
            governance_id: self.governance_id,
        }
    }
    pub fn get_proxy_oracle_id(self) -> governance_spec::GetProxyOracleId {
        governance_spec::GetProxyOracleId {
            governance_id: self.governance_id,
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

#[derive(Args, Debug)]
pub struct GetOperationTtl {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long, value_enum)]
    kind: OperationKindArg,
}

impl GetOperationTtl {
    pub fn parse(self) -> governance_spec::GetOperationTtl {
        governance_spec::GetOperationTtl {
            governance_id: self.governance_id,
            kind: self.kind.into(),
        }
    }
}

#[derive(Args, Debug)]
pub struct HasRole {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
    #[arg(long, value_enum)]
    role: RoleArg,
}

impl HasRole {
    pub fn parse(self) -> governance_spec::HasRole {
        governance_spec::HasRole {
            governance_id: self.governance_id,
            account_id: self.account_id,
            role: self.role.into(),
        }
    }
}

#[derive(Args, Debug)]
pub struct ListRole {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long, value_enum)]
    role: RoleArg,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    count: Option<u32>,
}

impl ListRole {
    pub fn parse(self) -> governance_spec::ListRole {
        governance_spec::ListRole {
            governance_id: self.governance_id,
            role: self.role.into(),
            offset: self.offset,
            count: self.count,
        }
    }
}

#[derive(Args, Debug)]
pub struct GetRoles {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
}

impl GetRoles {
    pub fn parse(self) -> governance_spec::GetRoles {
        governance_spec::GetRoles {
            governance_id: self.governance_id,
            account_id: self.account_id,
        }
    }
}

// ---------------------------------------------------------------------------
// create-proposal: shared header + one subcommand per governance Operation
// ---------------------------------------------------------------------------

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

    /// Build the gateway spec with the resolved proposal id.
    pub fn into_spec(self, id: u32) -> anyhow::Result<governance_spec::CreateProposal> {
        Ok(governance_spec::CreateProposal {
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
                breaker_id: a.breaker_id,
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
    #[arg(long, value_name = "ID")]
    breaker_id: u32,
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

// ---------------------------------------------------------------------------
// Oracle data
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProxyOracleNs {
    GetProxy(GetProxy),
    ListProxies(ListProxies),
    PriceFeedExists(PriceFeedExists),
    UpdatePrices(UpdatePrices),
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

#[derive(Args, Debug)]
pub struct PriceFeedExists {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long, value_name = "HEX")]
    price_id: String,
}

impl PriceFeedExists {
    pub fn parse(self) -> anyhow::Result<proxy_spec::PriceFeedExists> {
        Ok(proxy_spec::PriceFeedExists {
            oracle_id: self.oracle_id,
            price_identifier: parse_price_identifier(&self.price_id)?,
        })
    }
}

#[derive(Args, Debug)]
pub struct UpdatePrices {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Price identifiers (hex) to refresh; repeat the flag per feed
    #[arg(long = "price-id", value_name = "HEX", required = true)]
    price_ids: Vec<String>,
}

impl UpdatePrices {
    pub fn parse(self) -> anyhow::Result<proxy_spec::UpdatePrices> {
        let price_ids = self
            .price_ids
            .iter()
            .map(|hex| parse_price_identifier(hex))
            .collect::<anyhow::Result<_>>()?;
        Ok(proxy_spec::UpdatePrices {
            oracle_id: self.oracle_id,
            price_ids,
        })
    }
}

// ---------------------------------------------------------------------------
// Local clap mirrors of governance enums (keeps governance-common clap-free)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RoleArg {
    ManualTripper,
    CircuitBreakerOperator,
    ProxyConfigurationManager,
    Admin,
}

impl From<RoleArg> for Role {
    fn from(role: RoleArg) -> Self {
        match role {
            RoleArg::ManualTripper => Self::ManualTripper,
            RoleArg::CircuitBreakerOperator => Self::CircuitBreakerOperator,
            RoleArg::ProxyConfigurationManager => Self::ProxyConfigurationManager,
            RoleArg::Admin => Self::Admin,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
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
    fn from(kind: OperationKindArg) -> Self {
        match kind {
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn uniform_ttls(ttl: Nanoseconds) -> TtlConfig {
    TtlConfig {
        set_proxy: ttl,
        configure_circuit_breakers: ttl,
        add_circuit_breaker: ttl,
        remove_circuit_breaker: ttl,
        set_manual_trip: ttl,
        rearm: ttl,
        set_enforced: ttl,
        set_action_ttl: ttl,
        set_role: ttl,
        admin_upgrade: ttl,
        admin_function_call: ttl,
    }
}

fn load_json_file<T: DeserializeOwned>(path: &std::path::Path) -> anyhow::Result<T> {
    let contents =
        std::fs::read(path).with_context(|| format!("read JSON from {}", path.display()))?;
    serde_json::from_slice(&contents).with_context(|| format!("parse JSON from {}", path.display()))
}

fn decode_base64(value: String) -> anyhow::Result<Vec<u8>> {
    // Reuse Base64Bytes' base64 deserializer rather than adding a base64 dep.
    let bytes: Base64Bytes = serde_json::from_value(serde_json::Value::String(value))
        .context("decode base64 metadata")?;
    Ok(bytes.0)
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
