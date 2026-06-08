use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::types::{
    AddressStr, GovernanceActionKindArg, RestrictionModeArg, SourceAccount, SupplyQueueEntryArg,
    TimelockKindArg, WasmHash,
};

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Deploy and operate Templar Soroban vault stacks",
    long_about = "Deploy Templar Soroban vault contracts, reuse previously uploaded WASM where possible, and run typed user, curator, governance, share-token, and Blend adapter operations through the Stellar CLI."
)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "CLI flags are independent switches"
)]
pub struct Cli {
    /// Stellar network name from the local stellar config
    #[arg(long, env = "SOROBAN_NETWORK", default_value = "testnet")]
    pub network: String,

    /// RPC URL override
    #[arg(long, env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,

    /// Network passphrase override
    #[arg(
        long,
        env = "SOROBAN_NETWORK_PASSPHRASE",
        default_value = "Test SDF Network ; September 2015"
    )]
    pub network_passphrase: String,

    /// Stellar source account identity, public key, secret key, or seed phrase
    #[arg(long, env = "SOROBAN_IDENTITY", default_value = "templar")]
    pub source_account: SourceAccount,

    /// Stellar config directory
    #[arg(long, env = "STELLAR_CONFIG_DIR")]
    pub config_dir: Option<PathBuf>,

    /// Deployment manifest path
    #[arg(
        long,
        env = "TEMPLAR_SOROBAN_VAULT_STATE",
        default_value = "contract/vault/soroban/.deploy-state/manifest.json"
    )]
    pub state: PathBuf,

    /// Path to the workspace root
    #[arg(long, env = "WORKSPACE_PATH", default_value = ".")]
    pub workspace_path: PathBuf,

    /// Output machine-readable JSON
    #[arg(long)]
    pub json: bool,

    /// Print commands and manifest decisions without running writes
    #[arg(long)]
    pub dry_run: bool,

    /// Confirm overwrite or guarded production actions
    #[arg(long)]
    pub yes: bool,

    /// Permit mainnet write commands
    #[arg(long)]
    pub allow_mainnet_write: bool,

    /// Permit deploying governance with a zero timelock
    #[arg(long)]
    pub allow_zero_timelock: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Deploy, verify, or reuse vault stack components
    Deploy(DeployArgs),
    /// User-facing vault/share operations
    User(UserArgs),
    /// Curator and allocator operations
    Curator(CuratorArgs),
    /// Governance proposal operations
    Governance(GovernanceArgs),
    /// Share token operations and views
    ShareToken(ShareTokenArgs),
    /// Adapter operations and views
    Adapter(AdapterArgs),
    /// Extend TTL for every TTL-capable contract recorded in the deployment manifest
    ExtendTtl(ExtendTtlArgs),
    /// Print deployment status from the manifest
    Status,
    /// Export manifest values as shell environment assignments
    ExportEnv,
}

impl Commands {
    pub fn is_write(&self) -> bool {
        match self {
            Self::Status | Self::ExportEnv => false,
            Self::Deploy(_) | Self::ExtendTtl(_) => true,
            Self::User(args) => args.command.is_write(),
            Self::Curator(args) => args.command.is_write(),
            Self::Governance(args) => args.command.is_write(),
            Self::ShareToken(args) => args.command.is_write(),
            Self::Adapter(args) => args.command.is_write(),
        }
    }
}

#[derive(Args, Debug)]
pub struct ExtendTtlArgs {
    /// Caller/admin address for TTL entrypoints that require authorization. Defaults to `stellar keys address <source-account>`.
    #[arg(long, env = "SOROBAN_TTL_CALLER")]
    pub caller: Option<AddressStr>,
}

#[derive(Args, Debug)]
pub struct DeployArgs {
    #[command(subcommand)]
    pub command: DeployCommand,
}

#[derive(Subcommand, Debug)]
pub enum DeployCommand {
    /// Deploy or reuse a full vault stack
    Stack(Box<DeployStackArgs>),
    /// Add Blend adapters to an existing or imported vault deployment
    Adapters(DeployAdaptersArgs),
    /// Upload or verify a single WASM artifact
    Wasm(DeployWasmArgs),
}

#[derive(Args, Debug)]
pub struct DeployStackArgs {
    /// Admin/curator Soroban address. Defaults to `stellar keys address <source-account>`.
    #[arg(long, env = "SOROBAN_ADMIN")]
    pub admin: Option<AddressStr>,

