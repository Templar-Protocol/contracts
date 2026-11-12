use async_trait::async_trait;
use near_account_id::AccountId;
use near_api::{
    types::{
        crypto::KeyType,
        transaction::{actions::Action, TransactionV0},
    },
    PublicKey,
};
use serde::Serialize;
use templar_gateway_core::{
    client::{
        proxy_governance::{
            GovActionArgs, GovCreateArgs, GovGetArgs, GovGetRolesArgs, GovHasRoleArgs, GovListArgs,
            GovListRoleArgs,
        },
        ContractWriteOptions,
    },
    DispatchRead, GatewayError, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
    PlannedTransaction,
};
use templar_gateway_methods_spec::proxy_oracle_governance;
use templar_gateway_types::{
    common::WriteRequest, ManagedAccountId, NearGas, ProposalEncoding, ProtocolLimits,
    ProxyGovernance,
};
use templar_proxy_oracle_near_governance_common::{
    GovernancePolicy, GovernancePolicyWire, Operation, Proposal, ReflexiveOperation, Role,
    GAS_FOR_SELF_UPGRADE_MIGRATE,
};

use crate::{registry_impl::plan_create_from_registry, Dispatch};

// Conservative planning allowances, not protocol fees or mathematical execution minima.
// The payload coefficients cover storage read/remove/serialization and promise data copying.
// Recalibrate with receipt measurements when governance code or runtime costs change.
// Receipt calibration on the canonical DAO: ordinary governance wrappers burnt <2.3 Tgas;
// a 32 KiB policy create burnt <7 Tgas and execution <3.2 Tgas. The base/buffer allowances
// exceed these samples; they are not a measured population-level 99% success guarantee.
const CREATE_BASE_GAS: u64 = 15_000_000_000_000;
const EXECUTE_BASE_GAS: u64 = 10_000_000_000_000;
const WRAPPER_BUFFER_GAS: u64 = 10_000_000_000_000;
const PAYLOAD_GAS_PER_BYTE: u64 = 100_000_000;
const UPGRADE_DEPLOY_GAS: u64 = 5_000_000_000_000;
const DEPLOY_GAS_PER_BYTE: u64 = 20_000_000;
// Sputnik's FunctionCall execution reserves a 10 Tgas callback. Leave another 10 Tgas
// plus payload-dependent headroom for voting/dispatch. This is a single-call allowance,
// not a guarantee for arbitrary DAO versions, policies, or multi-action proposals.
const DAO_FORWARDING_GAS: u64 = 20_000_000_000_000;
const DAO_FORWARDING_GAS_PER_BYTE: u64 = 50_000_000;

fn gas_error(message: &str) -> GatewayError {
    GatewayError::RequestPreconditionFailed(message.to_owned())
}

fn checked_gas_sum(base: u64, bytes: u64, per_byte: u64) -> GatewayResult<u64> {
    bytes
        .checked_mul(per_byte)
        .and_then(|payload| base.checked_add(payload))
        .ok_or_else(|| gas_error("governance gas budget arithmetic overflow"))
}

fn operation_size(operation: &Operation, limits: &ProtocolLimits) -> GatewayResult<u64> {
    let bytes = u64::try_from(borsh::object_length(operation)?)
        .map_err(|_| gas_error("governance operation size overflow"))?;
    if bytes > limits.max_length_storage_value {
        return Err(gas_error(
            "governance operation exceeds the live storage-value size limit",
        ));
    }
    Ok(bytes)
}

fn child_reservation(operation: &Operation) -> GatewayResult<u64> {
    match operation {
        Operation::TargetFunctionCall(call) => {
            let gas = call.gas.as_gas();
            if gas == 0 {
                return Err(gas_error("governance target call gas must be nonzero"));
            }
            Ok(gas)
        }
        Operation::Reflexive(ReflexiveOperation::SelfUpgrade { .. }) => {
            Ok(GAS_FOR_SELF_UPGRADE_MIGRATE.as_gas())
        }
        Operation::Reflexive(
            ReflexiveOperation::SetReflexiveTtl { .. }
            | ReflexiveOperation::SetTargetDefault { .. }
            | ReflexiveOperation::SetMethodPolicy { .. }
            | ReflexiveOperation::SetRole { .. },
        ) => Ok(0),
    }
}

