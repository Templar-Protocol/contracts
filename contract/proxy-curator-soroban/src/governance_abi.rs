use soroban_sdk::{contracterror, contracttype, Address, BytesN, String, Symbol, Vec};

#[contracttype]
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum TimelockKind {
    Admin,
    Pause,
    Curator,
    Governance,
    SupplyQueue,
    Fees,
    Restrictions,
    Sentinel,
    Allocators,
    AllowedAdapters,
    Cap,
    MarketRemoval,
    CapGroup,
    Skim,
    Upgrade,
    Migration,
    TimelockConfig,
    Other,
}

#[contracttype]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum GovernanceActionKind {
    Admin,
    Pause,
    Curator,
    Governance,
    SupplyQueue,
    Fees,
    Restrictions,
    Sentinel,
    Allocators,
    AllowedAdapters,
    Cap,
    MarketRemoval,
    CapGroup,
    Skim,
    Upgrade,
    Migrate,
    CancelMigration,
    TimelockConfig,
    Other,
    WithdrawalCooldown,
    IdleResyncCooldown,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct Timelocks {
    pub admin_ns: u64,
    pub pause_ns: u64,
    pub curator_ns: u64,
    pub governance_ns: u64,
    pub supply_queue_ns: u64,
    pub fees_ns: u64,
    pub restrictions_ns: u64,
    pub sentinel_ns: u64,
    pub allocators_ns: u64,
    pub allowed_adapters_ns: u64,
    pub cap_ns: u64,
    pub market_removal_ns: u64,
    pub cap_group_ns: u64,
    pub skim_ns: u64,
    pub upgrade_ns: u64,
    pub migration_ns: u64,
    pub timelock_config_ns: u64,
    pub other_ns: u64,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct FeeParams {
    pub performance_fee_wad: i128,
    pub performance_recipient: Address,
    pub management_fee_wad: i128,
    pub management_recipient: Address,
    pub max_growth_rate_wad: Option<i128>,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct SupplyQueueProposalEntry {
    pub target_id: u32,
    pub adapter: Address,
}

#[contracttype]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum RestrictionMode {
    None,
    Blacklist,
    Whitelist,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub enum GovernanceAction {
    SetAdmin(Address),
    SetPaused(bool),
    SetCurator(Address),
    SetGovernance(Address),
    SetSupplyQueue(Vec<SupplyQueueProposalEntry>),
    SetFees(FeeParams),
    SetRestrictions(RestrictionMode, Vec<Address>),
    SetSentinel(Address),
    SetAllocators(Vec<Address>),
    SetAllowedAdapters(Vec<Address>),
    SetCap(u32, i128),
    RemoveMarket(u32),
    SetGroupCap(String, i128),
    SetGroupRelCap(String, i128),
    SetGroupMember(u32, String),
    SetSkimRecipient(Address),
    Skim(Address),
    Upgrade(BytesN<32>),
    Migrate,
    CancelMigration,
    SetTimelock(TimelockKind, u64),
    Other(Symbol, BytesN<32>),
    SetWithdrawalCooldown(u64),
    SetIdleResyncCooldown(u64),
}

#[contracttype]
#[derive(Clone)]
pub struct PendingProposal {
    pub id: u64,
    pub action: GovernanceAction,
    pub valid_after_ns: u64,
}

#[contracterror]
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub enum GovernanceError {
    Unauthorized = 1,
    InvalidInput = 2,
    MissingConfig = 3,
    ProposalNotFound = 4,
    ProposalNotMature = 5,
    ArithmeticOverflow = 6,
    DuplicatePending = 7,
    NoChange = 8,
    TimelockOutOfBounds = 9,
    OtherNotApproved = 10,
    Abdicated = 11,
}