    /// Existing asset token contract address. If omitted, native asset SAC is deployed/resolved.
    #[arg(long, env = "SOROBAN_ASSET_TOKEN")]
    pub asset_token: Option<AddressStr>,

    /// Governance timelock in nanoseconds for a new governance contract
    #[arg(long, env = "SOROBAN_GOV_TIMELOCK_NS")]
    pub governance_timelock_ns: Option<u64>,

    /// Initial virtual shares offset
    #[arg(long, env = "SOROBAN_VIRTUAL_SHARES", default_value_t = 0)]
    pub virtual_shares: i128,

    /// Initial virtual assets offset
    #[arg(long, env = "SOROBAN_VIRTUAL_ASSETS", default_value_t = 0)]
    pub virtual_assets: i128,

    /// Share token display name
    #[arg(
        long,
        env = "SOROBAN_SHARE_NAME",
        default_value = "Templar Vault Share"
    )]
    pub share_name: String,

    /// Share token symbol
    #[arg(long, env = "SOROBAN_SHARE_SYMBOL", default_value = "tvSHARE")]
    pub share_symbol: String,

    /// Share token decimals
    #[arg(long, env = "SOROBAN_SHARE_DECIMALS", default_value_t = 7)]
    pub share_decimals: u32,

    /// Blend pool contract address. Repeat the flag, or provide comma-separated BLEND_POOL_ID values, to deploy multiple Blend adapters.
    #[arg(long = "blend-pool", env = "BLEND_POOL_ID", value_delimiter = ',')]
    pub blend_pools: Vec<AddressStr>,

    /// Rebuild missing artifacts before upload/deploy
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub build: bool,

    /// Deploy fresh contract instances instead of reusing manifest ids
    #[arg(long)]
    pub force_new: bool,
}

#[derive(Args, Debug)]
pub struct DeployAdaptersArgs {
    /// Existing vault contract address. Required when the manifest does not already contain `vault`.
    #[arg(long, env = "SOROBAN_VAULT")]
    pub vault: Option<AddressStr>,

    /// Existing governance contract address. Required when the manifest does not already contain `governance`.
    #[arg(long, env = "SOROBAN_GOVERNANCE")]
    pub governance: Option<AddressStr>,

    /// Existing asset token contract address to record for imported deployments.
    #[arg(long, env = "SOROBAN_ASSET_TOKEN")]
    pub asset_token: Option<AddressStr>,

    /// Blend pool contract address. Repeat the flag, or provide comma-separated BLEND_POOL_ID values, to append multiple adapters.
    #[arg(long = "blend-pool", env = "BLEND_POOL_ID", value_delimiter = ',')]
    pub blend_pools: Vec<AddressStr>,

    /// Rebuild the Blend adapter artifact before upload/deploy
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub build: bool,

    /// Deploy a fresh adapter even if an adapter for the same pool already exists
    #[arg(long)]
    pub force_new: bool,
}

#[derive(Args, Debug)]
pub struct DeployWasmArgs {
    /// Known artifact name
    #[arg(value_enum)]
    pub artifact: ArtifactName,

    /// Rebuild the artifact before upload/verification
    #[arg(long)]
    pub build: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, ValueEnum)]
pub enum ArtifactName {
    Vault,
    Governance,
    ShareToken,
    BlendAdapter,
    Proxy4626,
    CuratorProxy,
}

#[derive(Args, Debug)]
pub struct UserArgs {
    #[command(subcommand)]
    pub command: UserCommand,
}

