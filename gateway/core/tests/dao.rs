use anyhow::{Context as _, Result};
use near_account_id::AccountId;
use near_api::types::transaction::actions::Action;
use serde_json::{json, Value};
use templar_gateway_core::{GatewayContext, GatewayError, PlanWrite};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_methods_spec::{dao, tx};
use templar_gateway_testing::{failed_receipts, SandboxHarness};
use templar_gateway_types::{
    common::{ContractArgs, WriteOperationResult, WriteRequest},
    Base64Bytes, ContractMethodName, ManagedAccountId, NearGas, NearToken,
};

fn proposal(dao_id: &AccountId, receiver_id: &AccountId) -> Result<dao::AddProposal> {
    Ok(serde_json::from_value(json!({
        "dao_id": dao_id,
        "proposal": {
            "description": "Set a raw-unit rate through threshold approval",
            "kind": { "FunctionCall": {
                "receiver_id": receiver_id,
                "actions": [{
                    "method_name": "set_redemption_rate",
                    "args": Base64Bytes(serde_json::to_vec(&json!({"redemption_rate": "42"}))?),
                    "deposit": "0",
                    "gas": "30000000000000"
                }]
            }}
        }
    }))?)
}

fn assert_dao_rejection(
    result: &WriteOperationResult,
    dao_id: &AccountId,
    reason: &str,
) -> Result<()> {
    assert!(failed_receipts(result).any(|receipt| &receipt.contract_id == dao_id));
    let outcome = result
        .operation
        .final_outcome()
        .context("rejected DAO call must have a final outcome")?;
    assert!(
        outcome
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains(reason)),
        "expected {reason}, got {:?}",
        outcome.failure
    );
    Ok(())
}

fn proposal_id(result: &WriteOperationResult) -> Result<u64> {
    result
        .operation
        .final_outcome()
        .context("proposal submission has no outcome")?
        .return_value_json()?
        .context("proposal submission has no returned id")
}

async fn get_proposal(harness: &SandboxHarness, dao_id: &AccountId, id: u64) -> Result<Value> {
    Ok(serde_json::to_value(
        harness
            .client()?
            .read(dao::GetProposal {
                dao_id: dao_id.clone(),
                id,
            })
            .await?,
    )?)
}

async fn vote(
    harness: &SandboxHarness,
    voter: &ManagedAccountId,
    dao_id: &AccountId,
    stored: &Value,
) -> Result<WriteOperationResult> {
    harness
        .try_call_function(
            voter,
            dao_id,
            "act_proposal",
            json!({
                "id": stored["id"],
                "action": "VoteApprove",
                "proposal": stored["kind"],
                "memo": null
            }),
        )
        .await
}