fn bounded_gas(
    estimate: u64,
    child_gas: u64,
    payload_bytes: u64,
    override_gas: Option<NearGas>,
    limits: &ProtocolLimits,
) -> GatewayResult<NearGas> {
    let gas = override_gas.map_or(estimate, NearGas::as_gas);
    if gas == 0 || gas <= child_gas {
        return Err(gas_error(
            "governance call gas must be nonzero and exceed its child reservation",
        ));
    }
    if gas > limits.max_total_prepaid_gas.as_gas() {
        return Err(gas_error(
            "governance call gas exceeds the live protocol limit",
        ));
    }
    if override_gas.is_none() {
        let forwarded = checked_gas_sum(
            DAO_FORWARDING_GAS,
            payload_bytes,
            DAO_FORWARDING_GAS_PER_BYTE,
        )?
        .checked_add(gas)
        .ok_or_else(|| gas_error("governance forwarding budget arithmetic overflow"))?;
        if forwarded > limits.max_total_prepaid_gas.as_gas() {
            return Err(gas_error(
                "governance default gas plus DAO forwarding allowance exceeds the live protocol limit; reduce the payload or child gas, or explicitly budget a direct call",
            ));
        }
    }
    Ok(NearGas::from_gas(gas))
}

fn execute_gas(
    operation: &Operation,
    override_gas: Option<NearGas>,
    limits: &ProtocolLimits,
) -> GatewayResult<NearGas> {
    let bytes = operation_size(operation, limits)?;
    let child_gas = child_reservation(operation)?;
    let mut estimate = checked_gas_sum(
        EXECUTE_BASE_GAS + WRAPPER_BUFFER_GAS,
        bytes,
        PAYLOAD_GAS_PER_BYTE,
    )?;
    if let Operation::Reflexive(ReflexiveOperation::SelfUpgrade { code, .. }) = operation {
        let code_bytes = match code {
            templar_common::upgrade::UpgradeSource::Code(code) => {
                u64::try_from(code.0.len()).map_err(|_| gas_error("upgrade code size overflow"))?
            }
            templar_common::upgrade::UpgradeSource::GlobalHash(_) => 0,
        };
        estimate = estimate
            .checked_add(checked_gas_sum(
                UPGRADE_DEPLOY_GAS,
                code_bytes,
                DEPLOY_GAS_PER_BYTE,
            )?)
            .ok_or_else(|| gas_error("governance upgrade budget arithmetic overflow"))?;
    }
    estimate = estimate
        .checked_add(child_gas)
        .ok_or_else(|| gas_error("governance child reservation arithmetic overflow"))?;
    bounded_gas(estimate, child_gas, bytes, override_gas, limits)
}

fn create_gas(
    encoded_bytes: usize,
    override_gas: Option<NearGas>,
    limits: &ProtocolLimits,
) -> GatewayResult<NearGas> {
    let bytes =
        u64::try_from(encoded_bytes).map_err(|_| gas_error("governance proposal size overflow"))?;
    let estimate = checked_gas_sum(
        CREATE_BASE_GAS + WRAPPER_BUFFER_GAS,
        bytes,
        PAYLOAD_GAS_PER_BYTE,
    )?;
    bounded_gas(estimate, 0, bytes, override_gas, limits)
}

