use soroban_sdk::{contractevent, Address};
use templar_proxy_oracle_soroban_governance_common::OperationKind;

#[contractevent]
#[derive(Clone)]
pub struct ProposalSubmitted {
    #[topic]
    pub id: u64,
    pub valid_after_ns: u64,
    pub action_code: u32,
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

#[contractevent]
#[derive(Clone)]
pub struct GovernanceHandoffSubmitted {
    #[topic]
    pub id: u64,
    #[topic]
    pub new_governance: Address,
}

#[contractevent]
#[derive(Clone)]
pub struct ActionTtlSet {
    pub kind: OperationKind,
    pub new_ttl_ns: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct TtlExtended {}
