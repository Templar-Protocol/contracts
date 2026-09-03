use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::{
    canonical_sha256,
    domain::{
        AssetKind, DesiredRouteV1, Direction, Environment, OperationDraftV1, OperationV1, Vm,
        SCHEMA_VERSION,
    },
    environment,
    error::{Error, Result},
    evm::EvmChain as _,
    output::CommandData,
    state::{read_json, write_create_new_json, RouteStore},
};

#[derive(Debug, Parser)]
#[command(
    name = "tmplr-oft-bridge",
    version,
    about = "Non-USDC LayerZero OFT route operator"
)]
pub struct Cli {
    /// Emit the stable JSON envelope. Retained as an explicit global switch
    /// even though v1 currently emits JSON for every command.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init(InitArgs),
    Adopt(AdoptArgs),
    Operation(OperationArgs),
    Proposal(ProposalArgs),
    Artifact(ArtifactArgs),
    Asset(AssetArgs),
    Route(RouteArgs),
    Authority(AuthorityArgs),
    Stellar(StellarArgs),
    Contain(ContainArgs),
    Leg(LegArgs),
    Message(MessageArgs),
    Evidence(EvidenceArgs),
    Reconcile(ReconcileArgs),
    Health(StateOnlyArgs),
}

#[derive(Debug, Args)]
struct ReconcileArgs {
    #[arg(long)]
    state: PathBuf,
    /// Exit nonzero when custody reports any deficit.
    #[arg(long)]
    fail_on_deficit: bool,
}

/// Resolves an RPC URL from a named environment variable. URLs never enter
/// argv directly.
fn rpc_url(env_var: Option<&String>) -> Result<Option<String>> {
    env_var
        .map(String::as_str)
        .map(std::env::var)
        .transpose()
        .map_err(|error| Error::InvalidInput(format!("rpc env read failed: {error}")))
}

#[derive(Debug, Args)]
struct InitArgs {
    #[arg(long)]
    desired: PathBuf,
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    write: bool,
    /// Operator assertion of the target environment; must equal the
    /// identity-derived environment.
    #[arg(long)]
    network: Option<String>,
    /// Environment variable holding the Stellar RPC URL (required for --write).
    #[arg(long)]
    stellar_rpc_env: Option<String>,
    /// Environment variable holding the EVM RPC URL (required for --write).
    #[arg(long)]
    evm_rpc_env: Option<String>,
}

#[derive(Debug, Args)]
struct AdoptArgs {
    #[arg(long)]
    desired: PathBuf,
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    stellar_oft: String,
    #[arg(long)]
    evm_oft: String,
    #[arg(long)]
    write: bool,
}

#[derive(Debug, Args)]
struct StateOnlyArgs {
    #[arg(long)]
    state: PathBuf,
}

#[derive(Debug, Args)]
struct OperationArgs {
    #[command(subcommand)]
    command: OperationCommand,
}

#[derive(Debug, Subcommand)]
enum OperationCommand {
    Draft(DraftArgs),
}

#[derive(Debug, Args)]
struct DraftArgs {
    #[arg(long)]
    state: PathBuf,
    /// Closed operation command name (see OperationV1 variants, kebab-case).
    #[arg(long)]
    command: String,
    /// JSON file with the closed argument set for --command.
    #[arg(long)]
    args: PathBuf,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Args)]
struct ProposalArgs {
    #[command(subcommand)]
    command: ProposalCommand,
}

#[derive(Debug, Subcommand)]
enum ProposalCommand {
    Create(ProposalCreateArgs),
    Ingest(ProposalIngestArgs),
    StellarSignature(ProposalSignatureArgs),
    SafeVerify(ProposalSafeArgs),
}

#[derive(Debug, Args)]
struct ProposalCreateArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    draft: PathBuf,
    #[arg(long)]
    out: PathBuf,
    /// Environment variable holding the Stellar RPC URL.
    #[arg(long)]
    stellar_rpc_env: Option<String>,
    /// Environment variable holding the EVM RPC URL.
    #[arg(long)]
    evm_rpc_env: Option<String>,
}

#[derive(Debug, Args)]
struct ProposalIngestArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    proposal: PathBuf,
    #[arg(long)]
    executed_tx: String,
    #[arg(long)]
    write: bool,
}

#[derive(Debug, Args)]
struct ProposalSignatureArgs {
    #[command(subcommand)]
    command: ProposalSignatureCommand,
}

#[derive(Debug, Subcommand)]
enum ProposalSignatureCommand {
    Attach(ProposalAttachArgs),
    Verify(ProposalVerifyArgs),
}

#[derive(Debug, Args)]
struct ProposalAttachArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    proposal: PathBuf,
    #[arg(long)]
    public_key: String,
    #[arg(long)]
    signature: String,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Args)]
struct ProposalVerifyArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    proposal: PathBuf,
}

#[derive(Debug, Args)]
struct ProposalSafeArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    proposal: PathBuf,
    #[arg(long)]
    safe_tx: PathBuf,
}

#[derive(Debug, Args)]
struct ArtifactArgs {
    #[command(subcommand)]
    command: ArtifactCommand,
}

#[derive(Debug, Subcommand)]
enum ArtifactCommand {
    Verify(StateOnlyArgs),
    Build(ArtifactBuildArgs),
}

#[derive(Debug, Args)]
struct ArtifactBuildArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    out_dir: PathBuf,
    #[arg(long)]
    write: bool,
    /// Digest-verified build dependency archive (npm package closure).
    #[arg(long)]
    deps_archive: Option<PathBuf>,
}
#[derive(Debug, Args)]
struct AssetArgs {
    #[command(subcommand)]
    command: AssetCommand,
}

#[derive(Debug, Subcommand)]
enum AssetCommand {
    Wrap(WrapArgs),
}