#[allow(
    clippy::too_many_lines,
    reason = "one real DAO lifecycle proves bond, policy, threshold and downstream execution"
)]
#[tokio::test]
async fn dao_proposals_enforce_live_bond_policy_and_threshold() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let proposer = harness.create_user("proposer").await?;
    let council_a = harness.create_user("council-a").await?;
    let council_b = harness.create_user("council-b").await?;
    let outsider = harness.create_user("outsider").await?;
    let policy = json!({
        "roles": [
            {"name": "proposers", "kind": {"Group": [proposer]},
             "permissions": ["call:AddProposal", "policy_update_parameters:AddProposal"], "vote_policy": {}},
            {"name": "council", "kind": {"Group": [council_a, council_b]},
             "permissions": ["*:VoteApprove", "*:VoteReject", "*:Finalize"],
             "vote_policy": {}}
        ],
        "default_vote_policy": {
            "weight_kind": "RoleWeight", "quorum": "2", "threshold": "2"
        },
        "proposal_bond": NearToken::from_millinear(100),
        "proposal_period": "604800000000000",
        "bounty_bond": "0",
        "bounty_forgiveness_period": "86400000000000"
    });
    let expected_policy: dao::Policy = serde_json::from_value(policy)?;
    let dao_id = harness
        .deploy_sputnik_dao("real-dao", expected_policy.0.clone())
        .await?;
    let client = harness.client()?;
    assert_eq!(
        client
            .read(dao::GetPolicy {
                dao_id: dao_id.clone()
            })
            .await?,
        expected_policy
    );

    // A known-ineligible proposer is rejected before planning any submission.
    let rejected = <Dispatch as PlanWrite<dao::AddProposal, GatewayContext>>::plan(
        WriteRequest {
            signer_account_id: outsider.clone(),
            idempotency_key: None,
            body: proposal(&dao_id, &harness.ft_contract_id)?,
        },
        GatewayContext::from_near_client(harness.gateway_client()),
    )
    .await;
    assert!(matches!(
        rejected,
        Err(GatewayError::RequestPreconditionFailed(_))
    ));
    let count: u64 = harness
        .view_json(&dao_id, "get_last_proposal_id", json!({}))
        .await?;
    assert_eq!(count, 0);

    // Bypass the planner to verify the deployed contract also enforces authorization and bond.
    for (signer, deposit, reason) in [
        (
            &outsider,
            NearToken::from_millinear(100),
            "ERR_PERMISSION_DENIED",
        ),
        (&proposer, NearToken::ZERO, "ERR_MIN_BOND"),
    ] {
        let input = proposal(&dao_id, &harness.ft_contract_id)?;
        let rejected = harness
            .try_execute(
                signer,
                tx::FunctionCall {
                    receiver_id: dao_id.clone(),
                    method_name: ContractMethodName("add_proposal".to_owned()),
                    args: ContractArgs::Json(json!({"proposal": input.proposal})),
                    gas: NearGas::from_tgas(30),
                    deposit,
                },
            )
            .await?;
        assert_dao_rejection(&rejected, &dao_id, reason)?;
        assert_eq!(
            harness
                .view_json::<u64>(&dao_id, "get_last_proposal_id", json!({}))
                .await?,
            0
        );
    }

    let input = proposal(&dao_id, &harness.ft_contract_id)?;
    let expected_body = serde_json::to_value(&input.proposal)?;
    let submitted = harness.execute(&proposer, input).await?;
    let id = proposal_id(&submitted)?;
    let stored = get_proposal(&harness, &dao_id, id).await?;
    assert_eq!(stored["id"], id);
    assert_eq!(stored["description"], expected_body["description"]);
    assert_eq!(stored["kind"], expected_body["kind"]);
    assert_eq!(stored["proposer"], proposer.as_str());
    assert_eq!(stored["status"], "InProgress");

    let unauthorized = vote(&harness, &outsider, &dao_id, &stored).await?;
    assert_dao_rejection(&unauthorized, &dao_id, "ERR_PERMISSION_DENIED")?;
    assert_eq!(get_proposal(&harness, &dao_id, id).await?, stored);
    let first = vote(&harness, &council_a, &dao_id, &stored).await?;
    assert!(failed_receipts(&first).next().is_none());
    assert_eq!(
        get_proposal(&harness, &dao_id, id).await?["status"],
        "InProgress"
    );
    let second = vote(&harness, &council_b, &dao_id, &stored).await?;
    assert!(failed_receipts(&second).next().is_none());
    assert_eq!(
        get_proposal(&harness, &dao_id, id).await?["status"],
        "Approved"
    );
    let rate: String = harness
        .view_json(&harness.ft_contract_id, "redemption_rate", json!({}))
        .await?;
    assert_eq!(rate, "42");

    // The planner must observe changed policy rather than caching an obsolete proposal bond.
    let new_bond = NearToken::from_millinear(200);
    let change: dao::AddProposal = serde_json::from_value(json!({
        "dao_id": dao_id,
        "proposal": {"description": "Update proposal bond through threshold approval", "kind": {
            "ChangePolicyUpdateParameters": {"parameters": {
                "proposal_bond": new_bond, "proposal_period": null,
                "bounty_bond": null, "bounty_forgiveness_period": null
            }}
        }}
    }))?;
    let changed = harness.execute(&proposer, change).await?;
    let change_id = proposal_id(&changed)?;
    let change_body = get_proposal(&harness, &dao_id, change_id).await?;
    for voter in [&council_a, &council_b] {
        let result = vote(&harness, voter, &dao_id, &change_body).await?;
        assert!(failed_receipts(&result).next().is_none());
    }
    assert_eq!(
        serde_json::to_value(
            client
                .read(dao::GetPolicy {
                    dao_id: dao_id.clone()
                })
                .await?
        )?["proposal_bond"],
        serde_json::to_value(new_bond)?
    );
    let subsequent = harness
        .execute(&proposer, proposal(&dao_id, &harness.ft_contract_id)?)
        .await?;
    assert_eq!(
        get_proposal(&harness, &dao_id, proposal_id(&subsequent)?).await?["status"],
        "InProgress"
    );
    Ok(())
}

