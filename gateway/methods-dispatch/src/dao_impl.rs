use async_trait::async_trait;
use near_account_id::AccountId;
use near_api::types::transaction::actions::{Action, FunctionCallAction};
use serde::Serialize;
use sputnikdao2::{Policy, ProposalKind, RoleKind};
use templar_gateway_core::{
    DispatchRead, GatewayError, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::dao;
use templar_gateway_types::{common::WriteRequest, NearGas, NearToken};

use crate::Dispatch;

const ADD_PROPOSAL_GAS: NearGas = NearGas::from_tgas(30);

#[derive(Serialize)]
struct GetProposalArgs {
    id: u64,
}

#[derive(Serialize)]
struct AddProposalArgs {
    proposal: sputnikdao2::ProposalInput,
}

async fn get_policy<C: HasNearClient>(ctx: &C, dao_id: AccountId) -> GatewayResult<Policy> {
    ctx.near_client()
        .contract(dao_id)
        .view_function("get_policy", b"{}".to_vec())
        .await
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<dao::GetPolicy, C> for Dispatch {
    async fn dispatch(request: dao::GetPolicy, ctx: C) -> GatewayResult<dao::Policy> {
        get_policy(&ctx, request.dao_id).await.map(dao::Policy)
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<dao::GetProposal, C> for Dispatch {
    async fn dispatch(request: dao::GetProposal, ctx: C) -> GatewayResult<dao::ProposalOutput> {
        ctx.near_client()
            .contract(request.dao_id)
            .view_function(
                "get_proposal",
                serde_json::to_vec(&GetProposalArgs { id: request.id })?,
            )
            .await
            .map(dao::ProposalOutput)
    }
}

fn role_matches(kind: &RoleKind, account_id: &AccountId, weight: u128) -> bool {
    match kind {
        RoleKind::Everyone => true,
        RoleKind::Member(required) => weight >= required.0,
        RoleKind::Group(accounts) => accounts.contains(account_id),
    }
}

/// Whether the policy permits this proposal at any possible delegated weight.
/// This policy-only preflight does not fetch delegated balances. Reject only
/// known-ineligible proposers; the contract remains authoritative for membership.
fn may_add_proposal(policy: &Policy, account_id: &AccountId, kind: &ProposalKind) -> bool {
    let label = kind.to_policy_label();

    policy.roles.iter().enumerate().any(|(index, role)| {
        let permits = role.permissions.iter().any(|permission| {
            permission
                .split_once(':')
                .is_some_and(|(proposal, action)| {
                    (proposal == label || proposal == "*")
                        && (action == "AddProposal" || action == "*")
                })
        });
        if !permits {
            return false;
        }

        let minimum_weight = match &role.kind {
            RoleKind::Member(required) => required.0,
            _ => 0,
        };
        if !role_matches(&role.kind, account_id, minimum_weight) {
            return false;
        }

        // Policy::get_user_roles overwrites earlier matching roles with the
        // same name. At this role's minimum weight, a later matching role will
        // also match every larger weight, so this role can never become effective.
        // A later granting role is considered independently at its own minimum.
        !policy.roles[index + 1..].iter().any(|later| {
            later.name == role.name && role_matches(&later.kind, account_id, minimum_weight)
        })
    })
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<dao::AddProposal, C> for Dispatch {
    async fn plan(request: WriteRequest<dao::AddProposal>, ctx: C) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let policy = get_policy(&ctx, body.dao_id.clone()).await?;
        if !may_add_proposal(&policy, &request.signer_account_id.0, &body.proposal.kind) {
            return Err(GatewayError::RequestPreconditionFailed(format!(
                "{} is not permitted to add {} proposals to {}",
                request.signer_account_id.0,
                body.proposal.kind.to_policy_label(),
                body.dao_id,
            )));
        }
        let limits = ctx.near_client().chain().protocol_limits().await?;
        if ADD_PROPOSAL_GAS > limits.max_total_prepaid_gas {
            return Err(GatewayError::RequestPreconditionFailed(
                "DAO proposal gas exceeds the live protocol limit".to_owned(),
            ));
        }

        Ok(OperationPlan::execute(
            request.signer_account_id,
            body.dao_id,
            vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: "add_proposal".to_owned(),
                args: serde_json::to_vec(&AddProposalArgs {
                    proposal: body.proposal,
                })?,
                gas: ADD_PROPOSAL_GAS,
                deposit: NearToken::from_yoctonear(policy.proposal_bond.as_yoctonear()),
            }))],
        ))
    }
}

