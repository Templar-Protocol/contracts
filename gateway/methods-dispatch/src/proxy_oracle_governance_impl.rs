use async_trait::async_trait;
use templar_gateway_core::{
    client::{
        proxy_governance::{
            GovActionArgs, GovCreateArgs, GovGetArgs, GovGetRolesArgs, GovHasRoleArgs, GovListArgs,
            GovListRoleArgs,
        },
        ContractWriteOptions,
    },
    DispatchRead, GatewayError, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::proxy_oracle_governance;
use templar_gateway_types::{Governance, GovernanceVersion, ProposalEncoding};

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::NextProposalId, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::NextProposalId,
        ctx: C,
    ) -> GatewayResult<u32> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .next_proposal_id(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::ProposalCount, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::ProposalCount,
        ctx: C,
    ) -> GatewayResult<u32> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .proposal_count(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetGovernancePolicy, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::GetGovernancePolicy,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetGovernancePolicyResult> {
        let policy = ctx
            .near_client()
            .proxy_governance(request.governance_id)
            .get_governance_policy(())
            .await?;
        Ok(proxy_oracle_governance::GetGovernancePolicyResult { policy })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::ListProposals, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::ListProposals,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::ListProposalsResult> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .list_proposals(GovListArgs {
                offset: request.offset,
                count: request.count,
            })
            .await
            .map(|ids| proxy_oracle_governance::ListProposalsResult { ids })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetProposal, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::GetProposal,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetProposalResult> {
        let params = request;
        ctx.near_client()
            .proxy_governance(params.governance_id)
            .get_proposal(GovGetArgs { id: params.id })
            .await
            .map(|proposal| proxy_oracle_governance::GetProposalResult { proposal })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetProxyOracleId, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::GetProxyOracleId,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetProxyOracleIdResult> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .get_proxy_oracle_id(())
            .await
            .map(
                |proxy_oracle_id| proxy_oracle_governance::GetProxyOracleIdResult {
                    proxy_oracle_id,
                },
            )
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::HasRole, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::HasRole,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::HasRoleResult> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .has_role(GovHasRoleArgs {
                account_id: request.account_id,
                role: request.role,
            })
            .await
            .map(|has_role| proxy_oracle_governance::HasRoleResult { has_role })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::ListRole, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::ListRole,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::ListRoleResult> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .list_role(GovListRoleArgs {
                role: request.role,
                offset: request.offset,
                count: request.count,
            })
            .await
            .map(|members| proxy_oracle_governance::ListRoleResult { members })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetRoles, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::GetRoles,
        ctx: C,
    ) -> GatewayResult<proxy_oracle_governance::GetRolesResult> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .get_roles(GovGetRolesArgs {
                account_id: request.account_id,
            })
            .await
            .map(|roles| proxy_oracle_governance::GetRolesResult { roles })
    }
}

/// Refuse rather than silently downgrading to JSON: a caller opts into borsh because JSON cannot
/// carry the payload, so a fallback would fail differently on chain instead of here.
fn ensure_supports_borsh(
    governance_id: &near_account_id::AccountId,
    version: GovernanceVersion,
) -> GatewayResult<()> {
    if version.supports_borsh_create_proposal() {
        return Ok(());
    }
    let (major, minor, patch) = GovernanceVersion::BORSH_CREATE_PROPOSAL;
    Err(GatewayError::UnsupportedFeature(format!(
        "governance {governance_id} is version {version}; \
         borsh proposals require v{major}.{minor}.{patch}"
    )))
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_governance::CreateProposal, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<
            proxy_oracle_governance::CreateProposal,
        >,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let governance_id = body.governance_id;
        let options = ContractWriteOptions::new(request.signer_account_id)
            .one_yocto()
            .tgas(300);
        let args = GovCreateArgs {
            id: body.id,
            operation: body.operation,
            requested_ttl: body.requested_ttl,
        };

        match body.encoding {
            // No version read: JSON works wherever the current `Operation` shape works at all.
            ProposalEncoding::Json => ctx
                .near_client()
                .proxy_governance(governance_id)
                .create_proposal(options, args)
                .map(OperationPlan::from),
            ProposalEncoding::Borsh => {
                // Uncached: the metadata cache holds for an hour, which would refuse borsh for
                // that long after a governance upgrade — exactly when it is wanted.
                let version = ctx
                    .near_client()
                    .contract(governance_id.clone())
                    .version::<Governance>()
                    .await?;
                ensure_supports_borsh(&governance_id, version)?;
                ctx.near_client()
                    .proxy_governance(governance_id)
                    .create_proposal_borsh(options, &args)
                    .map(OperationPlan::from)
            }
        }
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_governance::CancelProposal, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<
            proxy_oracle_governance::CancelProposal,
        >,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .proxy_governance(body.governance_id)
            .cancel_proposal(
                ContractWriteOptions::new(request.signer_account_id)
                    .one_yocto()
                    .tgas(300),
                GovActionArgs { id: body.id },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_governance::ExecuteProposal, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<
            proxy_oracle_governance::ExecuteProposal,
        >,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .proxy_governance(body.governance_id)
            .execute_proposal(
                ContractWriteOptions::new(request.signer_account_id)
                    .one_yocto()
                    .tgas(300),
                GovActionArgs { id: body.id },
            )
            .map(OperationPlan::from)
    }
}