#[tokio::test]
async fn dao_preflight_matches_contract_wildcards_and_duplicate_roles() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let member = harness.create_user("member").await?;
    let outsider = harness.create_user("outsider").await?;
    let role = |kind: Value, permissions: &[&str]| json!({"name": "same", "kind": kind, "permissions": permissions, "vote_policy": {}});
    let cases = [
        (
            "wildcard",
            vec![role(json!("Everyone"), &["*:AddProposal"])],
            [true, true],
        ),
        (
            "group-overrides",
            vec![
                role(json!("Everyone"), &["*:AddProposal"]),
                role(json!({"Group": [member]}), &[]),
            ],
            [false, true],
        ),
        (
            "everyone-overrides",
            vec![
                role(json!({"Group": [member]}), &[]),
                role(json!("Everyone"), &["call:*"]),
            ],
            [true, true],
        ),
    ];
    for (name, roles, expected) in cases {
        let policy: dao::Policy = serde_json::from_value(json!({
            "roles": roles,
            "default_vote_policy": {
                "weight_kind": "RoleWeight", "quorum": "1", "threshold": "1"
            },
            "proposal_bond": NearToken::from_millinear(100),
            "proposal_period": "604800000000000",
            "bounty_bond": "0",
            "bounty_forgiveness_period": "86400000000000"
        }))?;
        let dao_id = harness.deploy_sputnik_dao(name, policy.0).await?;
        let mut count = 0_u64;
        for (signer, permitted) in [&member, &outsider].into_iter().zip(expected) {
            let input = proposal(&dao_id, &harness.ft_contract_id)?;
            let planned = <Dispatch as PlanWrite<dao::AddProposal, GatewayContext>>::plan(
                WriteRequest {
                    signer_account_id: signer.clone(),
                    idempotency_key: None,
                    body: input.clone(),
                },
                GatewayContext::from_near_client(harness.gateway_client()),
            )
            .await;
            if permitted {
                planned?;
            } else {
                assert!(matches!(
                    planned,
                    Err(GatewayError::RequestPreconditionFailed(_))
                ));
            }
            // Bypass preflight so the pinned contract independently checks the same policy.
            let result = harness
                .try_execute(
                    signer,
                    tx::FunctionCall {
                        receiver_id: dao_id.clone(),
                        method_name: ContractMethodName("add_proposal".to_owned()),
                        args: ContractArgs::Json(json!({"proposal": input.proposal})),
                        gas: NearGas::from_tgas(30),
                        deposit: NearToken::from_millinear(100),
                    },
                )
                .await?;
            if permitted {
                assert!(failed_receipts(&result).next().is_none());
                let id = proposal_id(&result)?;
                assert_eq!(id, count);
                let stored = get_proposal(&harness, &dao_id, id).await?;
                assert_eq!(stored["proposer"], signer.as_str());
                assert_eq!(stored["status"], "InProgress");
                count += 1;
            } else {
                assert_dao_rejection(&result, &dao_id, "ERR_PERMISSION_DENIED")?;
            }
            assert_eq!(
                harness
                    .view_json::<u64>(&dao_id, "get_last_proposal_id", json!({}))
                    .await?,
                count
            );
        }
    }
    Ok(())
}

async fn execute_governance_plan_through_dao(
    harness: &SandboxHarness,
    proposer: &ManagedAccountId,
    council: &[ManagedAccountId; 2],
    dao_id: &AccountId,
    plan: templar_gateway_core::OperationPlan,
) -> Result<WriteOperationResult> {
    let mut steps = plan.steps.into_iter();
    let transaction = steps.next().context("governance call transaction")?;
    anyhow::ensure!(steps.next().is_none(), "one governance call required");
    let transaction = serde_json::to_value(transaction)?;
    let request = serde_json::from_value::<dao::AddProposal>(json!({
        "dao_id": dao_id,
        "proposal": {"description":"Governance execution with DAO gas headroom",
            "kind":{"FunctionCall":{"receiver_id":transaction["receiver_id"],
                "actions":[transaction["actions"][0]["FunctionCall"]]}}}
    }))?;
    let expected_kind = request.proposal.kind.clone();
    let submitted = harness.execute(proposer, request).await?;
    let id = proposal_id(&submitted)?;
    let stored = harness
        .client()?
        .read(dao::GetProposal {
            dao_id: dao_id.clone(),
            id,
        })
        .await?;
    assert_eq!(
        stored.0.proposal.kind, expected_kind,
        "DAO approval must freeze the receiver, method, gas, deposit, and argument bytes"
    );
    let stored = serde_json::to_value(stored)?;
    let first = vote(harness, &council[0], dao_id, &stored).await?;
    anyhow::ensure!(
        failed_receipts(&first).next().is_none(),
        "first DAO vote failed: {:?}",
        first.operation
    );
    vote(harness, &council[1], dao_id, &stored).await
}

