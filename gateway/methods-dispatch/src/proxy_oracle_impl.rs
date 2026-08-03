use async_trait::async_trait;
use near_api::types::transaction::actions::{Action, DeployContractAction, FunctionCallAction};
use serde::Serialize;
use templar_gateway_core::{
    client::proxy_oracle::{
        AdminSetProxyArgs, GetProxyArgs, GetProxyCircuitBreakerSetArgs, ListProxiesArgs,
        PriceFeedExistsArgs, UpdatePricesArgs,
    },
    client::ContractWriteOptions,
    DispatchRead, GatewayError, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::{proxy_oracle, registry::Deploy};
use templar_gateway_types::{NearGas, NearToken, ProxyOracle, ProxyOracleVersion};

use crate::{registry_impl::plan_deploy_from_registry, Dispatch};

#[derive(Serialize)]
struct ProxyOracleInitArgs {
    owner_id: Option<near_account_id::AccountId>,
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle::Create, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<proxy_oracle::Create>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;

        plan_deploy_from_registry(
            &ctx,
            request.signer_account_id,
            Deploy {
                registry_id: body.registry_id,
                name: body.name,
                version_key: body.version_key,
                init_args: serde_json::to_vec(&ProxyOracleInitArgs {
                    owner_id: body.owner_id,
                })?
                .into(),
                full_access_keys: body.full_access_keys,
                deposit: body.deposit,
            },
        )
        .await
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle::UpdatePrices, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<proxy_oracle::UpdatePrices>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .proxy_oracle(body.oracle_id)
            .update_prices(
                ContractWriteOptions::new(request.signer_account_id).tgas(100),
                UpdatePricesArgs {
                    price_ids: body.price_ids,
                },
            )
            .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle::AdminSetProxy, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<proxy_oracle::AdminSetProxy>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .proxy_oracle(body.oracle_id)
            .admin_set_proxy(
                ContractWriteOptions::new(request.signer_account_id).tgas(100),
                AdminSetProxyArgs {
                    id: body.id,
                    proxy: body.proxy,
                },
            )
            .map(OperationPlan::from)
    }
}

