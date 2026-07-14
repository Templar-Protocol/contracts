use async_trait::async_trait;
use serde::Serialize;
use templar_gateway_core::{
    client::proxy_oracle::{
        GetProxyArgs, GetProxyCircuitBreakerSetArgs, ListProxiesArgs, PriceFeedExistsArgs,
        UpdatePricesArgs,
    },
    client::ContractWriteOptions,
    DispatchRead, GatewayError, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::{proxy_oracle, registry::Deploy};
use templar_gateway_types::version::ProxyOracleVersion;

use crate::{registry_impl::plan_deploy_from_registry, Dispatch};

#[derive(Serialize)]
struct ProxyOracleInitArgs {
    owner_id: Option<near_account_id::AccountId>,
}

/// Refuse an `owner_id` the named version would ignore — the deploy would
/// otherwise succeed and leave the registry as owner.
fn ensure_owner_id_is_honored(body: &proxy_oracle::Create) -> GatewayResult<()> {
    let Some(owner_id) = &body.owner_id else {
        return Ok(());
    };

    let version = ProxyOracleVersion::from_version_key(&body.version_key)
        .map_err(|error| GatewayError::UnsupportedFeature(error.to_string()))?;

    if !version.new_accepts_owner_id() {
        return Err(GatewayError::UnsupportedFeature(format!(
            "proxy oracle {version} cannot seat owner_id {owner_id}: its `new` takes no arguments, \
             so {} would own the oracle. Deploy >= 0.3.0.",
            body.registry_id,
        )));
    }

    Ok(())
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<proxy_oracle::Create, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<proxy_oracle::Create>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ensure_owner_id_is_honored(&body)?;

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
    use templar_gateway_types::NearToken;

    use super::{ensure_owner_id_is_honored, proxy_oracle, ProxyOracleInitArgs};

    fn create(version: &str, owner_id: Option<&str>) -> proxy_oracle::Create {
        proxy_oracle::Create {
            registry_id: "registry.near".parse().unwrap(),
            name: "proxy-oracle-btc".to_string(),
            version_key: format!("templar-proxy-oracle-near-contract@{version}#{:0>64}", "ab"),
            owner_id: owner_id.map(|id| id.parse().unwrap()),
            full_access_keys: None,
            deposit: NearToken::from_near(5),
        }
    }

    #[test]
    fn owner_id_is_honored_from_0_3_0() {
        assert!(ensure_owner_id_is_honored(&create("0.3.0", Some("gov.near"))).is_ok());
    }

    #[test]
    fn owner_id_is_refused_on_an_oracle_too_old_to_honor_it() {
        let error = ensure_owner_id_is_honored(&create("0.2.0", Some("gov.near")))
            .expect_err("0.2.0 cannot honor owner_id");

        let message = error.to_string();
        assert!(message.contains("takes no arguments"), "{message}");
        assert!(message.contains("registry.near"), "{message}");
    }

    /// Nothing for an old `new` to drop, so the guard must not fire.
    #[test]
    fn an_old_oracle_is_allowed_when_no_owner_id_is_named() {
        assert!(ensure_owner_id_is_honored(&create("0.2.0", None)).is_ok());
    }

    #[test]
    fn a_malformed_version_key_is_refused_when_an_owner_is_named() {
        let mut body = create("0.3.0", Some("gov.near"));
        body.version_key = "templar-proxy-oracle-near-contract-0.3.0".to_string();

        assert!(ensure_owner_id_is_honored(&body).is_err());
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
}