#[allow(
    clippy::too_many_lines,
    reason = "one real DAO lifecycle compares default and explicit gas, frozen actions and failures"
)]
#[tokio::test]
async fn governance_budgets_execute_target_through_real_dao_threshold() -> Result<()> {
    use templar_common::{oracle::pyth::PriceIdentifier, Nanoseconds};
    use templar_gateway_methods_spec::{proxy_oracle, proxy_oracle_governance as gov};
    use templar_gateway_types::ProposalEncoding;
    use templar_proxy_oracle_kernel::proxy::{FreshnessFilter, Proxy};
    use templar_proxy_oracle_near_common::request::OracleRequest;
    use templar_proxy_oracle_near_governance_common::Operation;

    let harness = SandboxHarness::start().await?;
    let proposer = harness.create_user("governance-proposer").await?;
    let council = [
        harness.create_user("governance-council-a").await?,
        harness.create_user("governance-council-b").await?,
    ];
    let policy = serde_json::from_value(json!({
        "roles":[
            {"name":"proposer","kind":{"Group":[proposer]},"permissions":["call:AddProposal"],"vote_policy":{}},
            {"name":"council","kind":{"Group":council},"permissions":["*:VoteApprove"],"vote_policy":{}}
        ],
        "default_vote_policy":{"weight_kind":"RoleWeight","quorum":"2","threshold":"2"},
        "proposal_bond":"100000000000000000000000","proposal_period":"604800000000000",
        "bounty_bond":"0","bounty_forgiveness_period":"86400000000000"
    }))?;
    let dao_id = harness.deploy_sputnik_dao("governance-dao", policy).await?;
    let source = harness.deploy_mock_oracle("governance-source").await?;
    let oracle = harness.deploy_proxy_oracle().await?;
    let feed = PriceIdentifier([0x42; 32]);
    harness
        .admin_set_proxy(
            oracle.clone(),
            feed,
            Some(Proxy::median_low(
                [OracleRequest::pyth(source, feed).into()],
                FreshnessFilter::new(Some(Nanoseconds::from_secs(3600)), None),
            )),
        )
        .await?;
    let governance = harness
        .deploy_governance_contract(oracle.clone(), dao_id.clone())
        .await?;
    let context = || GatewayContext::from_near_client(harness.gateway_client());
    for (create_gas, execute_gas, method_name, is_manually_tripped, child_fails) in [
        (None, None, "admin_set_manual_trip", true, false),
        (
            Some(NearGas::from_tgas(100)),
            Some(NearGas::from_tgas(200)),
            "admin_set_manual_trip",
            false,
            false,
        ),
        (None, None, "missing_governance_target", false, true),
    ] {
        let id = harness
            .client()?
            .read(gov::NextProposalId {
                governance_id: governance.clone(),
            })
            .await?;
        let operation: Operation = serde_json::from_value(json!({"TargetFunctionCall":{
            "method_name":method_name,
            "args":Base64Bytes(serde_json::to_vec(&json!({
                "id":feed,"is_manually_tripped":is_manually_tripped,"metadata":null
            }))?),
            "attached_deposit":"0","gas":"100000000000000"
        }}))?;
        let request = gov::CreateProposal {
            governance_id: governance.clone(),
            id,
            operation: operation.clone(),
            requested_ttl: Nanoseconds::from_secs(1),
            encoding: ProposalEncoding::Json,
            gas: create_gas,
        };
        let dao_count: u64 = harness
            .view_json(&dao_id, "get_last_proposal_id", json!({}))
            .await?;
        let mut impossible = request.clone();
        let Operation::TargetFunctionCall(call) = &mut impossible.operation else {
            unreachable!("fixture uses a target call");
        };
        call.gas = harness
            .gateway_client()
            .chain()
            .protocol_limits()
            .await?
            .max_total_prepaid_gas;
        let rejected = <Dispatch as PlanWrite<gov::CreateProposal, GatewayContext>>::plan(
            WriteRequest {
                signer_account_id: ManagedAccountId(dao_id.clone()),
                idempotency_key: None,
                body: impossible,
            },
            context(),
        )
        .await;
        assert!(matches!(
            rejected,
            Err(GatewayError::RequestPreconditionFailed(_))
        ));
        assert!(harness
            .client()?
            .read(gov::GetProposal {
                governance_id: governance.clone(),
                id,
            })
            .await?
            .is_none());
        assert_eq!(
            harness
                .view_json::<u64>(&dao_id, "get_last_proposal_id", json!({}))
                .await?,
            dao_count,
            "impossible execution must fail before submitting a DAO proposal"
        );
        let create = <Dispatch as PlanWrite<gov::CreateProposal, GatewayContext>>::plan(
            WriteRequest {
                signer_account_id: ManagedAccountId(dao_id.clone()),
                idempotency_key: None,
                body: request,
            },
            context(),
        )
        .await?;
        if let Some(explicit) = create_gas {
            let [Action::FunctionCall(action)] = &create.steps[0].actions[..] else {
                panic!("expected one governance FunctionCall");
            };
            assert_eq!(action.gas, explicit);
        }
        let created =
            execute_governance_plan_through_dao(&harness, &proposer, &council, &dao_id, create)
                .await?;
        assert!(failed_receipts(&created).next().is_none(), "{created:?}");
        assert_eq!(
            harness
                .client()?
                .read(gov::GetProposal {
                    governance_id: governance.clone(),
                    id,
                })
                .await?
                .context("governance proposal created by the DAO")?
                .operation,
            operation,
            "planning and DAO forwarding must not rewrite the approved child gas"
        );
        harness.fast_forward(10).await?;

        // Child gas alone cannot run the wrapper. Reject locally before a DAO proposal exists.
        let before: u64 = harness
            .view_json(&dao_id, "get_last_proposal_id", json!({}))
            .await?;
        for gas in [NearGas::from_gas(0), NearGas::from_tgas(100)] {
            let rejected = <Dispatch as PlanWrite<gov::ExecuteProposal, GatewayContext>>::plan(
                WriteRequest {
                    signer_account_id: ManagedAccountId(dao_id.clone()),
                    idempotency_key: None,
                    body: gov::ExecuteProposal {
                        governance_id: governance.clone(),
                        id,
                        gas: Some(gas),
                    },
                },
                context(),
            )
            .await;
            assert!(matches!(
                rejected,
                Err(GatewayError::RequestPreconditionFailed(_))
            ));
        }
        assert_eq!(
            harness
                .view_json::<u64>(&dao_id, "get_last_proposal_id", json!({}))
                .await?,
            before
        );

        let execute = <Dispatch as PlanWrite<gov::ExecuteProposal, GatewayContext>>::plan(
            WriteRequest {
                signer_account_id: ManagedAccountId(dao_id.clone()),
                idempotency_key: None,
                body: gov::ExecuteProposal {
                    governance_id: governance.clone(),
                    id,
                    gas: execute_gas,
                },
            },
            context(),
        )
        .await?;
        if let Some(explicit) = execute_gas {
            let [Action::FunctionCall(action)] = &execute.steps[0].actions[..] else {
                panic!("expected one governance FunctionCall");
            };
            assert_eq!(action.gas, explicit);
        }
        let executed =
            execute_governance_plan_through_dao(&harness, &proposer, &council, &dao_id, execute)
                .await?;
        if child_fails {
            assert!(
                failed_receipts(&executed).any(|receipt| receipt.contract_id == oracle),
                "a consumed governance proposal must not hide detached target failure: {executed:?}"
            );
        } else {
            assert!(failed_receipts(&executed).next().is_none(), "{executed:?}");
        }
        assert_eq!(
            harness
                .client()?
                .read(proxy_oracle::GetProxyCircuitBreakerSet {
                    oracle_id: oracle.clone(),
                    id: feed,
                })
                .await?
                .context("configured breaker set")?
                .is_manually_tripped(),
            is_manually_tripped
        );
        assert!(harness
            .client()?
            .read(gov::GetProposal {
                governance_id: governance.clone(),
                id,
            })
            .await?
            .is_none());
    }
    Ok(())
}