#[derive(Debug, Args)]
struct WrapArgs {
    /// Asset identifier, asserted against the bound route's asset.
    #[arg(long)]
    asset: String,
    #[arg(long, value_enum)]
    asset_kind: AssetKindArg,
    #[arg(long)]
    state: PathBuf,
    /// Desired route JSON binding the operator topology and evidence.
    #[arg(long)]
    desired: PathBuf,
    #[arg(long)]
    name: String,
    #[arg(long)]
    symbol: String,
    /// Reserved live EVM deployer nonce; fetched via --evm-rpc-env when absent.
    #[arg(long)]
    evm_nonce: Option<u64>,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum AssetKindArg {
    NativeSac,
    IssuedSep41,
    Usdc,
}

impl From<AssetKindArg> for AssetKind {
    fn from(value: AssetKindArg) -> Self {
        match value {
            AssetKindArg::NativeSac => Self::NativeSac,
            AssetKindArg::IssuedSep41 => Self::IssuedSep41,
            AssetKindArg::Usdc => Self::Usdc,
        }
    }
}

#[derive(Debug, Args)]
struct RouteArgs {
    #[command(subcommand)]
    command: RouteCommand,
}

#[derive(Debug, Subcommand)]
enum RouteCommand {
    DraftConfig(OutputStateArgs),
    Inspect(StateOnlyArgs),
    SetPeer(SetPeerArgs),
    SetLibrary(SetLibraryArgs),
    RemoveReceiveTimeout(VmRemoteArgs),
    SetUln(ConfigHashArgs),
    SetExecutor(ConfigHashArgs),
    SetOptions(SetOptionsArgs),
}

#[derive(Debug, Args)]
struct OutputStateArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Args)]
struct SetPeerArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    vm: VmArg,
    #[arg(long)]
    remote_eid: u32,
    #[arg(long)]
    peer: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct SetLibraryArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    vm: VmArg,
    #[arg(long)]
    direction: String,
    #[arg(long)]
    remote_eid: u32,
    #[arg(long)]
    library: String,
    #[arg(long)]
    grace_period_seconds: Option<u64>,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct VmRemoteArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    vm: VmArg,
    #[arg(long)]
    remote_eid: u32,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct ConfigHashArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    vm: VmArg,
    /// ULN config direction; required for set-uln, rejected for set-executor.
    #[arg(long)]
    direction: Option<String>,
    #[arg(long)]
    remote_eid: u32,
    #[arg(long)]
    config: PathBuf,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct SetOptionsArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    vm: VmArg,
    #[arg(long)]
    remote_eid: u32,
    #[arg(long)]
    message_type: u16,
    #[arg(long)]
    options: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum VmArg {
    Stellar,
    Evm,
}
impl From<VmArg> for Vm {
    fn from(value: VmArg) -> Self {
        if matches!(value, VmArg::Stellar) {
            Vm::Stellar
        } else {
            Vm::Evm
        }
    }
}

#[derive(Debug, Args)]
struct AuthorityArgs {
    #[command(subcommand)]
    command: AuthorityCommand,
}
#[derive(Debug, Subcommand)]
enum AuthorityCommand {
    StellarBeginOwner(StellarBeginOwnerArgs),
    StellarAcceptOwner(StateEffectArgs),
    StellarCancelOwner(StateEffectArgs),
    StellarSetDelegate(StellarSetDelegateArgs),
    EvmTransferOwner(EvmTransferOwnerArgs),
    EvmSetDelegate(EvmSetDelegateArgs),
}
#[derive(Debug, Args)]
struct StellarBeginOwnerArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    new_owner: String,
    #[arg(long, default_value_t = 0)]
    ttl: u32,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct StateEffectArgs {
    #[arg(long)]
    state: PathBuf,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct StellarSetDelegateArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    delegate: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct EvmTransferOwnerArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    new_owner: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct EvmSetDelegateArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    delegate: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct StellarArgs {
    #[command(subcommand)]
    command: StellarCommand,
}
#[derive(Debug, Subcommand)]
enum StellarCommand {
    SetFee(SetFeeArgs),
    SetFeeDepositAddress(SetFeeDepositAddressArgs),
    SetMessageInspector(SetMessageInspectorArgs),
    SetRateLimit(SetRateLimitArgs),
    TtlSet(TtlSetArgs),
    TtlFreeze(StateEffectArgs),
    TtlExtendInstance(TtlExtendArgs),
    EmergencyPause(StateEffectArgs),
    EmergencyUnpause(StateEffectArgs),
    RoleGrant(RoleArgs),
    RoleRevoke(RoleArgs),
    RoleSetAdmin(RoleAdminArgs),
}

#[derive(Debug, Args)]
struct RoleAdminArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    role: String,
    #[arg(long)]
    admin_role: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct SetFeeArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    bps: u32,
    #[arg(long)]
    remote_eid: Option<u32>,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct SetFeeDepositAddressArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    recipient: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct SetMessageInspectorArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    inspector: Option<String>,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct SetRateLimitArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    direction: RateDirectionArg,
    #[arg(long)]
    remote_eid: u32,
    #[arg(long)]
    limit_raw: u128,
    #[arg(long)]
    window_seconds: u64,
    #[arg(long, default_value = "net")]
    mode: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct TtlSetArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    instance_threshold: u32,
    #[arg(long)]
    instance_extend_to: u32,
    #[arg(long)]
    persistent_threshold: u32,
    #[arg(long)]
    persistent_extend_to: u32,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct TtlExtendArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    ledgers: u32,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct RoleArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    role: String,
    #[arg(long)]
    address: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum RateDirectionArg {
    Inbound,
    Outbound,
}

#[derive(Debug, Args)]
struct ContainArgs {
    #[command(subcommand)]
    command: ContainCommand,
}
#[derive(Debug, Subcommand)]
enum ContainCommand {
    Outbound(ContainOutboundArgs),
    Inspect(StateOnlyArgs),
    Restore(ContainRestoreArgs),
}
#[derive(Debug, Args)]
struct ContainOutboundArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    direction: DirectionArg,
    /// Containment cap in base units; v1 supports only a full block (0).
    #[arg(long)]
    limit_raw: Option<u128>,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct ContainRestoreArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    snapshot: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum DirectionArg {
    StellarToEvm,
    EvmToStellar,
}
impl From<DirectionArg> for Direction {
    fn from(value: DirectionArg) -> Self {
        if matches!(value, DirectionArg::StellarToEvm) {
            Direction::StellarToEvm
        } else {
            Direction::EvmToStellar
        }
    }
}

#[derive(Debug, Args)]
struct LegArgs {
    #[command(subcommand)]
    command: LegCommand,
}
#[derive(Debug, Subcommand)]
enum LegCommand {
    Quote(LegQuoteArgs),
    Send(LegSendArgs),
}
#[derive(Debug, Args)]
struct LegQuoteArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    direction: DirectionArg,
    #[arg(long)]
    amount_raw: u128,
    #[arg(long)]
    to: String,
    #[arg(long)]
    out: PathBuf,
}
#[derive(Debug, Args)]
struct LegSendArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    intent: PathBuf,
    #[arg(long)]
    allow_additional_obligation: bool,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct MessageArgs {
    #[command(subcommand)]
    command: MessageCommand,
}
#[derive(Debug, Subcommand)]
enum MessageCommand {
    Watch(MessageWatchArgs),
    Recover(MessageRecoverArgs),
}
#[derive(Debug, Args)]
struct MessageWatchArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    guid: String,
    /// Stop condition for the watch; v1 accepts only `terminal`.
    #[arg(long)]
    until: Option<String>,
}
#[derive(Debug, Args)]
struct MessageRecoverArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    guid: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct EvidenceArgs {
    #[command(subcommand)]
    command: EvidenceCommand,
}
#[derive(Debug, Subcommand)]
enum EvidenceCommand {
    Import(EvidenceImportArgs),
}
#[derive(Debug, Args)]
struct EvidenceImportArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    bundle: PathBuf,
    #[arg(long)]
    write: bool,
}