/// Bound the direct V0 transaction, not just its function-call arguments.
///
/// Planning has no signing key, so reserve the larger supported SECP256K1 key:
/// this is exact for that key type and conservative by 32 bytes for Ed25519.
/// Nearcore's transaction-size limit excludes the signature. A DAO forwarding
/// transaction has its own envelope and must be checked separately.
fn validate_create_transaction_size(
    planned: PlannedTransaction,
    limits: &ProtocolLimits,
) -> GatewayResult<PlannedTransaction> {
    let transaction = TransactionV0 {
        signer_id: planned.signer_account_id.0,
        public_key: PublicKey::empty(KeyType::SECP256K1),
        nonce: 0,
        receiver_id: planned.receiver_id,
        block_hash: near_api::types::CryptoHash([0; 32]),
        actions: planned.actions,
    };
    // Count the canonical encoding without allocating or copying the payload.
    let bytes = u64::try_from(borsh::object_length(&transaction)?)
        .map_err(|_| gas_error("governance transaction size overflow"))?;
    if bytes > limits.max_transaction_size {
        return Err(gas_error(
            "governance proposal transaction exceeds the live transaction size limit with a conservative signing-key envelope",
        ));
    }
    Ok(PlannedTransaction {
        signer_account_id: ManagedAccountId(transaction.signer_id),
        receiver_id: transaction.receiver_id,
        actions: transaction.actions,
        continue_on_failure: planned.continue_on_failure,
    })
}

/// The governance contract's `new(proxy_oracle_id, admin_id, policy)`.
#[derive(Serialize)]
struct GovernanceInitArgs {
    proxy_oracle_id: AccountId,
    admin_id: AccountId,
    policy: GovernancePolicy,
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle_governance::Create, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<proxy_oracle_governance::Create>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let proxy_oracle_governance::Create {
            target,
            proxy_oracle_id,
            admin_id,
            policy,
        } = request.body;

