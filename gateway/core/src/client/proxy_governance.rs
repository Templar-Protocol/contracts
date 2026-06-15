use templar_common::Nanoseconds;
use templar_proxy_oracle_near_governance_common::{Operation, OperationKind, Proposal};

use crate::client::{
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

/// Client for the proxy-oracle governance contract (a separate account from the
/// oracle it governs, in `>= 0.2.0` deployments).
#[derive(Clone)]
pub struct ProxyGovernanceClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: near_account_id::AccountId,
}

impl BoundContractClient for ProxyGovernanceClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }
    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

#[derive(serde::Serialize)]
pub struct GovGetArgs {
    pub id: u32,
}
#[derive(serde::Serialize)]
pub struct GovCreateArgs {
    pub id: u32,
    pub operation: Operation,
    pub requested_ttl: Nanoseconds,
}
#[derive(serde::Serialize)]
pub struct GovActionArgs {
    pub id: u32,
}
#[derive(serde::Serialize)]
pub struct GovTtlArgs {
    pub kind: OperationKind,
}
#[derive(serde::Serialize)]
pub struct GovListArgs {
    pub offset: Option<u32>,
    pub count: Option<u32>,
}

impl ProxyGovernanceClient<'_> {
    contract_views! {
        pub fn next_proposal_id(()) -> u32;
        pub fn proposal_count(()) -> u32;
        pub fn get_operation_ttl(GovTtlArgs) -> Nanoseconds;
        pub fn list_proposals(GovListArgs) -> Vec<u32>;
        pub fn get_proposal(GovGetArgs) -> Option<Proposal<Operation>>;
    }

    contract_writes! {
        pub fn create_proposal(GovCreateArgs);
        pub fn cancel_proposal(GovActionArgs);
        pub fn execute_proposal(GovActionArgs);
    }
}
