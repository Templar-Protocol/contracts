use near_sdk::near;
use templar_vault_kernel::{Address, RestrictionMode};

pub use templar_vault_kernel::RestrictionKind as RestrictionReason;

#[near(serializers = [json, borsh])]
#[derive(Clone, PartialEq, Eq)]
pub enum Restrictions {
    Paused,
    Blacklist(Vec<Address>),
    Whitelist(Vec<Address>),
}

impl Restrictions {
    #[must_use]
    pub fn to_kernel_mode(&self) -> Option<RestrictionMode> {
        match self {
            Self::Paused => None,
            Self::Blacklist(addresses) => Some(RestrictionMode::blacklist(addresses.clone())),
            Self::Whitelist(addresses) => Some(RestrictionMode::whitelist(addresses.clone())),
        }
    }
}

impl From<RestrictionMode> for Restrictions {
    fn from(value: RestrictionMode) -> Self {
        match value {
            RestrictionMode::Blacklist(addresses) => Self::Blacklist(addresses),
            RestrictionMode::Whitelist(addresses) => Self::Whitelist(addresses),
        }
    }
}