#[derive(Debug, Args)]
struct ChainEffectArgs {
    #[arg(long, conflicts_with = "proposal_out")]
    execute: bool,
    #[arg(long, conflicts_with = "execute")]
    proposal_out: Option<PathBuf>,
    /// Environment variable holding the Stellar RPC URL for live-bound proposals.
    #[arg(long)]
    stellar_rpc_env: Option<String>,
    /// Environment variable holding the EVM RPC URL for live-bound proposals.
    #[arg(long)]
    evm_rpc_env: Option<String>,
}

impl Cli {
    pub fn command_name(&self) -> String {
        self.command.name().into()
    }

    pub fn run(self) -> Result<CommandData> {
        match self.command {
            Command::Init(args) => init(&args),
            Command::Adopt(args) => adopt(args),
            Command::Operation(args) => operation(args),
            Command::Proposal(args) => proposal(args),
            Command::Artifact(args) => artifact(args),
            Command::Asset(args) => asset(args),
            Command::Route(args) => route(args),
            Command::Authority(args) => generic_authority(args),
            Command::Stellar(args) => generic_stellar(args),
            Command::Contain(args) => contain(args),
            Command::Leg(args) => leg(args),
            Command::Message(args) => message(args),
            Command::Evidence(args) => evidence(args),
            Command::Reconcile(args) => {
                crate::reconcile::run_command(&args.state, args.fail_on_deficit)
            }
            Command::Health(args) => health(&args.state),
        }
    }
}

/// Compile-time exhaustive effect classification over the whole CLI surface.
/// Every new Clap variant must pick a class here; there are no wildcard
/// arms, so adding a command without classifying it breaks the build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandEffect {
    /// Read-only state inspection.
    LocalRead,
    /// Local file or state preparation; no chain interaction.
    LocalPrepare,
    /// Constructs, signs, or ingests a chain mutation. Testnet-only in v1.
    ChainMutation,
    /// Evidence import into the custody ledger; no chain interaction.
    EvidenceWrite,
}

impl CommandEffect {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalRead => "local_read",
            Self::LocalPrepare => "local_prepare",
            Self::ChainMutation => "chain_mutation",
            Self::EvidenceWrite => "evidence_write",
        }
    }
}

impl Cli {
    pub fn effect(&self) -> &'static str {
        self.command.effect().as_str()
    }
}

impl Command {
    const fn name(&self) -> &'static str {
        match self {
            Self::Init(_) => "init",
            Self::Adopt(_) => "adopt",
            Self::Operation(_) => "operation",
            Self::Proposal(_) => "proposal",
            Self::Artifact(_) => "artifact",
            Self::Asset(_) => "asset",
            Self::Route(_) => "route",
            Self::Authority(_) => "authority",
            Self::Stellar(_) => "stellar",
            Self::Contain(_) => "contain",
            Self::Leg(_) => "leg",
            Self::Message(_) => "message",
            Self::Evidence(_) => "evidence",
            Self::Reconcile(_) => "reconcile",
            Self::Health(_) => "health",
        }
    }

    pub(crate) fn effect(&self) -> CommandEffect {
        match self {
            Self::Init(_) | Self::Adopt(_) => CommandEffect::LocalPrepare,
            Self::Operation(args) => args.command.effect(),
            Self::Proposal(args) => args.command.effect(),
            Self::Artifact(args) => args.command.effect(),
            Self::Asset(args) => args.command.effect(),
            Self::Route(args) => args.command.effect(),
            Self::Authority(args) => args.command.effect(),
            Self::Stellar(args) => args.command.effect(),
            Self::Contain(args) => args.command.effect(),
            Self::Leg(args) => args.command.effect(),
            Self::Message(args) => args.command.effect(),
            Self::Evidence(args) => args.command.effect(),
            Self::Reconcile(_) | Self::Health(_) => CommandEffect::LocalRead,
        }
    }
}

impl OperationCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Draft(_) => CommandEffect::LocalPrepare,
        }
    }
}

impl ProposalCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Create(_) | Self::Ingest(_) => CommandEffect::ChainMutation,
            Self::StellarSignature(args) => match args.command {
                ProposalSignatureCommand::Attach(_) => CommandEffect::LocalPrepare,
                ProposalSignatureCommand::Verify(_) => CommandEffect::LocalRead,
            },
            Self::SafeVerify(_) => CommandEffect::LocalRead,
        }
    }
}

impl ArtifactCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Verify(_) => CommandEffect::LocalRead,
            Self::Build(_) => CommandEffect::LocalPrepare,
        }
    }
}

impl AssetCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Wrap(_) => CommandEffect::ChainMutation,
        }
    }
}

impl RouteCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::DraftConfig(_) => CommandEffect::LocalPrepare,
            Self::Inspect(_) => CommandEffect::LocalRead,
            Self::SetPeer(_)
            | Self::SetLibrary(_)
            | Self::RemoveReceiveTimeout(_)
            | Self::SetUln(_)
            | Self::SetExecutor(_)
            | Self::SetOptions(_) => CommandEffect::ChainMutation,
        }
    }
}

impl AuthorityCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::StellarBeginOwner(_)
            | Self::StellarAcceptOwner(_)
            | Self::StellarCancelOwner(_)
            | Self::StellarSetDelegate(_)
            | Self::EvmTransferOwner(_)
            | Self::EvmSetDelegate(_) => CommandEffect::ChainMutation,
        }
    }
}

impl StellarCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::SetFee(_)
            | Self::SetFeeDepositAddress(_)
            | Self::SetMessageInspector(_)
            | Self::SetRateLimit(_)
            | Self::TtlSet(_)
            | Self::TtlFreeze(_)
            | Self::TtlExtendInstance(_)
            | Self::EmergencyPause(_)
            | Self::EmergencyUnpause(_)
            | Self::RoleGrant(_)
            | Self::RoleRevoke(_)
            | Self::RoleSetAdmin(_) => CommandEffect::ChainMutation,
        }
    }
}

