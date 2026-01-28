//! Chain-agnostic restrictions (gates) for vault access control.
//!
//! Portable across NEAR and Soroban.

use alloc::collections::BTreeSet;

#[cfg(feature = "near")]
use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "near")]
use serde::{Deserialize, Serialize};

use crate::types::ActorId;

/// Restrictions that can be applied to the vault.
///
/// Supports Pausing, Whitelist, and Blacklist functionality.
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restrictions {
    /// Vault is paused - all operations blocked.
    Paused,
    /// Blacklist - specified actors are blocked.
    BlackList(BTreeSet<ActorId>),
    /// Whitelist - only specified actors are allowed.
    WhiteList(BTreeSet<ActorId>),
}

impl Restrictions {
    /// Check if the given actor is restricted.
    ///
    /// Returns `Some(restriction)` if blocked, `None` if allowed.
    ///
    /// # Arguments
    /// * `actor_id` - The actor to check.
    /// * `self_id` - The vault's own identity (whitelist allows self by default).
    pub fn is_restricted(&self, actor_id: &ActorId, self_id: &ActorId) -> Option<Restrictions> {
        match self {
            Restrictions::Paused => Some(Restrictions::Paused),
            Restrictions::BlackList(blacklist) => {
                if blacklist.contains(actor_id) {
                    Some(Restrictions::BlackList(blacklist.clone()))
                } else {
                    None
                }
            }
            Restrictions::WhiteList(whitelist) => {
                if whitelist.contains(actor_id) || actor_id == self_id {
                    None
                } else {
                    Some(Restrictions::WhiteList(whitelist.clone()))
                }
            }
        }
    }

    /// Check if paused.
    #[inline]
    #[must_use]
    pub const fn is_paused(&self) -> bool {
        matches!(self, Restrictions::Paused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    #[test]
    fn test_paused_blocks_everyone() {
        let r = Restrictions::Paused;
        let actor = String::from("alice.near");
        let self_id = String::from("vault.near");
        assert!(r.is_restricted(&actor, &self_id).is_some());
        assert!(r.is_restricted(&self_id, &self_id).is_some());
    }

    #[test]
    fn test_blacklist_blocks_listed() {
        let mut blacklist = BTreeSet::new();
        blacklist.insert(String::from("bad.near"));
        let r = Restrictions::BlackList(blacklist);

        let self_id = String::from("vault.near");
        assert!(r.is_restricted(&String::from("bad.near"), &self_id).is_some());
        assert!(r.is_restricted(&String::from("good.near"), &self_id).is_none());
    }

    #[test]
    fn test_whitelist_allows_listed_and_self() {
        let mut whitelist = BTreeSet::new();
        whitelist.insert(String::from("alice.near"));
        let r = Restrictions::WhiteList(whitelist);

        let self_id = String::from("vault.near");
        assert!(r.is_restricted(&String::from("alice.near"), &self_id).is_none());
        assert!(r.is_restricted(&self_id, &self_id).is_none());
        assert!(r.is_restricted(&String::from("bob.near"), &self_id).is_some());
    }
}