#[derive(Subcommand, Debug)]
pub enum UserCommand {
    /// Deposit assets through the ERC-4626 proxy when deployed, otherwise through the vault command payload.
    Deposit {
        /// Authorized operator address spending the assets.
        #[arg(long)]
        operator: AddressStr,
        /// Share receiver address. Defaults to --operator.
        #[arg(long)]
        receiver: Option<AddressStr>,
        /// Asset amount in contract base units.
        #[arg(long)]
        assets: i128,
        /// Minimum shares accepted from the deposit.
        #[arg(long, default_value_t = 0)]
        min_shares_out: i128,
    },
    /// Mint an exact number of shares through the ERC-4626 proxy.
    Mint {
        /// Authorized operator address spending the assets.
        #[arg(long)]
        operator: AddressStr,
        /// Share receiver address. Defaults to --operator.
        #[arg(long)]
        receiver: Option<AddressStr>,
        /// Share amount in share-token base units.
        #[arg(long)]
        shares: i128,
    },
    /// Withdraw an exact asset amount through the ERC-4626 proxy.
    Withdraw {
        /// Authorized operator address burning shares.
        #[arg(long)]
        operator: AddressStr,
        /// Asset receiver address. Defaults to --operator.
        #[arg(long)]
        receiver: Option<AddressStr>,
        /// Share owner address. Defaults to --operator.
        #[arg(long)]
        owner: Option<AddressStr>,
        /// Asset amount in contract base units.
        #[arg(long)]
        assets: i128,
        /// Maximum shares allowed to burn. Defaults to --assets for operator workflows.
        #[arg(long)]
        max_shares_burned: Option<i128>,
    },
    /// Redeem an exact share amount through the ERC-4626 proxy.
    Redeem {
        /// Authorized operator address burning shares.
        #[arg(long)]
        operator: AddressStr,
        /// Asset receiver address. Defaults to --operator.
        #[arg(long)]
        receiver: Option<AddressStr>,
        /// Share owner address. Defaults to --operator.
        #[arg(long)]
        owner: Option<AddressStr>,
        /// Share amount in share-token base units.
        #[arg(long)]
        shares: i128,
        /// Minimum assets accepted from the redeem.
        #[arg(long, default_value_t = 0)]
        min_assets_out: i128,
    },
    /// Queue a delayed withdrawal request directly against the vault.
    RequestWithdraw {
        /// Share owner address.
        #[arg(long)]
        owner: AddressStr,
        /// Asset receiver address. Defaults to --owner.
        #[arg(long)]
        receiver: Option<AddressStr>,
        /// Share amount to burn after cooldown.
        #[arg(long)]
        shares: i128,
        /// Minimum assets accepted when the withdrawal executes.
        #[arg(long, default_value_t = 0)]
        min_assets_out: i128,
    },
    /// Execute the caller's pending withdrawal through the proxy when available.
    ExecuteWithdraw {
        /// Address whose pending withdrawal should execute.
        #[arg(long)]
        operator: AddressStr,
    },
    /// Read share-token balance for an owner.
    Balance {
        #[arg(long)]
        owner: AddressStr,
    },
    /// Preview vault/proxy conversion and limit values for an owner.
    Preview {
        #[arg(long)]
        owner: AddressStr,
        #[arg(long, default_value_t = 0)]
        assets: i128,
        #[arg(long, default_value_t = 0)]
        shares: i128,
    },
    /// Alias for preview that preserves the underlying vault view naming.
    View {
        #[arg(long)]
        owner: AddressStr,
        #[arg(long, default_value_t = 0)]
        assets: i128,
        #[arg(long, default_value_t = 0)]
        shares: i128,
    },
}

impl UserCommand {
    pub fn is_write(&self) -> bool {
        !matches!(
            self,
            Self::Balance { .. } | Self::Preview { .. } | Self::View { .. }
        )
    }
}

#[derive(Args, Debug)]
pub struct CuratorArgs {
    #[command(subcommand)]
    pub command: CuratorCommand,
}

#[derive(Subcommand, Debug)]
pub enum CuratorCommand {
    /// Allocate a positive or negative supply delta to a market through the vault command payload.
    AllocateSupply {
        #[arg(long)]
        caller: AddressStr,
        #[arg(long)]
        market: u32,
        #[arg(long)]
        amount: i128,
    },
    /// Allocate a positive or negative withdrawal delta to a market through the vault command payload.
    AllocateWithdraw {
        #[arg(long)]
        caller: AddressStr,
        #[arg(long)]
        market: u32,
        #[arg(long)]
        amount: i128,
    },
    /// Refresh one or more market ids through the vault command payload.
    RefreshMarkets {
        #[arg(long)]
        caller: AddressStr,
        #[arg(long, value_delimiter = ',')]
        markets: Vec<u32>,
    },
    /// Refresh fee accounting through the vault command payload.
    RefreshFees,
    /// Resynchronize idle asset accounting through the vault command payload.
    ResyncIdle,
    /// Submit a governance proposal for the allowed adapter address set.
    SetAllowedAdapters {
        #[arg(long)]
        admin: AddressStr,
        #[arg(long, value_delimiter = ',')]
        adapters: Vec<AddressStr>,
        #[arg(long)]
        auto_accept: bool,
    },
    /// Submit a governance proposal for the supply queue using typed `SupplyQueueProposalEntry` values.
    SetSupplyQueue {
        #[arg(long)]
        admin: AddressStr,
        /// Supply queue entry formatted as `target_id:adapter_address`. Repeat for each queue item.
        #[arg(long = "entry")]
        entries: Vec<SupplyQueueEntryArg>,
        #[arg(long)]
        auto_accept: bool,
    },
}