impl ContainCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Inspect(_) => CommandEffect::LocalRead,
            Self::Outbound(_) | Self::Restore(_) => CommandEffect::ChainMutation,
        }
    }
}

impl LegCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Quote(_) => CommandEffect::LocalPrepare,
            Self::Send(_) => CommandEffect::ChainMutation,
        }
    }
}

impl MessageCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Watch(_) => CommandEffect::LocalRead,
            Self::Recover(_) => CommandEffect::ChainMutation,
        }
    }
}

impl EvidenceCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Import(_) => CommandEffect::EvidenceWrite,
        }
    }
}

fn init(args: &InitArgs) -> Result<CommandData> {
    let desired = read_desired_precontext(&args.desired)?;
    let environment = environment::classify(&desired.identity)?;
    if let Some(network) = args.network.as_deref() {
        let expected = match environment {
            Environment::StellarTestnetSepolia => "stellar_testnet_sepolia",
            Environment::StellarMainnetEthereum => "stellar_mainnet_ethereum",
        };
        if network != expected {
            return Err(Error::InvalidInput(format!(
                "network mismatch: {network} but identity binds {expected}"
            )));
        }
    }
    if !args.write {
        return data(
            serde_json::json!({"preview": true, "desired_sha256": canonical_sha256(&desired)?}),
        );
    }
    environment::init_environment(
        &desired.identity,
        rpc_url(args.stellar_rpc_env.as_ref())?.as_deref(),
        rpc_url(args.evm_rpc_env.as_ref())?.as_deref(),
        true,
    )?;
    let (store, mut state) = RouteStore::create(&args.state, desired.clone())?;
    // Route authority records: proposal construction binds owners from state.
    state
        .contracts
        .insert("stellar_owner".into(), desired.stellar_owner.clone());
    state
        .contracts
        .insert("stellar_delegate".into(), desired.stellar_delegate.clone());
    state
        .contracts
        .insert("evm_owner".into(), desired.evm_owner.clone());
    state
        .contracts
        .insert("evm_delegate".into(), desired.evm_delegate.clone());
    store.save_state(&state)?;
    data(serde_json::to_value(state)?)
}

fn adopt(args: AdoptArgs) -> Result<CommandData> {
    let desired = read_desired_precontext(&args.desired)?;
    environment::classify(&desired.identity)?;
    if !args.write {
        return data(
            serde_json::json!({"preview": true, "stellar_oft": args.stellar_oft, "evm_oft": args.evm_oft}),
        );
    }
    let (store, mut state) = RouteStore::create(&args.state, desired.clone())?;
    state
        .contracts
        .insert("stellar_owner".into(), desired.stellar_owner.clone());
    state
        .contracts
        .insert("stellar_delegate".into(), desired.stellar_delegate.clone());
    state
        .contracts
        .insert("evm_owner".into(), desired.evm_owner.clone());
    state
        .contracts
        .insert("evm_delegate".into(), desired.evm_delegate.clone());
    state
        .contracts
        .insert("stellar_oft".into(), args.stellar_oft);
    state.contracts.insert("evm_oft".into(), args.evm_oft);
    store.save_state(&state)?;
    data(serde_json::to_value(state)?)
}
fn operation(args: OperationArgs) -> Result<CommandData> {
    match args.command {
        OperationCommand::Draft(args) => {
            let store = RouteStore::open(&args.state)?;
            let state = store.load_state()?;
            let value = read_json::<serde_json::Value>(&args.args)?;
            let operation = draft_operation(&args.command, &value)?;
            let draft = OperationDraftV1 {
                schema_name: "operation_draft".into(),
                schema_version: SCHEMA_VERSION,
                route_id: state.route_id,
                desired_sha256: state.desired_sha256,
                operation,
                observed_sha256: canonical_sha256(&state.effective_config)?,
            };
            write_create_new_json(&args.out, &draft)?;
            artifact_data("operation_draft", args.out, &draft, false)
        }
    }
}

fn str_field(value: &serde_json::Value, name: &str) -> Result<String> {
    value
        .get(name)
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .ok_or_else(|| Error::InvalidInput(format!("draft args: missing string field {name}")))
}

fn opt_str_field(value: &serde_json::Value, name: &str) -> Result<Option<String>> {
    match value.get(name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(Error::InvalidInput(format!(
            "draft args: field {name} must be a string or null"
        ))),
    }
}
fn num_field<T>(value: &serde_json::Value, name: &str) -> Result<T>
where
    T: TryFrom<u64>,
{
    let raw = value
        .get(name)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| Error::InvalidInput(format!("draft args: missing numeric field {name}")))?;
    T::try_from(raw)
        .map_err(|_| Error::InvalidInput(format!("draft args: field {name} overflows its type")))
}
/// Closed dispatch from a kebab-case command name and checked JSON args to
/// an OperationV1. No OperationV1 JSON is ever read directly.
fn draft_operation(command: &str, value: &serde_json::Value) -> Result<OperationV1> {
    match command {
        "install-stellar-wasm" => Ok(OperationV1::InstallStellarWasm {
            wasm_sha256: str_field(value, "wasm_sha256")?,
        }),
        "deploy-stellar-oft" => Ok(OperationV1::DeployStellarOft {
            salt: str_field(value, "salt")?,
        }),
        "deploy-evm-oft" => Ok(OperationV1::DeployEvmOft {
            deployer: str_field(value, "deployer")?,
            nonce: num_field(value, "nonce")?,
        }),
        "begin-stellar-ownership-transfer" => Ok(OperationV1::BeginStellarOwnershipTransfer {
            new_owner: str_field(value, "new_owner")?,
            ttl: num_field(value, "ttl")?,
        }),
        "accept-stellar-ownership" => Ok(OperationV1::AcceptStellarOwnership),
        "cancel-stellar-ownership-transfer" => Ok(OperationV1::CancelStellarOwnershipTransfer),
        "transfer-evm-ownership" => Ok(OperationV1::TransferEvmOwnership {
            new_owner: str_field(value, "new_owner")?,
        }),
        "set-stellar-delegate" => Ok(OperationV1::SetStellarDelegate {
            delegate: str_field(value, "delegate")?,
        }),
        "set-evm-delegate" => Ok(OperationV1::SetEvmDelegate {
            delegate: str_field(value, "delegate")?,
        }),
        "set-stellar-peer" => Ok(OperationV1::SetStellarPeer {
            remote_eid: num_field(value, "remote_eid")?,
            peer: str_field(value, "peer")?,
        }),
        "set-evm-peer" => Ok(OperationV1::SetEvmPeer {
            remote_eid: num_field(value, "remote_eid")?,
            peer: str_field(value, "peer")?,
        }),
        "set-stellar-send-library" => Ok(OperationV1::SetStellarSendLibrary {
            remote_eid: num_field(value, "remote_eid")?,
            library: str_field(value, "library")?,
        }),
        "set-stellar-receive-library" => Ok(OperationV1::SetStellarReceiveLibrary {
            remote_eid: num_field(value, "remote_eid")?,
            library: str_field(value, "library")?,
            grace_period_seconds: num_field(value, "grace_period_seconds")?,
        }),
        "remove-stellar-receive-library-timeout" => {
            Ok(OperationV1::RemoveStellarReceiveLibraryTimeout {
                remote_eid: num_field(value, "remote_eid")?,
            })
        }
        "set-evm-send-library" => Ok(OperationV1::SetEvmSendLibrary {
            remote_eid: num_field(value, "remote_eid")?,
            library: str_field(value, "library")?,
        }),
        "set-evm-receive-library" => Ok(OperationV1::SetEvmReceiveLibrary {
            remote_eid: num_field(value, "remote_eid")?,
            library: str_field(value, "library")?,
            grace_period_seconds: num_field(value, "grace_period_seconds")?,
        }),
        "remove-evm-receive-library-timeout" => Ok(OperationV1::RemoveEvmReceiveLibraryTimeout {
            remote_eid: num_field(value, "remote_eid")?,
        }),
        "set-stellar-uln-config" => Ok(OperationV1::SetStellarUlnConfig {
            remote_eid: num_field(value, "remote_eid")?,
            config_sha256: str_field(value, "config_sha256")?,
        }),
        "set-evm-uln-config" => Ok(OperationV1::SetEvmUlnConfig {
            remote_eid: num_field(value, "remote_eid")?,
            config_sha256: str_field(value, "config_sha256")?,
        }),
        _ => draft_operation_tail(command, value),
    }
}

