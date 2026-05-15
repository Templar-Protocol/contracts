use soroban_sdk::{
    contracterror, contractevent, contracttype, Address, BytesN, String, Symbol, Vec,
};

#[contracttype]
#[derive(Clone)]
pub(crate) enum DataKey {
    Admin,
    Sentinel,
    Vault,
    TimelockNs,
    Timelocks,
    NextProposalId,
    PendingPageIndex,
    PendingPage(u64),
    ApprovedOther(Symbol, BytesN<32>),
    CurrentPaused,
    CurrentFees,
    CurrentRestrictionMode,
    CurrentRestrictionAccounts,
    CurrentCapGroupMembership(u32),
    Abdicated(GovernanceActionKind),
    SkimRecipient,
    CurrentCap(u32),
    CurrentCapGroupCap(String),
    CurrentCapGroupRelCap(String),
}

#[contracttype]
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum TimelockKind {
    Pause,
    Curator,
    Governance,
    SupplyQueue,
    Fees,
    Restrictions,
    Sentinel,
    Cap,
    MarketRemoval,
    CapGroup,
    Skim,
    TimelockConfig,
    Other,
}

#[contracttype]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum GovernanceActionKind {
    Pause,
    Curator,
    Governance,
    SupplyQueue,
    Fees,
    Restrictions,
    Sentinel,
    Cap,
    MarketRemoval,
    CapGroup,
    Skim,
    TimelockConfig,
    Other,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct Timelocks {
    pub pause_ns: u64,
    pub curator_ns: u64,
    pub governance_ns: u64,
    pub supply_queue_ns: u64,
    pub fees_ns: u64,
    pub restrictions_ns: u64,
    pub sentinel_ns: u64,
    pub cap_ns: u64,
    pub market_removal_ns: u64,
    pub cap_group_ns: u64,
    pub skim_ns: u64,
    pub timelock_config_ns: u64,
    pub other_ns: u64,
}

impl Timelocks {
    pub(crate) fn from_default(default_ns: u64) -> Self {
        Self {
            pause_ns: default_ns,
            curator_ns: default_ns,
            governance_ns: default_ns,
            supply_queue_ns: default_ns,
            fees_ns: default_ns,
            restrictions_ns: default_ns,
            sentinel_ns: default_ns,
            cap_ns: default_ns,
            market_removal_ns: default_ns,
            cap_group_ns: default_ns,
            skim_ns: default_ns,
            timelock_config_ns: default_ns,
            other_ns: default_ns,
        }
    }

    pub(crate) fn get(self, kind: TimelockKind) -> u64 {
        match kind {
            TimelockKind::Pause => self.pause_ns,
            TimelockKind::Curator => self.curator_ns,
            TimelockKind::Governance => self.governance_ns,
            TimelockKind::SupplyQueue => self.supply_queue_ns,
            TimelockKind::Fees => self.fees_ns,
            TimelockKind::Restrictions => self.restrictions_ns,
            TimelockKind::Sentinel => self.sentinel_ns,
            TimelockKind::Cap => self.cap_ns,
            TimelockKind::MarketRemoval => self.market_removal_ns,
            TimelockKind::CapGroup => self.cap_group_ns,
            TimelockKind::Skim => self.skim_ns,
            TimelockKind::TimelockConfig => self.timelock_config_ns,
            TimelockKind::Other => self.other_ns,
        }
    }

    pub(crate) fn set(&mut self, kind: TimelockKind, value: u64) {
        match kind {
            TimelockKind::Pause => self.pause_ns = value,
            TimelockKind::Curator => self.curator_ns = value,
            TimelockKind::Governance => self.governance_ns = value,
            TimelockKind::SupplyQueue => self.supply_queue_ns = value,
            TimelockKind::Fees => self.fees_ns = value,
            TimelockKind::Restrictions => self.restrictions_ns = value,
            TimelockKind::Sentinel => self.sentinel_ns = value,
            TimelockKind::Cap => self.cap_ns = value,
            TimelockKind::MarketRemoval => self.market_removal_ns = value,
            TimelockKind::CapGroup => self.cap_group_ns = value,
            TimelockKind::Skim => self.skim_ns = value,
            TimelockKind::TimelockConfig => self.timelock_config_ns = value,
            TimelockKind::Other => self.other_ns = value,
        }
    }
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
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum RestrictionMode {
    None,
    Blacklist,
    Whitelist,
}

impl RestrictionMode {
    pub(crate) fn from_u32(value: u32) -> Result<Self, GovernanceError> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Blacklist),
            2 => Ok(Self::Whitelist),
            _ => Err(GovernanceError::InvalidInput),
        }
    }

    pub(crate) fn as_u32(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Blacklist => 1,
            Self::Whitelist => 2,
        }
    }
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub enum GovernanceAction {
    SetPaused(bool),
    SetCurator(Address),
    SetGovernance(Address),
    SetSupplyQueue(Vec<u32>),
    SetFees(FeeParams),
    SetRestrictions(RestrictionMode, Vec<Address>),
    SetSentinel(Address),
    SetCap(u32, i128),
    RemoveMarket(u32),
    SetGroupCap(String, i128),
    SetGroupRelCap(String, i128),
    SetGroupMember(u32, String),
    SetSkimRecipient(Address),
    Skim(Address),
    SetTimelock(TimelockKind, u64),
    Other(Symbol, BytesN<32>),
}

#[contracttype]
#[derive(Clone)]
pub struct PendingProposal {
    pub id: u64,
    pub action: GovernanceAction,
    pub valid_after_ns: u64,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct QueuedProposal {
    pub id: u64,
    pub action: GovernanceAction,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct StoredPending {
    pub id: u64,
    pub action: GovernanceAction,
    pub valid_at_ns: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct ProposalSubmitted {
    #[topic]
    pub id: u64,
    pub valid_after_ns: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct ProposalAccepted {
    #[topic]
    pub id: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct ProposalRevoked {
    #[topic]
    pub id: u64,
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