impl CuratorCommand {
    pub fn is_write(&self) -> bool {
        true
    }
}

#[derive(Args, Debug)]
pub struct GovernanceArgs {
    #[command(subcommand)]
    pub command: GovernanceCommand,
}

#[derive(Subcommand, Debug)]
pub enum GovernanceCommand {
    /// Accept a pending proposal after its timelock has elapsed.
    Accept {
        /// Governance admin address accepting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Pending proposal id to accept.
        #[arg(long)]
        proposal_id: u64,
    },
    /// Revoke a pending proposal before acceptance.
    Revoke {
        /// Governance admin address revoking the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Pending proposal id to revoke.
        #[arg(long)]
        proposal_id: u64,
    },
    /// Print one proposal or list pending proposal ids.
    Pending {
        /// Optional proposal id to inspect. Omit to list pending ids.
        #[arg(long)]
        proposal_id: Option<u64>,
    },
    /// Print the current governance timelock configuration.
    Timelocks,
    /// Submit a proposal to change the governance admin address.
    SubmitSetAdmin {
        /// Current governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed replacement governance admin address.
        #[arg(long)]
        new_admin: AddressStr,
    },
    /// Submit a proposal to change the vault curator address.
    SubmitSetCurator {
        /// Current governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed replacement vault curator address.
        #[arg(long)]
        new_curator: AddressStr,
    },
    /// Submit a proposal to change the vault governance address.
    SubmitSetGovernance {
        /// Current governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed replacement governance contract address.
        #[arg(long)]
        new_governance: AddressStr,
    },
    /// Submit a proposal to pause or unpause the vault.
    SubmitSetPaused {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed paused state.
        #[arg(long)]
        paused: bool,
    },
    /// Submit a proposal to replace the vault supply queue with typed target/adapter entries.
    SubmitSetSupplyQueue {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Supply queue entry formatted as `target_id:adapter_address`. Repeat for each queue item.
        #[arg(long = "entry")]
        entries: Vec<SupplyQueueEntryArg>,
    },
    /// Submit a proposal to update fee parameters.
    SubmitSetFees {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Performance fee as a WAD-scaled integer.
        #[arg(long)]
        performance_fee_wad: i128,
        /// Address that receives performance fees.
        #[arg(long)]
        performance_recipient: AddressStr,
        /// Management fee as a WAD-scaled integer.
        #[arg(long)]
        management_fee_wad: i128,
        /// Address that receives management fees.
        #[arg(long)]
        management_recipient: AddressStr,
        /// Optional maximum fee growth rate as a WAD-scaled integer.
        #[arg(long)]
        max_growth_rate_wad: Option<i128>,
    },
    /// Submit a proposal to update transfer restrictions.
    SubmitSetRestrictions {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Restriction mode: none, blacklist, or whitelist.
        #[arg(long)]
        mode: RestrictionModeArg,
        /// Restricted or allowed account addresses, comma-separated or repeated.
        #[arg(long, value_delimiter = ',')]
        accounts: Vec<AddressStr>,
    },
    /// Submit a proposal to update the sentinel address.
    SubmitSetSentinel {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed sentinel address.
        #[arg(long)]
        sentinel: AddressStr,
    },
    /// Submit a proposal to replace allocator addresses.
    SubmitSetAllocators {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Allocator addresses, comma-separated or repeated.
        #[arg(long, value_delimiter = ',')]
        allocators: Vec<AddressStr>,
    },
    /// Submit a proposal to replace allowed adapter addresses.
    SubmitSetAllowedAdapters {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Allowed adapter addresses, comma-separated or repeated.
        #[arg(long, value_delimiter = ',')]
        adapters: Vec<AddressStr>,
    },
    /// Submit a proposal to update one governance timelock kind.
    SubmitSetTimelock {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Timelock kind from the governance contract, for example supply-queue or migration.
        #[arg(long)]
        kind: TimelockKindArg,
        /// Proposed timelock duration in nanoseconds.
        #[arg(long)]
        timelock_ns: u64,
    },
    /// Submit a proposal to update an absolute market cap.
    SubmitSetCap {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Market id whose cap should change.
        #[arg(long)]
        market_id: u32,
        /// Proposed absolute cap in asset base units.
        #[arg(long)]
        cap: i128,
    },
    /// Submit a proposal to remove a market from the vault.
    SubmitRemoveMarket {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Market id to remove.
        #[arg(long)]
        market_id: u32,
    },
    /// Submit a proposal to update an absolute cap for a cap group.
    SubmitSetGroupCap {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Cap group identifier.
        #[arg(long)]
        group: String,
        /// Proposed group absolute cap in asset base units.
        #[arg(long)]
        cap: i128,
    },
    /// Submit a proposal to update a relative cap for a cap group.
    SubmitSetGroupRelCap {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Cap group identifier.
        #[arg(long)]
        group: String,
        /// Proposed relative cap as a WAD-scaled integer.
        #[arg(long)]
        relative_cap: i128,
    },
    /// Submit a proposal to assign a market to a cap group.
    SubmitSetGroupMember {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Market id whose group should change.
        #[arg(long)]
        market_id: u32,
        /// Cap group identifier to assign.
        #[arg(long)]
        group: String,
    },
    /// Submit a proposal to update the skim recipient.
    SubmitSetSkimRecipient {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed skim recipient address.
        #[arg(long)]
        recipient: AddressStr,
    },
    /// Submit a proposal to skim a token.
    SubmitSkim {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Token contract address to skim.
        #[arg(long)]
        token: AddressStr,
    },
    /// Submit a proposal to update the withdrawal cooldown.
    SubmitSetWithdrawalCooldown {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed withdrawal cooldown in nanoseconds.
        #[arg(long)]
        cooldown_ns: u64,
    },
    /// Submit a proposal to update the idle resync cooldown.
    SubmitSetIdleResyncCooldown {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed idle resync cooldown in nanoseconds.
        #[arg(long)]
        cooldown_ns: u64,
    },
    /// Submit a proposal to upgrade the vault WASM.
    SubmitUpgrade {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed 32-byte vault WASM hash as hex.
        #[arg(long)]
        wasm_hash: WasmHash,
    },
    /// Submit the governance-controlled migration action.
    SubmitMigrate {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
    },
    /// Submit the governance-controlled cancel-migration action.
    SubmitCancelMigration {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
    },
    /// Permanently abdicate one governance action kind.
    Abdicate {
        /// Governance admin address abdicating the action kind.
        #[arg(long)]
        admin: AddressStr,
        /// Governance action kind from the contract enum.
        #[arg(long)]
        kind: GovernanceActionKindArg,
    },
}