#[cfg(test)]
mod tests {
    use near_account_id::AccountId;
    use serde_json::json;
    use sputnikdao2::{Policy, ProposalKind, RolePermission};

    use super::may_add_proposal;

    fn role(name: &str, kind: &serde_json::Value, permissions: &[&str]) -> RolePermission {
        serde_json::from_value(json!({
            "name": name,
            "kind": kind,
            "permissions": permissions,
            "vote_policy": {},
        }))
        .unwrap()
    }

    fn policy(roles: Vec<RolePermission>) -> Policy {
        Policy {
            roles,
            ..sputnikdao2::default_policy(vec![])
        }
    }

    #[test]
    fn exact_kind_and_both_wildcards_grant_only_add_permission() {
        let account: AccountId = "member.near".parse().unwrap();
        for permission in ["vote:AddProposal", "vote:*", "*:AddProposal", "*:*"] {
            let policy = policy(vec![role("all", &json!("Everyone"), &[permission])]);
            assert!(may_add_proposal(&policy, &account, &ProposalKind::Vote));
        }
        for permission in ["call:AddProposal", "vote:VoteApprove", "*:VoteApprove"] {
            let policy = policy(vec![role("all", &json!("Everyone"), &[permission])]);
            assert!(!may_add_proposal(&policy, &account, &ProposalKind::Vote));
        }
    }

    #[test]
    fn everyone_still_matches_group_members_and_group_exclusion_is_known() {
        let member: AccountId = "member.near".parse().unwrap();
        let outsider: AccountId = "outsider.near".parse().unwrap();
        let mut group = role(
            "council",
            &json!({"Group": [member]}),
            &["vote:AddProposal"],
        );
        let restricted = policy(vec![group.clone()]);
        assert!(may_add_proposal(&restricted, &member, &ProposalKind::Vote));
        assert!(!may_add_proposal(
            &restricted,
            &outsider,
            &ProposalKind::Vote
        ));
        group.permissions.clear();
        let open = policy(vec![
            role("all", &json!("Everyone"), &["*:AddProposal"]),
            group,
        ]);
        assert!(may_add_proposal(&open, &member, &ProposalKind::Vote));
    }

    #[test]
    fn unknown_member_weight_does_not_cause_rejection() {
        let account: AccountId = "member.near".parse().unwrap();
        for threshold in [0, 1, u128::MAX] {
            let policy = policy(vec![role(
                "stakers",
                &json!({"Member": threshold.to_string()}),
                &["vote:AddProposal"],
            )]);
            assert!(may_add_proposal(&policy, &account, &ProposalKind::Vote));
        }
    }

    #[test]
    fn duplicate_names_use_last_matching_role_at_each_possible_weight() {
        let account: AccountId = "member.near".parse().unwrap();
        let open = || role("same", &json!("Everyone"), &["vote:AddProposal"]);
        let deny = || role("same", &json!("Everyone"), &[]);
        assert!(!may_add_proposal(
            &policy(vec![open(), deny()]),
            &account,
            &ProposalKind::Vote
        ));
        assert!(may_add_proposal(
            &policy(vec![deny(), open()]),
            &account,
            &ProposalKind::Vote
        ));
        let positive_weight_denies = role("same", &json!({"Member": "1"}), &[]);
        assert!(may_add_proposal(
            &policy(vec![open(), positive_weight_denies]),
            &account,
            &ProposalKind::Vote,
        ));
        let grant_at_ten = role("same", &json!({"Member": "10"}), &["vote:AddProposal"]);
        let deny_at_five = role("same", &json!({"Member": "5"}), &[]);
        assert!(!may_add_proposal(
            &policy(vec![grant_at_ten, deny_at_five]),
            &account,
            &ProposalKind::Vote,
        ));
    }
}
