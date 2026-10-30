//! Storage layout for the governance contract.
//!
//! - `ProxyOracle` (instance): the target proxy-oracle address.
//! - `Header` (instance): the governance kernel's small ledger — `next_id`,
//!   the pending id set, the TTL table, and the pending cap. Cheap to rewrite
//!   on every governance action.
//! - `ProposalGroup(slot)` (persistent): eight fixed slots holding at most
//!   eight unchanged proposal bodies each. Slot selection is independent of
//!   historical IDs, so churn cannot enlarge queue-wide revocation's footprint.
//! - `PendingBytes` (instance): sum of the XDR sizes of active proposal
//!   values, bounded to keep atomic revocation within transaction limits.
//!
//! This fresh-stack layout does not decode the previous per-body layout.
//!
//! Role membership lives in `stellar-access` (see `roles`), not here.

use soroban_sdk::{contracttype, xdr::ToXdr, Env, Map, Vec};
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_soroban_common::{DEFAULT_TTL_EXTEND_TO, DEFAULT_TTL_THRESHOLD};
use templar_proxy_oracle_soroban_governance_common::{
    GovernanceAction, GovernanceError, Proposal, TtlConfig,
};

pub type KernelGovernance = templar_proxy_oracle_governance_kernel::Governance<TtlConfig>;
pub type KernelProposal =
    templar_proxy_oracle_governance_kernel::Proposal<GovernanceAction, soroban_sdk::Address>;

pub const PROPOSALS_PER_GROUP: u64 = 8;
pub const PROPOSAL_GROUP_COUNT: u64 = 8;

// Leave room below the 132,096-byte transaction write limit for group keys,
// map entries, ledger envelopes, the instance header, and role mutations.
// Reserve space within the payload budget for an Admin recovery proposal.
pub const MAX_PENDING_PROPOSAL_BYTES: u32 = 120 * 1024;
pub const ADMIN_PROPOSAL_BYTE_RESERVE: u32 = 2 * 1024;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    ProxyOracle,
    Header,
    ProposalGroup(u64),
    PendingBytes,
}

/// Soroban-native mirror of the kernel ledger. Only the `active_ids` vector
/// needs converting between `soroban_sdk::Vec` and the kernel's `alloc::Vec`;
/// every other field is shared verbatim.
#[contracttype]
#[derive(Clone)]
pub struct StoredHeader {
    pub next_id: u64,
    pub active_ids: Vec<u64>,
    pub ttls: TtlConfig,
    pub max_pending_proposals: u32,
}

pub fn save_header(env: &Env, header: &KernelGovernance) {
    let mut active_ids = Vec::new(env);
    for id in header.active_ids() {
        active_ids.push_back(*id);
    }
    let stored = StoredHeader {
        next_id: header.next_id(),
        active_ids,
        ttls: header.ttls().clone(),
        max_pending_proposals: header.max_pending_proposals(),
    };
    env.storage().instance().set(&DataKey::Header, &stored);
}

pub fn load_header(env: &Env) -> Result<KernelGovernance, GovernanceError> {
    let stored: StoredHeader = env
        .storage()
        .instance()
        .get(&DataKey::Header)
        .ok_or(GovernanceError::MissingConfig)?;
    KernelGovernance::try_from_parts(
        stored.next_id,
        stored.active_ids.iter().collect(),
        stored.ttls,
        stored.max_pending_proposals,
    )
    .map_err(|_| GovernanceError::InvalidInput)
}

pub fn proposal_bytes(env: &Env, proposal: &Proposal) -> u32 {
    proposal.clone().to_xdr(env).len()
}

pub fn pending_bytes(env: &Env) -> Result<u32, GovernanceError> {
    env.storage()
        .instance()
        .get(&DataKey::PendingBytes)
        .ok_or(GovernanceError::MissingConfig)
}