impl GovernanceCommand {
    pub fn is_write(&self) -> bool {
        !matches!(self, Self::Pending { .. } | Self::Timelocks)
    }
}

#[derive(Args, Debug)]
pub struct ShareTokenArgs {
    #[command(subcommand)]
    pub command: ShareTokenCommand,
}

#[derive(Subcommand, Debug)]
pub enum ShareTokenCommand {
    /// Read share balance for an account.
    Balance {
        #[arg(long)]
        account: AddressStr,
    },
    /// Read total share supply.
    TotalSupply,
    /// Read share-token admin.
    Admin,
    /// Read configured vault address.
    Vault,
    /// Extend share-token instance TTL.
    ExtendTtl {
        #[arg(long)]
        caller: AddressStr,
    },
}

impl ShareTokenCommand {
    pub fn is_write(&self) -> bool {
        matches!(self, Self::ExtendTtl { .. })
    }
}

#[derive(Args, Debug)]
pub struct AdapterArgs {
    /// Indexed Blend adapter to operate on, matching deploy order for repeated --blend-pool flags.
    #[arg(long, default_value_t = 0)]
    pub adapter_index: usize,

    /// Explicit manifest contract key, such as blend_adapter_1.
    #[arg(long)]
    pub adapter_key: Option<String>,

    /// Select the adapter whose constructor pool matches this address.
    #[arg(long)]
    pub adapter_pool: Option<AddressStr>,

    #[command(subcommand)]
    pub command: AdapterCommand,
}

