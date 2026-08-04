use std::borrow::Borrow;

use near_account_id::AccountId;
use near_api::types::transaction::actions::{Action, FunctionCallAction};
use templar_gateway_types::GovernanceVersion;
use templar_proxy_oracle_near_governance_common::{
    CreateProposalArgs, GovernancePolicyWire, Operation, Proposal, Role,
};

use crate::client::{
    macros::{contract_views, contract_writes},
    ContractWriteOptions, NearClient,
};
use crate::operation::PlannedTransaction;
use crate::{GatewayError, GatewayResult};

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
    /// large.
    ///
    /// Takes the callee's `version` rather than trusting the caller to have checked it: an older
    /// contract has no such method, and the call would fail on chain after paying for it.
    pub fn create_proposal_borsh(
        &self,
        options: ContractWriteOptions,
        version: GovernanceVersion,
        args: impl Borrow<GovCreateArgs>,
    ) -> GatewayResult<PlannedTransaction> {
        if !version.supports_borsh_create_proposal() {
            let required = GovernanceVersion::from(GovernanceVersion::BORSH_CREATE_PROPOSAL);
            return Err(GatewayError::UnsupportedFeature(format!(
                "governance {} is version {version}; borsh proposals require v{required}",
                self.contract_id(),
            )));
        }

        Ok(PlannedTransaction::single_action(
            options.signer_account_id,
            self.contract_id().to_owned(),
            Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: "create_proposal_borsh".to_owned(),
                args: near_sdk::borsh::to_vec(args.borrow())?,
                gas: options.gas,
                deposit: options.deposit,
            })),
        ))
    }

    contract_writes! {
        pub fn create_proposal(GovCreateArgs);
        pub fn cancel_proposal(GovActionArgs);
        pub fn execute_proposal(GovActionArgs);
    }
}

#[cfg(test)]
mod tests {
    use near_api::{types::transaction::actions::Action, NetworkConfig};
    use templar_gateway_types::{GovernanceVersion, ManagedAccountId};
    use templar_proxy_oracle_near_governance_common::{target, Operation};

    use super::{GovCreateArgs, NearClient};
    use crate::client::ContractWriteOptions;

    fn args() -> GovCreateArgs {
        GovCreateArgs {
            id: 7,
            operation: Operation::TargetFunctionCall(
                target::admin_rearm(
                    templar_common::oracle::pyth::PriceIdentifier([0xaa; 32]),
                    0,
                    templar_common::Nanoseconds::zero(),
                    templar_proxy_oracle_kernel::proxy::circuit_breaker::AcceptedHistorySource::Empty,
                    None,
                )
                .expect("build rearm call"),
            ),
            requested_ttl: templar_common::Nanoseconds::zero(),
        }
    }

    fn plan(
        version: (u64, u64, u64),
    ) -> crate::GatewayResult<crate::operation::PlannedTransaction> {
        let client = NearClient::new(NetworkConfig::from_rpc_url(
            "test",
            "http://127.0.0.1:1".parse().unwrap(),
        ));
        client
            .proxy_governance("gov.near".parse().unwrap())
            .create_proposal_borsh(
                ContractWriteOptions::new(ManagedAccountId("admin.near".parse().unwrap()))
                    .one_yocto()
                    .tgas(300),
                GovernanceVersion::from(version),
                args(),
            )
    }

    #[test]
    fn encodes_the_shared_args_type() {
        let planned = plan((0, 3, 0)).expect("0.3.0 supports borsh");

        let [Action::FunctionCall(action)] = &planned.actions[..] else {
            panic!("expected one function call");
        };
        assert_eq!(action.method_name, "create_proposal_borsh");
        assert_eq!(action.args, near_sdk::borsh::to_vec(&args()).unwrap());
    }

    /// The guard lives here rather than in the caller so no second caller can skip it.
    #[test]
    fn refuses_a_contract_without_the_entrypoint() {
        let error = plan((0, 2, 0)).expect_err("0.2.0 has no borsh entrypoint");
        assert!(
            error.to_string().contains("is version 0.2.0"),
            "error should name the version: {error}"
        );
    }
}