/// Second half of the closed draft dispatch (executor/economic/admin/
/// containment commands).
fn draft_operation_tail(command: &str, value: &serde_json::Value) -> Result<OperationV1> {
    match command {
        "set-stellar-executor-config" => Ok(OperationV1::SetStellarExecutorConfig {
            remote_eid: num_field(value, "remote_eid")?,
            config_sha256: str_field(value, "config_sha256")?,
        }),
        "set-evm-executor-config" => Ok(OperationV1::SetEvmExecutorConfig {
            remote_eid: num_field(value, "remote_eid")?,
            config_sha256: str_field(value, "config_sha256")?,
        }),
        "set-stellar-receive-options" => Ok(OperationV1::SetStellarReceiveOptions {
            remote_eid: num_field(value, "remote_eid")?,
            message_type: num_field(value, "message_type")?,
            options: str_field(value, "options")?,
        }),
        "set-evm-receive-options" => Ok(OperationV1::SetEvmReceiveOptions {
            remote_eid: num_field(value, "remote_eid")?,
            message_type: num_field(value, "message_type")?,
            options: str_field(value, "options")?,
        }),
        "set-default-fee" => Ok(OperationV1::SetDefaultFee {
            bps: num_field(value, "bps")?,
        }),
        "set-destination-fee" => Ok(OperationV1::SetDestinationFee {
            remote_eid: num_field(value, "remote_eid")?,
            bps: num_field(value, "bps")?,
        }),
        "set-fee-recipient" => Ok(OperationV1::SetFeeRecipient {
            recipient: str_field(value, "recipient")?,
        }),
        "set-message-inspector" => Ok(OperationV1::SetMessageInspector {
            inspector: opt_str_field(value, "inspector")?,
        }),
        "set-inbound-rate-limit" => Ok(OperationV1::SetInboundRateLimit {
            remote_eid: num_field(value, "remote_eid")?,
            limit_raw: num_field(value, "limit_raw")?,
            window_seconds: num_field(value, "window_seconds")?,
            mode: str_field(value, "mode")?,
        }),
        "set-outbound-rate-limit" => Ok(OperationV1::SetOutboundRateLimit {
            remote_eid: num_field(value, "remote_eid")?,
            limit_raw: num_field(value, "limit_raw")?,
            window_seconds: num_field(value, "window_seconds")?,
            mode: str_field(value, "mode")?,
        }),
        _ => draft_operation_admin(command, value),
    }
}

/// Third half of the closed draft dispatch (ttl/role/recovery/containment).
fn draft_operation_admin(command: &str, value: &serde_json::Value) -> Result<OperationV1> {
    match command {
        "pause-emergency" => Ok(OperationV1::PauseEmergency),
        "unpause-emergency" => Ok(OperationV1::UnpauseEmergency),
        "set-ttl-config" => Ok(OperationV1::SetTtlConfig {
            instance_threshold: num_field(value, "instance_threshold")?,
            instance_extend_to: num_field(value, "instance_extend_to")?,
            persistent_threshold: num_field(value, "persistent_threshold")?,
            persistent_extend_to: num_field(value, "persistent_extend_to")?,
        }),
        "freeze-ttl-config" => Ok(OperationV1::FreezeTtlConfig {
            acknowledgement: str_field(value, "acknowledgement")?,
        }),
        "extend-instance-ttl" => Ok(OperationV1::ExtendInstanceTtl {
            ledgers: num_field(value, "ledgers")?,
        }),
        "grant-role" => Ok(OperationV1::GrantRole {
            role: str_field(value, "role")?,
            address: str_field(value, "address")?,
        }),
        "revoke-role" => Ok(OperationV1::RevokeRole {
            role: str_field(value, "role")?,
            address: str_field(value, "address")?,
        }),
        "set-role-admin" => Ok(OperationV1::SetRoleAdmin {
            role: str_field(value, "role")?,
            admin_role: str_field(value, "admin_role")?,
        }),
        "remove-role-admin" => Ok(OperationV1::RemoveRoleAdmin {
            role: str_field(value, "role")?,
            admin_role: str_field(value, "admin_role")?,
        }),
        "send-leg" => Ok(OperationV1::SendLeg {
            intent_sha256: str_field(value, "intent_sha256")?,
        }),
        "commit-verification" => Ok(OperationV1::CommitVerification {
            vm: match str_field(value, "vm")?.as_str() {
                "stellar" => Vm::Stellar,
                "evm" => Vm::Evm,
                other => {
                    return Err(Error::InvalidInput(format!(
                        "draft args: unknown vm {other}"
                    )))
                }
            },
            guid: str_field(value, "guid")?,
            packet_sha256: str_field(value, "packet_sha256")?,
        }),
        "execute-receive" => Ok(OperationV1::ExecuteReceive {
            vm: match str_field(value, "vm")?.as_str() {
                "stellar" => Vm::Stellar,
                "evm" => Vm::Evm,
                other => {
                    return Err(Error::InvalidInput(format!(
                        "draft args: unknown vm {other}"
                    )))
                }
            },
            guid: str_field(value, "guid")?,
            packet_sha256: str_field(value, "packet_sha256")?,
        }),
        "contain-outbound" => Ok(OperationV1::ContainOutbound {
            direction: match str_field(value, "direction")?.as_str() {
                "stellar_to_evm" => Direction::StellarToEvm,
                "evm_to_stellar" => Direction::EvmToStellar,
                other => {
                    return Err(Error::InvalidInput(format!(
                        "draft args: unknown direction {other}"
                    )))
                }
            },
        }),
        "restore-outbound" => Ok(OperationV1::RestoreOutbound {
            snapshot_sha256: str_field(value, "snapshot_sha256")?,
        }),
        "restore-footprint" => Ok(OperationV1::RestoreFootprint {
            original_operation_sha256: str_field(value, "original_operation_sha256")?,
        }),
        other => Err(Error::InvalidInput(format!(
            "unknown draft command {other}"
        ))),
    }
}