#[cfg(test)]
mod tests {
    use near_api::{types::transaction::actions::Action, NetworkConfig};
    use templar_gateway_core::{HasNearClient, NearClient, PlanWrite};
    use templar_gateway_methods_spec::proxy_oracle_governance;
    use templar_gateway_types::{
        common::WriteRequest, GovernanceVersion, ManagedAccountId, ProposalEncoding,
    };
    use templar_proxy_oracle_near_governance_common::{target, CreateProposalArgs, Operation};

    use super::{ensure_supports_borsh, Dispatch};

    #[derive(Clone)]
    struct TestCtx(NearClient);

    impl HasNearClient for TestCtx {
        fn near_client(&self) -> &NearClient {
            &self.0
        }
    }

    /// Points at a closed port, so any path that touches the network fails.
    fn offline_ctx() -> TestCtx {
        TestCtx(NearClient::new(NetworkConfig::from_rpc_url(
            "test",
            "http://127.0.0.1:1".parse().unwrap(),
        )))
    }

    fn operation() -> Operation {
        Operation::TargetFunctionCall(
            target::admin_rearm(
                templar_common::oracle::pyth::PriceIdentifier([0xaa; 32]),
                0,
                templar_common::Nanoseconds::zero(),
                templar_proxy_oracle_kernel::proxy::circuit_breaker::AcceptedHistorySource::Empty,
                None,
            )
            .expect("build rearm call"),
        )
    }

    fn request(
        encoding: ProposalEncoding,
    ) -> WriteRequest<proxy_oracle_governance::CreateProposal> {
        WriteRequest {
            signer_account_id: ManagedAccountId("admin.near".parse().unwrap()),
            idempotency_key: None,
            body: proxy_oracle_governance::CreateProposal {
                governance_id: "gov.near".parse().unwrap(),
                id: 7,
                operation: operation(),
                requested_ttl: templar_common::Nanoseconds::zero(),
                encoding,
            },
        }
    }

    async fn plan(
        encoding: ProposalEncoding,
    ) -> templar_gateway_core::GatewayResult<templar_gateway_core::OperationPlan> {
        <Dispatch as PlanWrite<proxy_oracle_governance::CreateProposal, TestCtx>>::plan(
            request(encoding),
            offline_ctx(),
        )
        .await
    }

    /// Planning offline *is* the assertion: it can only succeed if no version was read.
    #[tokio::test]
    async fn json_encoding_plans_without_reading_a_version() {
        let plan = plan(ProposalEncoding::Json)
            .await
            .expect("json planning must not touch the network");

        let [Action::FunctionCall(action)] = &plan.steps[0].actions[..] else {
            panic!("expected one function call");
        };
        assert_eq!(action.method_name, "create_proposal");
        assert!(action.args.starts_with(b"{"), "expected json arguments");
    }

    /// The converse of the above: the same offline client fails, so opting in does read a version.
    #[tokio::test]
    async fn borsh_encoding_reads_the_version_first() {
        plan(ProposalEncoding::Borsh)
            .await
            .expect_err("borsh planning must consult the governance version");
    }

    #[test]
    fn borsh_planning_encodes_the_shared_args_type() {
        let args = CreateProposalArgs {
            id: 7,
            operation: operation(),
            requested_ttl: templar_common::Nanoseconds::zero(),
        };
        let planned = offline_ctx()
            .near_client()
            .proxy_governance("gov.near".parse().unwrap())
            .create_proposal_borsh(
                templar_gateway_core::client::ContractWriteOptions::new(ManagedAccountId(
                    "admin.near".parse().unwrap(),
                ))
                .one_yocto()
                .tgas(300),
                &args,
            )
            .expect("plan borsh call");

        let [Action::FunctionCall(action)] = &planned.actions[..] else {
            panic!("expected one function call");
        };
        assert_eq!(action.method_name, "create_proposal_borsh");
        assert_eq!(action.args, borsh::to_vec(&args).unwrap());
    }

    #[test]
    fn borsh_is_refused_below_0_3_0() {
        let governance_id = "gov.near".parse().unwrap();
        let error = ensure_supports_borsh(&governance_id, GovernanceVersion::from((0, 2, 0)))
            .expect_err("0.2.0 has no borsh entrypoint");
        assert!(
            error.to_string().contains("is version 0.2.0"),
            "error should name the version: {error}"
        );

        ensure_supports_borsh(&governance_id, GovernanceVersion::from((0, 3, 0)))
            .expect("0.3.0 supports borsh");
    }

    /// The persisted idempotency fingerprint hashes these params, so a default request must
    /// serialize as it did before `encoding` existed or retries stop matching their stored operation.
    #[test]
    fn a_default_request_keeps_its_fingerprint() {
        let body = request(ProposalEncoding::Json).body;
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("encoding").is_none(), "{json}");

        let opted_in = request(ProposalEncoding::Borsh).body;
        assert_eq!(
            serde_json::to_value(&opted_in).unwrap()["encoding"],
            serde_json::json!("borsh")
        );
    }

    /// Requests written before this field existed keep their behaviour.
    #[test]
    fn a_request_omitting_encoding_defaults_to_json() {
        let body: proxy_oracle_governance::CreateProposal =
            serde_json::from_value(serde_json::json!({
                "governance_id": "gov.near",
                "id": 7,
                "operation": operation(),
                "requested_ttl": "0",
            }))
            .expect("legacy request shape must still deserialize");

        assert_eq!(body.encoding, ProposalEncoding::Json);
    }
}