pub fn save_pending_bytes(env: &Env, bytes: u32) {
    env.storage().instance().set(&DataKey::PendingBytes, &bytes);
}

pub fn load_proposal(env: &Env, id: u64) -> Option<Proposal> {
    for slot in 0..PROPOSAL_GROUP_COUNT {
        let key = DataKey::ProposalGroup(slot);
        let group: Option<Map<u64, Proposal>> = env.storage().persistent().get(&key);
        if let Some(proposal) = group.and_then(|group| group.get(id)) {
            env.storage().persistent().extend_ttl(
                &key,
                DEFAULT_TTL_THRESHOLD,
                DEFAULT_TTL_EXTEND_TO,
            );
            return Some(proposal);
        }
    }
    None
}

pub fn extend_active_proposal_ttls(env: &Env) {
    let storage = env.storage().persistent();
    for slot in 0..PROPOSAL_GROUP_COUNT {
        let key = DataKey::ProposalGroup(slot);
        if storage.has(&key) {
            storage.extend_ttl(&key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
        }
    }
}

pub fn save_proposal(
    env: &Env,
    id: u64,
    proposal: &Proposal,
    is_admin: bool,
) -> Result<(), GovernanceError> {
    let bytes = pending_bytes(env)?
        .checked_add(proposal_bytes(env, proposal))
        .ok_or(GovernanceError::PendingProposalBytesExceeded)?;
    let limit = if is_admin {
        MAX_PENDING_PROPOSAL_BYTES
    } else {
        MAX_PENDING_PROPOSAL_BYTES - ADMIN_PROPOSAL_BYTE_RESERVE
    };
    if bytes > limit {
        return Err(GovernanceError::PendingProposalBytesExceeded);
    }
    for slot in 0..PROPOSAL_GROUP_COUNT {
        let key = DataKey::ProposalGroup(slot);
        let mut group: Map<u64, Proposal> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Map::new(env));
        if u64::from(group.len()) < PROPOSALS_PER_GROUP {
            group.set(id, proposal.clone());
            env.storage().persistent().set(&key, &group);
            env.storage().persistent().extend_ttl(
                &key,
                DEFAULT_TTL_THRESHOLD,
                DEFAULT_TTL_EXTEND_TO,
            );
            save_pending_bytes(env, bytes);
            return Ok(());
        }
    }
    // The kernel's 64-entry cap guarantees space for an admitted new body.
    Err(GovernanceError::InvalidInput)
}

pub fn remove_proposal(env: &Env, id: u64) -> Result<(), GovernanceError> {
    for slot in 0..PROPOSAL_GROUP_COUNT {
        let key = DataKey::ProposalGroup(slot);
        let Some(mut group): Option<Map<u64, Proposal>> = env.storage().persistent().get(&key)
        else {
            continue;
        };
        let Some(proposal) = group.get(id) else {
            continue;
        };
        let bytes = pending_bytes(env)?
            .checked_sub(proposal_bytes(env, &proposal))
            .ok_or(GovernanceError::InvalidInput)?;
        group.remove_unchecked(id);
        if group.is_empty() {
            env.storage().persistent().remove(&key);
        } else {
            env.storage().persistent().set(&key, &group);
        }
        save_pending_bytes(env, bytes);
        return Ok(());
    }
    Err(GovernanceError::ProposalNotFound)
}

pub fn proposal_to_kernel(proposal: Proposal) -> KernelProposal {
    KernelProposal {
        operation: proposal.operation,
        created_at: Nanoseconds::from_ns(proposal.created_at_ns),
        ttl: Nanoseconds::from_ns(proposal.ttl_ns),
        created_by: proposal.created_by,
    }
}

pub fn proposal_from_kernel(proposal: KernelProposal) -> Proposal {
    Proposal {
        operation: proposal.operation,
        created_at_ns: proposal.created_at.as_ns(),
        ttl_ns: proposal.ttl.as_ns(),
        created_by: proposal.created_by,
    }
}