fn proposal(args: ProposalArgs) -> Result<CommandData> {
    match args.command {
        ProposalCommand::Create(args) => crate::governance::create_proposal(
            &args.state,
            &args.draft,
            &args.out,
            rpc_url(args.stellar_rpc_env.as_ref())?.as_deref(),
            rpc_url(args.evm_rpc_env.as_ref())?.as_deref(),
        ),
        ProposalCommand::Ingest(args) => crate::governance::ingest_proposal(
            &args.state,
            &args.proposal,
            &args.executed_tx,
            args.write,
        ),
        ProposalCommand::StellarSignature(args) => match args.command {
            ProposalSignatureCommand::Attach(args) => crate::governance::attach_signature_command(
                &args.state,
                &args.proposal,
                &args.public_key,
                &args.signature,
                &args.out,
            ),
            ProposalSignatureCommand::Verify(args) => {
                crate::governance::verify_stellar_proposal(&args.state, &args.proposal)
            }
        },
        ProposalCommand::SafeVerify(args) => {
            crate::governance::verify_safe_proposal(&args.state, &args.proposal, &args.safe_tx)
        }
    }
}

fn artifact(args: ArtifactArgs) -> Result<CommandData> {
    match args.command {
        ArtifactCommand::Verify(args) => crate::artifacts::verify_command(&args.state),
        ArtifactCommand::Build(args) => crate::artifacts::build_command(
            &args.state,
            &args.out_dir,
            args.write,
            args.deps_archive.as_deref(),
        ),
    }
}

fn asset(args: AssetArgs) -> Result<CommandData> {
    match args.command {
        AssetCommand::Wrap(args) => wrap(args),
    }
}

fn wrap(args: WrapArgs) -> Result<CommandData> {
    // Pre-dispatch USDC boundary before any state, artifact, or signer access.
    let kind: AssetKind = args.asset_kind.into();
    if kind == AssetKind::Usdc || crate::domain::is_known_usdc(&args.asset) {
        return Err(Error::Policy("unsupported_use_cctp".into()));
    }
    let desired = read_desired_precontext(&args.desired)?;
    let store = RouteStore::open(&args.state)?;
    let state = store.load_state()?;
    if state.asset.kind != kind || state.asset.asset_id != args.asset {
        return Err(Error::InvalidInput(
            "wrap asset must equal the bound route asset".into(),
        ));
    }
    if desired.route_id != state.route_id || canonical_sha256(&desired)? != state.desired_sha256 {
        return Err(Error::Conflict(
            "desired route does not bind to this route state".into(),
        ));
    }
    let require_evidence =
        environment::classify(&state.identity)? == Environment::StellarMainnetEthereum;
    let nonce = if let Some(nonce) = args.evm_nonce {
        nonce
    } else {
        let url = rpc_url(args.effect.evm_rpc_env.as_ref())?.ok_or_else(|| {
            Error::InvalidInput(
                "live_environment_required: EVM RPC URL is required without --evm-nonce".into(),
            )
        })?;
        let evm = crate::evm::HttpEvmChain::new(&url)?;
        let deployer = crate::evm::parse_address(&desired.evm_owner)?;
        crate::block_on_result(evm.account_nonce(deployer))?
    };
    let plan = crate::wrap::plan_wrap(
        &desired,
        &state.desired_sha256,
        &args.name,
        &args.symbol,
        nonce,
        require_evidence,
    )?;
    if args.effect.execute {
        return Err(Error::Chain(
            "wrap execution requires a qualified live adapter; use --proposal-out".into(),
        ));
    }
    if let Some(out) = args.effect.proposal_out {
        // v1 emits a proposal for the first plan node; the remaining nodes
        // are driven by the granular commands in plan order.
        let first = plan
            .operations
            .first()
            .cloned()
            .ok_or_else(|| Error::InvalidInput("wrap plan has no operations".into()))?;
        return crate::governance::proposal_for_operation(
            &args.state,
            &first,
            &out,
            rpc_url(args.effect.stellar_rpc_env.as_ref())?.as_deref(),
            rpc_url(args.effect.evm_rpc_env.as_ref())?.as_deref(),
        );
    }
    data(serde_json::to_value(&plan)?)
}