fn v0_upgrade_actions(
    signer_id: &near_account_id::AccountId,
    oracle_id: &near_account_id::AccountId,
    version: ProxyOracleVersion,
    wasm: Vec<u8>,
) -> GatewayResult<Vec<Action>> {
    const MIGRATE_ARGS: &[u8] = br#"{"from_version":"v0"}"#;
    const MIGRATE_GAS: NearGas = NearGas::from_tgas(250);

    if signer_id != oracle_id {
        return Err(GatewayError::UnsupportedFeature(format!(
            "migration v0 must be signed by the oracle account {oracle_id}; got {signer_id}"
        )));
    }
    if version != (0, 1, 0) {
        return Err(GatewayError::UnsupportedFeature(format!(
            "proxy oracle {oracle_id} is version {version}; migration v0 requires v0.1.0"
        )));
    }

    Ok(vec![
        Action::DeployContract(DeployContractAction { code: wasm }),
        Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "migrate".to_owned(),
            args: MIGRATE_ARGS.to_vec(),
            gas: MIGRATE_GAS,
            deposit: NearToken::from_yoctonear(0),
        })),
    ])
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle::Upgrade, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<proxy_oracle::Upgrade>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        if request.signer_account_id.0 != body.oracle_id {
            return Err(GatewayError::UnsupportedFeature(format!(
                "migration v0 must be signed by the oracle account {}; got {}",
                body.oracle_id, request.signer_account_id.0
            )));
        }

        let version = ctx
            .near_client()
            .contract(body.oracle_id.clone())
            .version::<ProxyOracle>()
            .await?;
        let actions = match body.migration {
            proxy_oracle::Migration::V0 => v0_upgrade_actions(
                &request.signer_account_id.0,
                &body.oracle_id,
                version,
                body.wasm.0,
            )?,
        };

        Ok(OperationPlan::execute(
            request.signer_account_id,
            body.oracle_id,
            actions,
        ))
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle::ListProxies, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle::ListProxies,
        ctx: C,
    ) -> GatewayResult<proxy_oracle::ListProxiesResult> {
        ctx.near_client()
            .proxy_oracle(request.oracle_id)
            .list_proxies(ListProxiesArgs {
                offset: request.offset,
                count: request.count,
            })
            .await
            .map(|proxies| proxy_oracle::ListProxiesResult { proxies })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle::GetProxy, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle::GetProxy,
        ctx: C,
    ) -> GatewayResult<proxy_oracle::GetProxyResult> {
        let params = request;
        ctx.near_client()
            .proxy_oracle(params.oracle_id)
            .cached_get_proxy(GetProxyArgs { id: params.id })
            .await
            .map(|proxy| proxy_oracle::GetProxyResult { proxy })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle::GetProxyCircuitBreakerSet, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle::GetProxyCircuitBreakerSet,
        ctx: C,
    ) -> GatewayResult<proxy_oracle::GetProxyCircuitBreakerSetResult> {
        ctx.near_client()
            .proxy_oracle(request.oracle_id)
            .get_proxy_circuit_breaker_set(GetProxyCircuitBreakerSetArgs { id: request.id })
            .await
            .map(
                |circuit_breaker_set| proxy_oracle::GetProxyCircuitBreakerSetResult {
                    circuit_breaker_set,
                },
            )
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle::PriceFeedExists, C> for Dispatch {
    async fn dispatch(
        request: proxy_oracle::PriceFeedExists,
        ctx: C,
    ) -> GatewayResult<proxy_oracle::PriceFeedExistsResult> {
        let params = request;
        ctx.near_client()
            .proxy_oracle(params.oracle_id)
            .price_feed_exists(PriceFeedExistsArgs {
                price_identifier: params.price_identifier,
            })
            .await
            .map(|exists| proxy_oracle::PriceFeedExistsResult { exists })
    }
}

#[cfg(test)]
mod tests {
    use near_api::{types::transaction::actions::Action, NetworkConfig};
    use templar_gateway_core::{HasNearClient, NearClient, PlanWrite};
    use templar_gateway_methods_spec::proxy_oracle;
    use templar_gateway_types::{
        common::WriteRequest, Base64Bytes, ManagedAccountId, ProxyOracleVersion,
    };

    use super::{v0_upgrade_actions, Dispatch, ProxyOracleInitArgs};

    #[derive(Clone)]
    struct TestCtx(NearClient);

    impl HasNearClient for TestCtx {
        fn near_client(&self) -> &NearClient {
            &self.0
        }
    }

    /// Must stay an object: a bare `null` payload fails to deserialize into
    /// `new`'s argument struct, where a missing `owner_id` defaults to `None`.
    #[test]
    fn init_args_are_an_object_even_without_an_owner() {
        let args = serde_json::to_value(ProxyOracleInitArgs { owner_id: None }).unwrap();
        assert_eq!(args, serde_json::json!({ "owner_id": null }));

        let args = serde_json::to_value(ProxyOracleInitArgs {
            owner_id: Some("gov.near".parse().unwrap()),
        })
        .unwrap();
        assert_eq!(args, serde_json::json!({ "owner_id": "gov.near" }));
    }

    #[test]
    fn v0_upgrade_actions_refuses_invalid_sources_and_signers() {
        let oracle_id = "oracle.near".parse().unwrap();

        for version in [(0, 0, 9), (0, 2, 0), (0, 3, 0)] {
            let error = v0_upgrade_actions(
                &oracle_id,
                &oracle_id,
                ProxyOracleVersion::from(version),
                vec![],
            )
            .expect_err("v0 migration requires v0.1.0");
            assert!(error.to_string().contains("migration v0 requires v0.1.0"));
        }

        let error = v0_upgrade_actions(
            &"operator.near".parse().unwrap(),
            &oracle_id,
            ProxyOracleVersion::from((0, 3, 0)),
            vec![],
        )
        .expect_err("private migration requires self signer");
        assert!(error
            .to_string()
            .contains("must be signed by the oracle account"));
    }

    #[test]
    fn v0_upgrade_actions_deploys_supplied_wasm_then_migrates() {
        let wasm = b"\0asm\x01\0\0\0proxy-oracle".to_vec();
        let actions = v0_upgrade_actions(
            &"oracle.near".parse().unwrap(),
            &"oracle.near".parse().unwrap(),
            ProxyOracleVersion::from((0, 1, 0)),
            wasm.clone(),
        )
        .expect("v0.1.0 source with self signer");
        assert_eq!(actions.len(), 2);

        match &actions[0] {
            Action::DeployContract(action) => assert_eq!(action.code, wasm),
            action => panic!("expected deploy action, got {action:?}"),
        }
        match &actions[1] {
            Action::FunctionCall(action) => {
                assert_eq!(action.method_name, "migrate");
                assert_eq!(action.args, br#"{"from_version":"v0"}"#);
                assert_eq!(action.gas.as_gas(), 250_000_000_000_000);
                assert_eq!(action.deposit.as_yoctonear(), 0);
            }
            action => panic!("expected function-call action, got {action:?}"),
        }
    }

    #[tokio::test]
    async fn upgrade_rejects_wrong_signer_before_metadata_view() {
        let request = WriteRequest {
            signer_account_id: ManagedAccountId("operator.near".parse().unwrap()),
            idempotency_key: None,
            body: proxy_oracle::Upgrade {
                oracle_id: "oracle.near".parse().unwrap(),
                wasm: Base64Bytes(vec![]),
                migration: proxy_oracle::Migration::V0,
            },
        };
        let ctx = TestCtx(NearClient::new(NetworkConfig::from_rpc_url(
            "test",
            "http://127.0.0.1:1".parse().unwrap(),
        )));

        let error = <Dispatch as PlanWrite<proxy_oracle::Upgrade, TestCtx>>::plan(request, ctx)
            .await
            .expect_err("wrong signer must be rejected before the unreachable metadata RPC");
        assert!(error
            .to_string()
            .contains("must be signed by the oracle account"));
    }
}
