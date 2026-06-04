//! Storage layout for the governance contract.
//!
//! - `ProxyOracle` (instance): the target proxy-oracle address.
//! - `Header` (instance): the governance kernel's small ledger — `next_id`,
//!   the pending id set, the TTL table, and the pending cap. Cheap to rewrite
//!   on every governance action.
//! - `Proposal(id)` (persistent): one entry per pending proposal body, so a
//!   create/cancel/execute only rewrites the single proposal it touches.
//!
//! Role membership lives in `stellar-access` (see `roles`), not here.

use soroban_sdk::{contracttype, Env, Vec};
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_soroban_common::{DEFAULT_TTL_EXTEND_TO, DEFAULT_TTL_THRESHOLD};
use templar_proxy_oracle_soroban_governance_common::{
    GovernanceAction, GovernanceError, Proposal, TtlConfig,
};

pub type KernelGovernance = templar_proxy_oracle_governance_kernel::Governance<TtlConfig>;
pub type KernelProposal =
    templar_proxy_oracle_governance_kernel::Proposal<GovernanceAction, soroban_sdk::Address>;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    ProxyOracle,
    Header,
    Proposal(u64),
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
    for id in &header.active_ids {
        active_ids.push_back(*id);
    }
    let stored = StoredHeader {
        next_id: header.next_id,
        active_ids,
        ttls: header.ttls.clone(),
        max_pending_proposals: header.max_pending_proposals,
    };
    env.storage().instance().set(&DataKey::Header, &stored);
}

pub fn load_header(env: &Env) -> Result<KernelGovernance, GovernanceError> {
    let stored: StoredHeader = env
        .storage()
        .instance()
        .get(&DataKey::Header)
        .ok_or(GovernanceError::MissingConfig)?;
    let mut header = KernelGovernance::new(stored.ttls, stored.max_pending_proposals);
    header.next_id = stored.next_id;
    header.active_ids = stored.active_ids.iter().collect();
    Ok(header)
}

pub fn load_proposal(env: &Env, id: u64) -> Option<Proposal> {
    let key = DataKey::Proposal(id);
    let proposal = env.storage().persistent().get(&key);
    if proposal.is_some() {
        env.storage()
            .persistent()
            .extend_ttl(&key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
    }
    proposal
}

pub fn save_proposal(env: &Env, id: u64, proposal: &Proposal) {
    let key = DataKey::Proposal(id);
    env.storage().persistent().set(&key, proposal);
    env.storage()
        .persistent()
        .extend_ttl(&key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
}

pub fn remove_proposal(env: &Env, id: u64) {
    env.storage().persistent().remove(&DataKey::Proposal(id));
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
