use near_account_id::AccountId;
use templar_gateway_types::common::ContractArgs;
use templar_proxy_oracle_near_governance_common::{
    CreateProposalArgs, GovernancePolicyWire, Operation, Proposal, Role,
};

use crate::client::{
    macros::{contract_views, contract_writes},
    ContractWriteOptions, NearClient,
};
use crate::operation::PlannedTransaction;
use crate::GatewayResult;

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
pub type GovCreateArgs = CreateProposalArgs<Operation>;
#[derive(serde::Serialize)]
pub struct GovActionArgs {
    pub id: u32,
}
#[derive(serde::Serialize)]
pub struct GovListArgs {
    pub offset: Option<u32>,
    pub count: Option<u32>,
}
#[derive(serde::Serialize)]
pub struct GovHasRoleArgs {
    pub account_id: AccountId,
    pub role: Role,
}
#[derive(serde::Serialize)]
pub struct GovListRoleArgs {
    pub role: Role,
    pub offset: Option<u32>,
    pub count: Option<u32>,
}
#[derive(serde::Serialize)]
pub struct GovGetRolesArgs {
    pub account_id: AccountId,
}

impl ProxyGovernanceClient<'_> {
    contract_views! {
        pub fn next_proposal_id(()) -> u32;
        pub fn proposal_count(()) -> u32;
        pub fn get_governance_policy(()) -> GovernancePolicyWire;
        pub fn list_proposals(GovListArgs) -> Vec<u32>;
        pub fn get_proposal(GovGetArgs) -> Option<Proposal<Operation>>;
        pub fn has_role(GovHasRoleArgs) -> bool;
        pub fn list_role(GovListRoleArgs) -> Vec<AccountId>;
        pub fn get_roles(GovGetRolesArgs) -> Vec<Role>;
        pub fn get_proxy_oracle_id(()) -> AccountId;
    }

    /// Borsh-encoded twin of [`Self::create_proposal`], for payloads JSON makes too costly or too
    /// large. `contract_writes!` always emits `ContractArgs::Json`, so this is hand-written.
    pub fn create_proposal_borsh(
        &self,
        options: ContractWriteOptions,
        args: &CreateProposalArgs<Operation>,
    ) -> GatewayResult<PlannedTransaction> {
        let encoded = near_sdk::borsh::to_vec(args)?;
        Ok(PlannedTransaction {
            signer_account_id: options.signer_account_id,
            receiver_id: self.contract_id().to_owned(),
            actions: vec![
                ::near_api::types::transaction::actions::Action::FunctionCall(Box::new(
                    ::near_api::types::transaction::actions::FunctionCallAction {
                        method_name: "create_proposal_borsh".to_owned(),
                        args: ContractArgs::Raw(encoded.into()).try_into_bytes()?,
                        gas: options.gas,
                        deposit: options.deposit,
                    },
                )),
            ],
            continue_on_failure: false,
        })
    }

    contract_writes! {
        pub fn create_proposal(GovCreateArgs);
        pub fn cancel_proposal(GovActionArgs);
        pub fn execute_proposal(GovActionArgs);
    }
}