fn route(args: RouteArgs) -> Result<CommandData> {
    match args.command {
        RouteCommand::DraftConfig(args) => draft_config(&args.state, &args.out),
        RouteCommand::Inspect(args) => inspect(&args.state),
        RouteCommand::SetPeer(args) => {
            let remote_eid = args.remote_eid;
            let peer = args.peer;
            let operation = match args.vm.into() {
                Vm::Stellar => OperationV1::SetStellarPeer { remote_eid, peer },
                Vm::Evm => OperationV1::SetEvmPeer { remote_eid, peer },
            };
            chain_effect(&args.state, &operation, args.effect)
        }
        RouteCommand::SetLibrary(args) => {
            let direction = match args.direction.as_str() {
                "send" => LibraryDirection::Send,
                "receive" => LibraryDirection::Receive,
                other => {
                    return Err(Error::InvalidInput(format!(
                        "library direction must be send or receive, got {other}"
                    )))
                }
            };
            let operation = library_operation(
                args.vm.into(),
                direction,
                args.remote_eid,
                args.library,
                args.grace_period_seconds,
            )?;
            chain_effect(&args.state, &operation, args.effect)
        }
        RouteCommand::RemoveReceiveTimeout(args) => {
            let remote_eid = args.remote_eid;
            let operation = match args.vm.into() {
                Vm::Stellar => OperationV1::RemoveStellarReceiveLibraryTimeout { remote_eid },
                Vm::Evm => OperationV1::RemoveEvmReceiveLibraryTimeout { remote_eid },
            };
            chain_effect(&args.state, &operation, args.effect)
        }
        RouteCommand::SetUln(args) => config_hash_effect(args, true),
        RouteCommand::SetExecutor(args) => config_hash_effect(args, false),
        RouteCommand::SetOptions(args) => {
            let remote_eid = args.remote_eid;
            let message_type = args.message_type;
            let options = args.options;
            let operation = match args.vm.into() {
                Vm::Stellar => OperationV1::SetStellarReceiveOptions {
                    remote_eid,
                    message_type,
                    options,
                },
                Vm::Evm => OperationV1::SetEvmReceiveOptions {
                    remote_eid,
                    message_type,
                    options,
                },
            };
            chain_effect(&args.state, &operation, args.effect)
        }
    }
}

fn generic_authority(args: AuthorityArgs) -> Result<CommandData> {
    match args.command {
        AuthorityCommand::StellarBeginOwner(a) => chain_effect(
            &a.state,
            &OperationV1::BeginStellarOwnershipTransfer {
                new_owner: a.new_owner,
                ttl: a.ttl,
            },
            a.effect,
        ),
        AuthorityCommand::StellarAcceptOwner(a) => {
            chain_effect(&a.state, &OperationV1::AcceptStellarOwnership, a.effect)
        }
        AuthorityCommand::StellarCancelOwner(a) => chain_effect(
            &a.state,
            &OperationV1::CancelStellarOwnershipTransfer,
            a.effect,
        ),
        AuthorityCommand::StellarSetDelegate(a) => chain_effect(
            &a.state,
            &OperationV1::SetStellarDelegate {
                delegate: a.delegate,
            },
            a.effect,
        ),
        AuthorityCommand::EvmTransferOwner(a) => chain_effect(
            &a.state,
            &OperationV1::TransferEvmOwnership {
                new_owner: a.new_owner,
            },
            a.effect,
        ),
        AuthorityCommand::EvmSetDelegate(a) => chain_effect(
            &a.state,
            &OperationV1::SetEvmDelegate {
                delegate: a.delegate,
            },
            a.effect,
        ),
    }
}

fn chain_effect(
    state_path: &Path,
    operation: &OperationV1,
    effect: ChainEffectArgs,
) -> Result<CommandData> {
    let store = RouteStore::open(state_path)?;
    let state = store.load_state()?;
    environment::require_testnet(&state.identity)?;
    if !effect.execute && effect.proposal_out.is_none() {
        return data(serde_json::json!({"preview": true, "operation": operation}));
    }
    if let Some(out) = effect.proposal_out {
        return crate::governance::proposal_for_operation(
            state_path,
            operation,
            &out,
            rpc_url(effect.stellar_rpc_env.as_ref())?.as_deref(),
            rpc_url(effect.evm_rpc_env.as_ref())?.as_deref(),
        );
    }
    Err(Error::Chain("native execution requires a qualified live adapter; use --proposal-out until qualification succeeds".into()))
}

fn generic_stellar(args: StellarArgs) -> Result<CommandData> {
    match args.command {
        StellarCommand::SetFee(a) => {
            let operation = match a.remote_eid {
                Some(remote_eid) => OperationV1::SetDestinationFee {
                    remote_eid,
                    bps: a.bps,
                },
                None => OperationV1::SetDefaultFee { bps: a.bps },
            };
            chain_effect(&a.state, &operation, a.effect)
        }
        StellarCommand::SetFeeDepositAddress(a) => chain_effect(
            &a.state,
            &OperationV1::SetFeeRecipient {
                recipient: a.recipient,
            },
            a.effect,
        ),
        StellarCommand::SetMessageInspector(a) => chain_effect(
            &a.state,
            &OperationV1::SetMessageInspector {
                inspector: a.inspector,
            },
            a.effect,
        ),
        StellarCommand::SetRateLimit(a) => {
            let remote_eid = a.remote_eid;
            let limit_raw = a.limit_raw;
            let window_seconds = a.window_seconds;
            let mode = a.mode;
            let operation = match a.direction {
                RateDirectionArg::Inbound => OperationV1::SetInboundRateLimit {
                    remote_eid,
                    limit_raw,
                    window_seconds,
                    mode,
                },
                RateDirectionArg::Outbound => OperationV1::SetOutboundRateLimit {
                    remote_eid,
                    limit_raw,
                    window_seconds,
                    mode,
                },
            };
            chain_effect(&a.state, &operation, a.effect)
        }
        StellarCommand::TtlSet(a) => chain_effect(
            &a.state,
            &OperationV1::SetTtlConfig {
                instance_threshold: a.instance_threshold,
                instance_extend_to: a.instance_extend_to,
                persistent_threshold: a.persistent_threshold,
                persistent_extend_to: a.persistent_extend_to,
            },
            a.effect,
        ),
        StellarCommand::TtlFreeze(a) => chain_effect(
            &a.state,
            &OperationV1::FreezeTtlConfig {
                acknowledgement: "typed".into(),
            },
            a.effect,
        ),
        StellarCommand::TtlExtendInstance(a) => chain_effect(
            &a.state,
            &OperationV1::ExtendInstanceTtl { ledgers: a.ledgers },
            a.effect,
        ),
        StellarCommand::EmergencyPause(a) => {
            chain_effect(&a.state, &OperationV1::PauseEmergency, a.effect)
        }
        StellarCommand::EmergencyUnpause(a) => {
            chain_effect(&a.state, &OperationV1::UnpauseEmergency, a.effect)
        }
        StellarCommand::RoleGrant(a) => chain_effect(
            &a.state,
            &OperationV1::GrantRole {
                role: a.role,
                address: a.address,
            },
            a.effect,
        ),
        StellarCommand::RoleRevoke(a) => chain_effect(
            &a.state,
            &OperationV1::RevokeRole {
                role: a.role,
                address: a.address,
            },
            a.effect,
        ),

        StellarCommand::RoleSetAdmin(a) => chain_effect(
            &a.state,
            &OperationV1::SetRoleAdmin {
                role: a.role,
                admin_role: a.admin_role,
            },
            a.effect,
        ),
    }
}