#[derive(Subcommand, Debug)]
pub enum AdapterCommand {
    /// Read adapter total assets for a token address.
    TotalAssets {
        #[arg(long)]
        asset: AddressStr,
    },
    /// Read adapter admin.
    Admin,
    /// Read configured vault address.
    Vault,
    /// Read configured Blend pool address.
    Pool,
    /// Set a pending adapter admin.
    SetAdmin {
        #[arg(long)]
        caller: AddressStr,
        #[arg(long)]
        admin: AddressStr,
    },
    /// Accept a pending adapter admin handoff.
    AcceptAdmin {
        #[arg(long)]
        caller: AddressStr,
    },
    /// Extend adapter instance TTL.
    ExtendTtl {
        #[arg(long)]
        caller: AddressStr,
    },
}

impl AdapterCommand {
    pub fn is_write(&self) -> bool {
        !matches!(
            self,
            Self::TotalAssets { .. } | Self::Admin | Self::Vault | Self::Pool
        )
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Commands, DeployCommand};

    const ADMIN: &str = "GBRFSXJNPLMYJV7EBFTBZT2PU6KN5WWPX3UKHDAAQQT7BNS7QTFCS3AY";
    const POOL: &str = "CDY3B7IXFN5L4OY4UFFS2FA4MAQWJZLJD76LW37S7HFVWRS3RPQ2SIXX";

    #[test]
    fn parses_deploy_stack_flags() {
        let cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "--source-account",
            "alice",
            "deploy",
            "stack",
            "--admin",
            ADMIN,
            "--governance-timelock-ns",
            "1000",
            "--blend-pool",
            POOL,
            "--blend-pool",
            POOL,
        ])
        .expect("parse cli");

        match cli.command {
            Commands::Deploy(args) => match args.command {
                DeployCommand::Stack(stack) => {
                    assert_eq!(
                        stack.admin.as_ref().map(ToString::to_string).as_deref(),
                        Some(ADMIN)
                    );
                    assert_eq!(stack.governance_timelock_ns, Some(1000));
                    assert_eq!(stack.blend_pools.len(), 2);
                }
                DeployCommand::Adapters(_) | DeployCommand::Wasm(_) => {
                    panic!("expected deploy stack")
                }
            },
            _ => panic!("expected deploy command"),
        }
    }

    #[test]
    fn parses_additive_adapter_deploy_flags() {
        let cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "deploy",
            "adapters",
            "--vault",
            POOL,
            "--governance",
            POOL,
            "--asset-token",
            POOL,
            "--blend-pool",
            POOL,
        ])
        .expect("parse cli");

        match cli.command {
            Commands::Deploy(args) => match args.command {
                DeployCommand::Adapters(args) => {
                    assert_eq!(
                        args.vault.as_ref().map(ToString::to_string).as_deref(),
                        Some(POOL)
                    );
                    assert_eq!(args.blend_pools.len(), 1);
                }
                DeployCommand::Stack(_) | DeployCommand::Wasm(_) => {
                    panic!("expected deploy adapters")
                }
            },
            _ => panic!("expected deploy command"),
        }
    }

    #[test]
    fn parses_deployment_extend_ttl_command() {
        let cli = Cli::try_parse_from(["tmplr-soroban-vault", "extend-ttl", "--caller", ADMIN])
            .expect("parse cli");

        assert!(matches!(cli.command, Commands::ExtendTtl(_)));
    }

    #[test]
    fn rejects_invalid_soroban_addresses() {
        let err = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "user",
            "balance",
            "--owner",
            "not-an-address",
        ])
        .expect_err("invalid address should fail");

        assert!(err.to_string().contains("invalid Soroban"));
    }

    #[test]
    fn rejects_free_form_governance_action_kind() {
        let err = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "governance",
            "abdicate",
            "--admin",
            ADMIN,
            "--kind",
            "whatever",
        ])
        .expect_err("unknown governance kind should fail");

        assert!(err.to_string().contains("unknown governance action kind"));
    }

    #[test]
    fn parses_typed_governance_timelock_and_restriction_commands() {
        let timelock_cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "governance",
            "submit-set-timelock",
            "--admin",
            ADMIN,
            "--kind",
            "supply-queue",
            "--timelock-ns",
            "1000",
        ])
        .expect("parse timelock command");
        assert!(matches!(timelock_cli.command, Commands::Governance(_)));

        let restrictions_cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "governance",
            "submit-set-restrictions",
            "--admin",
            ADMIN,
            "--mode",
            "whitelist",
            "--accounts",
            ADMIN,
        ])
        .expect("parse restrictions command");
        assert!(matches!(restrictions_cli.command, Commands::Governance(_)));
    }
}
