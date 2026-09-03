use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::{
    canonical_sha256,
    domain::{
        AssetKind, DesiredRouteV1, Direction, OperationDraftV1, OperationV1, Vm, SCHEMA_VERSION,
    },
    environment,
    error::{Error, Result},
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

#[derive(Debug, Args)]
struct InitArgs {
    #[arg(long)]
    desired: PathBuf,
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    write: bool,
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
    #[arg(long)]
    operation: PathBuf,
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
    #[arg(long)]
    asset: String,
    #[arg(long, value_enum)]
    asset_kind: AssetKindArg,
    #[arg(long)]
    state: Option<PathBuf>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    symbol: Option<String>,
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
}

fn init(args: &InitArgs) -> Result<CommandData> {
    let desired = read_desired_precontext(&args.desired)?;
    environment::classify(&desired.identity)?;
    if !args.write {
        return data(
            serde_json::json!({"preview": true, "desired_sha256": canonical_sha256(&desired)?}),
        );
    }
    let (_, state) = RouteStore::create(&args.state, desired.clone())?;
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
    let (store, mut state) = RouteStore::create(&args.state, desired)?;
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
            let operation: OperationV1 = read_json(&args.operation)?;
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

fn proposal(args: ProposalArgs) -> Result<CommandData> {
    match args.command {
        ProposalCommand::Create(args) => {
            crate::governance::create_proposal(&args.state, &args.draft, &args.out)
        }
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
        ArtifactCommand::Build(args) => {
            crate::artifacts::build_command(&args.state, &args.out_dir, args.write)
        }
    }
}

fn asset(args: AssetArgs) -> Result<CommandData> {
    match args.command {
        AssetCommand::Wrap(args) => {
            let kind: AssetKind = args.asset_kind.into();
            if kind == AssetKind::Usdc || crate::domain::is_known_usdc(&args.asset) {
                return Err(Error::Policy("unsupported_use_cctp".into()));
            }
            let state = args.state.ok_or_else(|| {
                Error::InvalidInput("--state is required for non-USDC assets".into())
            })?;
            let operation = OperationV1::DeployStellarOft {
                salt: canonical_sha256(&(args.asset, args.name, args.symbol))?,
            };
            chain_effect(&state, operation, args.effect)
        }
    }
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
            chain_effect(&args.state, operation, args.effect)
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
            chain_effect(&args.state, operation, args.effect)
        }
        RouteCommand::RemoveReceiveTimeout(args) => {
            let remote_eid = args.remote_eid;
            let operation = match args.vm.into() {
                Vm::Stellar => OperationV1::RemoveStellarReceiveLibraryTimeout { remote_eid },
                Vm::Evm => OperationV1::RemoveEvmReceiveLibraryTimeout { remote_eid },
            };
            chain_effect(&args.state, operation, args.effect)
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
            chain_effect(&args.state, operation, args.effect)
        }
    }
}

fn generic_authority(args: AuthorityArgs) -> Result<CommandData> {
    match args.command {
        AuthorityCommand::StellarBeginOwner(a) => chain_effect(
            &a.state,
            OperationV1::BeginStellarOwnershipTransfer {
                new_owner: a.new_owner,
                ttl: a.ttl,
            },
            a.effect,
        ),
        AuthorityCommand::StellarAcceptOwner(a) => {
            chain_effect(&a.state, OperationV1::AcceptStellarOwnership, a.effect)
        }
        AuthorityCommand::StellarCancelOwner(a) => chain_effect(
            &a.state,
            OperationV1::CancelStellarOwnershipTransfer,
            a.effect,
        ),
        AuthorityCommand::StellarSetDelegate(a) => chain_effect(
            &a.state,
            OperationV1::SetStellarDelegate {
                delegate: a.delegate,
            },
            a.effect,
        ),
        AuthorityCommand::EvmTransferOwner(a) => chain_effect(
            &a.state,
            OperationV1::TransferEvmOwnership {
                new_owner: a.new_owner,
            },
            a.effect,
        ),
        AuthorityCommand::EvmSetDelegate(a) => chain_effect(
            &a.state,
            OperationV1::SetEvmDelegate {
                delegate: a.delegate,
            },
            a.effect,
        ),
    }
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
            chain_effect(&a.state, operation, a.effect)
        }
        StellarCommand::SetFeeDepositAddress(a) => chain_effect(
            &a.state,
            OperationV1::SetFeeRecipient {
                recipient: a.recipient,
            },
            a.effect,
        ),
        StellarCommand::SetMessageInspector(a) => chain_effect(
            &a.state,
            OperationV1::SetMessageInspector {
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
            chain_effect(&a.state, operation, a.effect)
        }
        StellarCommand::TtlSet(a) => chain_effect(
            &a.state,
            OperationV1::SetTtlConfig {
                instance_threshold: a.instance_threshold,
                instance_extend_to: a.instance_extend_to,
                persistent_threshold: a.persistent_threshold,
                persistent_extend_to: a.persistent_extend_to,
            },
            a.effect,
        ),
        StellarCommand::TtlFreeze(a) => chain_effect(
            &a.state,
            OperationV1::FreezeTtlConfig {
                acknowledgement: "typed".into(),
            },
            a.effect,
        ),
        StellarCommand::TtlExtendInstance(a) => chain_effect(
            &a.state,
            OperationV1::ExtendInstanceTtl { ledgers: a.ledgers },
            a.effect,
        ),
        StellarCommand::EmergencyPause(a) => {
            chain_effect(&a.state, OperationV1::PauseEmergency, a.effect)
        }
        StellarCommand::EmergencyUnpause(a) => {
            chain_effect(&a.state, OperationV1::UnpauseEmergency, a.effect)
        }
        StellarCommand::RoleGrant(a) => chain_effect(
            &a.state,
            OperationV1::GrantRole {
                role: a.role,
                address: a.address,
            },
            a.effect,
        ),
        StellarCommand::RoleRevoke(a) => chain_effect(
            &a.state,
            OperationV1::RevokeRole {
                role: a.role,
                address: a.address,
            },
            a.effect,
        ),

        StellarCommand::RoleSetAdmin(a) => chain_effect(
            &a.state,
            OperationV1::SetRoleAdmin {
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
    chain_effect(&args.state, operation, args.effect)
}

fn contain(args: ContainArgs) -> Result<CommandData> {
    match args.command {
        ContainCommand::Inspect(args) => crate::layerzero::containment_status(&args.state),
        ContainCommand::Outbound(args) => chain_effect(
            &args.state,
            OperationV1::ContainOutbound {
                direction: args.direction.into(),
            },
            args.effect,
        ),
        ContainCommand::Restore(args) => chain_effect(
            &args.state,
            OperationV1::RestoreOutbound {
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
        ),
    }
}

fn message(args: MessageArgs) -> Result<CommandData> {
    match args.command {
        MessageCommand::Watch(args) => crate::canary::watch(&args.state, &args.guid),
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

fn chain_effect(
    state_path: &Path,
    operation: OperationV1,
    effect: ChainEffectArgs,
) -> Result<CommandData> {
    let store = RouteStore::open(state_path)?;
    let state = store.load_state()?;
    environment::require_testnet(&state.identity)?;
    if !effect.execute && effect.proposal_out.is_none() {
        return data(serde_json::json!({"preview": true, "operation": operation}));
    }
    if let Some(out) = effect.proposal_out {
        return crate::governance::proposal_for_operation(state_path, operation, &out);
    }
    Err(Error::Chain("native execution requires a qualified live adapter; use --proposal-out until qualification succeeds".into()))
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