enum LibraryDirection {
    Send,
    Receive,
}
fn library_operation(
    vm: Vm,
    direction: LibraryDirection,
    remote_eid: u32,
    library: String,
    grace_period_seconds: Option<u64>,
) -> Result<OperationV1> {
    Ok(match (vm, direction) {
        (Vm::Stellar, LibraryDirection::Send) => OperationV1::SetStellarSendLibrary {
            remote_eid,
            library,
        },
        (Vm::Stellar, LibraryDirection::Receive) => OperationV1::SetStellarReceiveLibrary {
            remote_eid,
            library,
            grace_period_seconds: grace_period_seconds.ok_or_else(|| {
                Error::InvalidInput("receive library requires --grace-period-seconds".into())
            })?,
        },
        (Vm::Evm, LibraryDirection::Send) => OperationV1::SetEvmSendLibrary {
            remote_eid,
            library,
        },
        (Vm::Evm, LibraryDirection::Receive) => OperationV1::SetEvmReceiveLibrary {
            remote_eid,
            library,
            grace_period_seconds: grace_period_seconds.ok_or_else(|| {
                Error::InvalidInput("receive library requires --grace-period-seconds".into())
            })?,
        },
    })
}

fn config_hash_effect(args: ConfigHashArgs, uln: bool) -> Result<CommandData> {
    if uln {
        match args.direction.as_deref() {
            Some("send" | "receive") => {}
            Some(other) => {
                return Err(Error::InvalidInput(format!(
                    "uln direction must be send or receive, got {other}"
                )))
            }
            None => {
                return Err(Error::InvalidInput(
                    "set-uln requires --direction send|receive".into(),
                ))
            }
        }
    } else if args.direction.is_some() {
        return Err(Error::InvalidInput(
            "set-executor has no --direction".into(),
        ));
    }
    let hash = file_sha256(&args.config)?;
    let remote_eid = args.remote_eid;
    let config_sha256 = hash;
    let operation = match (args.vm.into(), uln) {
        (Vm::Stellar, true) => OperationV1::SetStellarUlnConfig {
            remote_eid,
            config_sha256,
        },
        (Vm::Evm, true) => OperationV1::SetEvmUlnConfig {
            remote_eid,
            config_sha256,
        },
        (Vm::Stellar, false) => OperationV1::SetStellarExecutorConfig {
            remote_eid,
            config_sha256,
        },
        (Vm::Evm, false) => OperationV1::SetEvmExecutorConfig {
            remote_eid,
            config_sha256,
        },
    };
    chain_effect(&args.state, &operation, args.effect)
}

fn contain(args: ContainArgs) -> Result<CommandData> {
    match args.command {
        ContainCommand::Inspect(args) => crate::layerzero::containment_status(&args.state),
        ContainCommand::Outbound(args) => {
            if let Some(limit) = args.limit_raw {
                if limit != 0 {
                    return Err(Error::InvalidInput(
                        "v1 containment is zero-cap only: --limit-raw must be 0".into(),
                    ));
                }
            }
            chain_effect(
                &args.state,
                &OperationV1::ContainOutbound {
                    direction: args.direction.into(),
                },
                args.effect,
            )
        }
        ContainCommand::Restore(args) => chain_effect(
            &args.state,
            &OperationV1::RestoreOutbound {
                snapshot_sha256: args.snapshot,
            },
            args.effect,
        ),
    }
}

fn leg(args: LegArgs) -> Result<CommandData> {
    match args.command {
        LegCommand::Quote(args) => crate::canary::quote(
            &args.state,
            args.direction.into(),
            args.amount_raw,
            &args.to,
            &args.out,
        ),
        LegCommand::Send(args) => crate::canary::send(
            &args.state,
            &args.intent,
            args.allow_additional_obligation,
            args.effect.execute,
            args.effect.proposal_out.as_deref(),
            rpc_url(args.effect.stellar_rpc_env.as_ref())?.as_deref(),
            rpc_url(args.effect.evm_rpc_env.as_ref())?.as_deref(),
        ),
    }
}

fn message(args: MessageArgs) -> Result<CommandData> {
    match args.command {
        MessageCommand::Watch(args) => {
            match args.until.as_deref() {
                None | Some("terminal") => {}
                Some(other) => {
                    return Err(Error::InvalidInput(format!(
                        "watch --until accepts only terminal, got {other}"
                    )))
                }
            }
            crate::canary::watch(&args.state, &args.guid)
        }
        MessageCommand::Recover(args) => crate::canary::recover(
            &args.state,
            &args.guid,
            args.effect.execute,
            args.effect.proposal_out.as_deref(),
        ),
    }
}

fn evidence(args: EvidenceArgs) -> Result<CommandData> {
    match args.command {
        EvidenceCommand::Import(args) => {
            crate::canary::import_evidence(&args.state, &args.bundle, args.write)
        }
    }
}

fn draft_config(state_path: &Path, out: &Path) -> Result<CommandData> {
    let state = RouteStore::open(state_path)?.load_state()?;
    let value = serde_json::json!({"schema_name":"route_config_draft","schema_version":1,"route_id":state.route_id,"desired_sha256":state.desired_sha256,"effective":state.effective_config});
    write_create_new_json(out, &value)?;
    artifact_data("route_config_draft", out.to_path_buf(), &value, false)
}

fn inspect(state: &Path) -> Result<CommandData> {
    data(serde_json::to_value(
        RouteStore::open(state)?.load_state()?,
    )?)
}
fn health(state: &Path) -> Result<CommandData> {
    crate::reconcile::health_command(state)
}

fn read_desired_precontext(path: &Path) -> Result<DesiredRouteV1> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 1_048_576 {
        return Err(Error::InvalidInput(
            "desired route must be a real file no larger than 1 MiB".into(),
        ));
    }
    let desired: DesiredRouteV1 = read_json(path)?;
    desired.parse()
}

fn file_sha256(path: &Path) -> Result<String> {
    use sha2::{Digest as _, Sha256};
    Ok(hex::encode(Sha256::digest(fs::read(path)?)))
}

#[allow(clippy::unnecessary_wraps)]
fn data(result: serde_json::Value) -> Result<CommandData> {
    Ok(CommandData {
        result,
        artifact: None,
    })
}

fn artifact_data<T: Serialize>(
    kind: &str,
    path: PathBuf,
    value: &T,
    authoritative: bool,
) -> Result<CommandData> {
    let artifact = crate::domain::ArtifactRefV1 {
        kind: kind.into(),
        path,
        sha256: canonical_sha256(value)?,
        schema_version: SCHEMA_VERSION,
        authoritative,
    };
    Ok(CommandData {
        result: serde_json::json!({}),
        artifact: Some(artifact),
    })
}
