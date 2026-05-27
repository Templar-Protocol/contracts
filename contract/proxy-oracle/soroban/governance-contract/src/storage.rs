//! Storage layout for the governance contract: instance-storage singletons
//! (`ProxyOracle`, `Ttls`, `NextProposalId`, `ProposalCount`) plus the
//! proposal table (`ProposalIds`, `Proposal(id)`).

use soroban_sdk::{contracttype, Env, Vec};
use templar_proxy_oracle_soroban_common::{GovernanceError, Proposal, TtlConfig};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    ProxyOracle,
    Ttls,
    NextProposalId,
    ProposalIds,
    ProposalCount,
    Proposal(u64),
}

pub fn load_ttls(env: &Env) -> Result<TtlConfig, GovernanceError> {
    env.storage()
        .instance()
        .get(&DataKey::Ttls)
        .ok_or(GovernanceError::MissingConfig)
}

pub fn save_ttls(env: &Env, ttls: &TtlConfig) {
    env.storage().instance().set(&DataKey::Ttls, ttls);
}

pub fn load_proposal_ids(env: &Env) -> Vec<u64> {
    env.storage()
        .instance()
        .get(&DataKey::ProposalIds)
        .unwrap_or_else(|| Vec::new(env))
}

pub fn save_proposal_ids(env: &Env, ids: &Vec<u64>) {
    env.storage().instance().set(&DataKey::ProposalIds, ids);
}

pub fn load_proposal(env: &Env, id: u64) -> Option<Proposal> {
    env.storage().instance().get(&DataKey::Proposal(id))
}

pub fn save_proposal(env: &Env, id: u64, proposal: &Proposal) {
    env.storage()
        .instance()
        .set(&DataKey::Proposal(id), proposal);
}

pub fn remove_proposal(env: &Env, id: u64) {
    env.storage().instance().remove(&DataKey::Proposal(id));
}

pub fn remove_proposal_id(env: &Env, id: u64) {
    let mut ids = load_proposal_ids(env);
    if let Some(index) = ids
        .iter()
        .position(|pid| pid == id)
        .and_then(|index| u32::try_from(index).ok())
    {
        ids.remove(index);
    }
    save_proposal_ids(env, &ids);
}

pub fn load_proposal_count(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::ProposalCount)
        .unwrap_or(0)
}

pub fn save_proposal_count(env: &Env, count: u32) {
    env.storage()
        .instance()
        .set(&DataKey::ProposalCount, &count);
}
