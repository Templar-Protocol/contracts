use super::*;

/// Lightweight tag indicating why an account was restricted.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestrictionReason {
    Paused,
    Blacklisted,
    NotWhitelisted,
}

/// Access control restrictions for the vault. Exactly one variant is active at a time. `Paused` blocks all user operations. `Blacklist` blocks listed accounts. `Whitelist` allows only listed accounts (plus the vault's own account).
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restrictions {
    Paused,
    #[serde(rename = "BlackList")]
    Blacklist(BTreeSet<AccountId>),
    #[serde(rename = "WhiteList")]
    Whitelist(BTreeSet<AccountId>),
}

impl Restrictions {
    /// Check if the account is restricted, and if so, what is the reason.
    pub fn is_restricted(&self, account_id: &AccountIdRef) -> Option<RestrictionReason> {
        match self {
            Restrictions::Paused => Some(RestrictionReason::Paused),
            Restrictions::Blacklist(blacklist) => {
                if blacklist.contains(account_id) {
                    Some(RestrictionReason::Blacklisted)
                } else {
                    None
                }
            }
            Restrictions::Whitelist(whitelist) => {
                if whitelist.contains(account_id) || account_id == env::current_account_id() {
                    None
                } else {
                    Some(RestrictionReason::NotWhitelisted)
                }
            }
        }
    }
}