        plan_create_from_registry(
            &ctx,
            request.signer_account_id,
            target,
            serde_json::to_vec(&GovernanceInitArgs {
                proxy_oracle_id,
                admin_id,
                policy,
            })?,
        )
        .await
    }
}

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
    ) -> GatewayResult<GovernancePolicyWire> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .get_governance_policy(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::ListProposals, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::ListProposals,
        ctx: C,
    ) -> GatewayResult<Vec<u32>> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .list_proposals(GovListArgs {
                offset: request.offset,
                count: request.count,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetProposal, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::GetProposal,
        ctx: C,
    ) -> GatewayResult<Option<Proposal<Operation>>> {
        let params = request;
        ctx.near_client()
            .proxy_governance(params.governance_id)
            .get_proposal(GovGetArgs { id: params.id })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetProxyOracleId, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::GetProxyOracleId,
        ctx: C,
    ) -> GatewayResult<AccountId> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .get_proxy_oracle_id(())
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::HasRole, C> for Dispatch {
    async fn dispatch(request: proxy_oracle_governance::HasRole, ctx: C) -> GatewayResult<bool> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .has_role(GovHasRoleArgs {
                account_id: request.account_id,
                role: request.role,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::ListRole, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::ListRole,
        ctx: C,
    ) -> GatewayResult<Vec<AccountId>> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .list_role(GovListRoleArgs {
                role: request.role,
                offset: request.offset,
                count: request.count,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle_governance::GetRoles, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle_governance::GetRoles,
        ctx: C,
    ) -> GatewayResult<Vec<Role>> {
        ctx.near_client()
            .proxy_governance(request.governance_id)
            .get_roles(GovGetRolesArgs {
                account_id: request.account_id,
            })
            .await
    }
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
        let limits = ctx.near_client().chain().protocol_limits().await?;
        // A conservative execution estimate is not a minimum: a later explicit
        // direct-call budget may fit. Reject only a child reservation that leaves
        // no protocol-permitted gas for governance itself.
        if child_reservation(&body.operation)? >= limits.max_total_prepaid_gas.as_gas() {
            return Err(gas_error(
                "governance child reservation leaves no execution gas within the live protocol limit",
            ));
        }
        let options = ContractWriteOptions::new(request.signer_account_id).one_yocto();
        let args = GovCreateArgs {
            id: body.id,
            operation: body.operation,
            requested_ttl: body.requested_ttl,
        };

        let mut transaction = match body.encoding {
            ProposalEncoding::Json => ctx
                .near_client()
                .proxy_governance(governance_id)
                .create_proposal(options, args),
            ProposalEncoding::Borsh => {
                // Uncached: the metadata cache holds for an hour, which would refuse borsh for
                // that long after a governance upgrade.
                let version = ctx
                    .near_client()
                    .contract(governance_id.clone())
                    .version::<ProxyGovernance>()
                    .await?;
                ctx.near_client()
                    .proxy_governance(governance_id)
                    .create_proposal_borsh(options, version, args)
            }
        }?;
        let [Action::FunctionCall(action)] = &mut transaction.actions[..] else {
            return Err(GatewayError::Internal(
                "governance create codec did not produce one FunctionCall".to_owned(),
            ));
        };
        action.gas = create_gas(action.args.len(), body.gas, &limits)?;
        validate_create_transaction_size(transaction, &limits).map(OperationPlan::from)
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
        let proposal = ctx
            .near_client()
            .proxy_governance(request.body.governance_id.clone())
            .get_proposal(GovGetArgs {
                id: request.body.id,
            })
            .await?
            .ok_or_else(|| gas_error("governance proposal does not exist"))?;
        Self::plan_governance_execution(request, &proposal.operation, ctx)
            .await
            .map(OperationPlan::from)
    }
}

impl Dispatch {
    /// Plan execution of a known immutable operation without reading a stored proposal.
    ///
    /// The caller must bind this operation and proposal ID to the earlier create step.
    /// This supports planning before the governance account or proposal exists; it does
    /// not establish that the proposal has been created or is eligible for execution.
    pub async fn plan_governance_execution<C: HasNearClient>(
        request: WriteRequest<proxy_oracle_governance::ExecuteProposal>,
        operation: &Operation,
        ctx: C,
    ) -> GatewayResult<PlannedTransaction> {
        let body = request.body;
        let limits = ctx.near_client().chain().protocol_limits().await?;
        let gas = execute_gas(operation, body.gas, &limits)?;
        ctx.near_client()
            .proxy_governance(body.governance_id)
            .execute_proposal(
                ContractWriteOptions::new(request.signer_account_id)
                    .one_yocto()
                    .gas(gas),
                GovActionArgs { id: body.id },
            )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };

    use near_api::{
        types::{crypto::KeyType, transaction::TransactionV0},
        NetworkConfig, PublicKey,
    };
    use templar_common::upgrade::UpgradeSource;
    use templar_gateway_core::{GatewayContext, GatewayError, PlanWrite};
    use templar_gateway_types::{NearGas, ProtocolLimits};
    use templar_proxy_oracle_near_governance_common::{
        Operation, ReflexiveOperation, Role, GAS_FOR_SELF_UPGRADE_MIGRATE,
    };

    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

    use super::{
        checked_gas_sum, create_gas, execute_gas, proxy_oracle_governance, Dispatch,
        ProposalEncoding, WriteRequest,
    };

    fn operation() -> Operation {
        Operation::Reflexive(ReflexiveOperation::SetRole {
            account_id: "op.near".parse().unwrap(),
            role: Role::Admin,
            set: true,
        })
    }

    fn limits() -> ProtocolLimits {
        ProtocolLimits {
            max_total_prepaid_gas: NearGas::from_tgas(300),
            max_transaction_size: 1_572_864,
            max_length_storage_value: 4_194_304,
            max_length_storage_key: 2_048,
            num_bytes_account: 100,
            num_extra_bytes_record: 40,
        }
    }

    fn target(gas: NearGas, bytes: usize) -> Operation {
        Operation::TargetFunctionCall(
            serde_json::from_value(serde_json::json!({
                "method_name": "admin_set_proxy",
                "args": near_sdk::json_types::Base64VecU8(vec![0; bytes]),
                "attached_deposit": "0",
                "gas": gas,
            }))
            .unwrap(),
        )
    }

    fn upgrade(code: UpgradeSource) -> Operation {
        Operation::Reflexive(ReflexiveOperation::SelfUpgrade {
            code,
            migrate_args: near_sdk::json_types::Base64VecU8(Vec::new()),
        })
    }

    #[test]
    fn target_reservation_is_preserved_and_payload_cost_is_monotonic() {
        let small = target(NearGas::from_tgas(100), 0);
        let large = target(NearGas::from_tgas(100), 65_536);
        let small_gas = execute_gas(&small, None, &limits()).unwrap();
        assert!(small_gas > NearGas::from_tgas(100));
        assert!(execute_gas(&large, None, &limits()).unwrap() > small_gas);
        let more_child = target(NearGas::from_tgas(101), 0);
        assert_eq!(
            execute_gas(&more_child, None, &limits()).unwrap().as_gas() - small_gas.as_gas(),
            NearGas::from_tgas(1).as_gas()
        );
    }

    #[test]
    fn self_upgrade_reserves_migration_and_inline_deployment_headroom() {
        let global = upgrade(UpgradeSource::GlobalHash([7; 32].into()));
        let inline = upgrade(UpgradeSource::Code(near_sdk::json_types::Base64VecU8(
            vec![0; 1_024],
        )));
        let global_gas = execute_gas(&global, None, &limits()).unwrap();
        assert!(global_gas > GAS_FOR_SELF_UPGRADE_MIGRATE);
        assert!(execute_gas(&inline, None, &limits()).unwrap() > global_gas);
        assert!(execute_gas(&operation(), None, &limits()).unwrap() < global_gas);
        let oversized = upgrade(UpgradeSource::Code(near_sdk::json_types::Base64VecU8(
            vec![0; 1_000_000],
        )));
        assert!(execute_gas(&oversized, None, &limits()).is_err());
        assert!(execute_gas(&global, Some(GAS_FOR_SELF_UPGRADE_MIGRATE), &limits()).is_err());
    }

    #[test]
    fn explicit_budget_is_not_mistaken_for_an_estimated_minimum() {
        let operation = target(NearGas::from_tgas(100), 65_536);
        let explicit = NearGas::from_tgas(101);
        assert_eq!(
            execute_gas(&operation, Some(explicit), &limits()).unwrap(),
            explicit
        );
        assert_eq!(
            create_gas(65_536, Some(explicit), &limits()).unwrap(),
            explicit
        );
        for invalid in [
            NearGas::from_gas(0),
            NearGas::from_tgas(100),
            NearGas::from_tgas(301),
        ] {
            assert!(execute_gas(&operation, Some(invalid), &limits()).is_err());
        }
        assert!(create_gas(0, Some(NearGas::from_gas(0)), &limits()).is_err());
        assert!(create_gas(0, Some(NearGas::from_tgas(301)), &limits()).is_err());
        assert!(execute_gas(&target(NearGas::from_gas(0), 0), None, &limits()).is_err());
    }

    #[test]
    fn live_caps_and_forwarding_allowance_are_never_clamped() {
        let operation = target(NearGas::from_tgas(100), 1_024);
        let default = execute_gas(&operation, None, &limits()).unwrap();
        let mut restricted = limits();
        restricted.max_total_prepaid_gas = default;
        assert!(execute_gas(&operation, None, &restricted).is_err());
        // A direct caller may explicitly budget within the cap without DAO forwarding.
        assert_eq!(
            execute_gas(&operation, Some(default), &restricted).unwrap(),
            default
        );
        restricted.max_total_prepaid_gas = NearGas::from_tgas(90);
        assert!(execute_gas(&operation, None, &restricted).is_err());
        assert!(execute_gas(&target(NearGas::from_tgas(300), 0), None, &limits()).is_err());
        assert!(create_gas(1_500_000, None, &restricted).is_err());
        assert!(checked_gas_sum(u64::MAX, 1, 1).is_err());
        assert!(checked_gas_sum(0, u64::MAX, 2).is_err());
    }

    #[test]
    fn storage_bounds_fail_even_with_explicit_gas() {
        let mut restricted = limits();
        let operation = target(NearGas::from_tgas(100), 1_024);
        let size = u64::try_from(borsh::object_length(&operation).unwrap()).unwrap();
        restricted.max_length_storage_value = size;
        assert!(execute_gas(&operation, None, &restricted).is_ok());
        restricted.max_length_storage_value = size - 1;
        assert!(execute_gas(&operation, Some(NearGas::from_tgas(200)), &restricted).is_err());
        assert!(
            create_gas(65_536, None, &limits()).unwrap()
                > create_gas(1_024, None, &limits()).unwrap()
        );
    }

    #[rstest::rstest]
    #[case::json_default(ProposalEncoding::Json, None)]
    #[case::json_override(ProposalEncoding::Json, Some(NearGas::from_tgas(100)))]
    #[case::borsh_default(ProposalEncoding::Borsh, None)]
    #[case::borsh_override(ProposalEncoding::Borsh, Some(NearGas::from_tgas(100)))]
    #[tokio::test]
    async fn create_proposal_checks_complete_transaction_size(
        #[case] encoding: ProposalEncoding,
        #[case] gas: Option<NearGas>,
    ) {
        let server = MockServer::start().await;
        let max_size = Arc::new(AtomicU64::new(limits().max_transaction_size));
        let live_max_size = Arc::clone(&max_size);
        Mock::given(method("POST"))
            .respond_with(move |request: &wiremock::Request| {
                let rpc: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let result = match rpc["method"].as_str().unwrap() {
                    "EXPERIMENTAL_protocol_config" => serde_json::json!({
                        "runtime_config": {
                            "transaction_costs": {"storage_usage_config": {
                                "num_bytes_account": 100, "num_extra_bytes_record": 40
                            }},
                            "wasm_config": {"limit_config": {
                                "max_transaction_size": live_max_size.load(Ordering::Relaxed),
                                "max_total_prepaid_gas": "300000000000000",
                                "max_length_storage_key": 2048,
                                "max_length_storage_value": 4_194_304
                            }}
                        }
                    }),
                    "query" => {
                        assert_eq!(rpc["params"]["method_name"], "contract_source_metadata");
                        serde_json::json!({
                            "block_hash": "11111111111111111111111111111111",
                            "block_height": 1,
                            "logs": [],
                            "result": serde_json::to_vec(&serde_json::json!({
                                "version": "0.3.0", "standards": []
                            })).unwrap()
                        })
                    }
                    unexpected => panic!("unexpected RPC method {unexpected}"),
                };
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0", "id": rpc["id"], "result": result
                }))
            })
            .mount(&server)
            .await;
        let context = GatewayContext::new(NetworkConfig::from_rpc_url(
            "test",
            server.uri().parse().unwrap(),
        ))
        .unwrap();
        let request = WriteRequest {
            signer_account_id: templar_gateway_types::ManagedAccountId(
                "administrator.near".parse().unwrap(),
            ),
            idempotency_key: None,
            body: proxy_oracle_governance::CreateProposal {
                governance_id: "governance.near".parse().unwrap(),
                id: 7,
                operation: target(NearGas::from_tgas(100), 1_024),
                requested_ttl: templar_common::Nanoseconds::zero(),
                encoding,
                gas,
            },
        };
        let plan = <Dispatch as PlanWrite<_, _>>::plan(request.clone(), context.clone())
            .await
            .unwrap();
        let planned = plan.steps.into_iter().next().unwrap();
        // Planning does not know the signing key: use the larger supported key.
        let transaction = TransactionV0 {
            signer_id: planned.signer_account_id.0,
            public_key: PublicKey::empty(KeyType::SECP256K1),
            nonce: 0,
            receiver_id: planned.receiver_id,
            block_hash: near_api::types::CryptoHash([0; 32]),
            actions: planned.actions,
        };
        let complete_size = u64::try_from(borsh::to_vec(&transaction).unwrap().len()).unwrap();
        max_size.store(complete_size, Ordering::Relaxed);
        assert!(
            <Dispatch as PlanWrite<_, _>>::plan(request.clone(), context.clone())
                .await
                .is_ok()
        );
        max_size.store(complete_size - 1, Ordering::Relaxed);
        let error = <Dispatch as PlanWrite<_, _>>::plan(request, context)
            .await
            .expect_err("a transaction one byte over the live cap must fail planning");
        assert!(matches!(error, GatewayError::RequestPreconditionFailed(_)));
    }
}
