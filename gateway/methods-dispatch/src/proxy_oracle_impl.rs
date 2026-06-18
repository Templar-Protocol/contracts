use async_trait::async_trait;
use templar_gateway_core::{
    client::proxy_oracle::{GetProxyArgs, ListProxiesArgs, PriceFeedExistsArgs},
    DispatchRead, GatewayResult, HasNearClient,
};
use templar_gateway_methods_spec::proxy_oracle;
use templar_gateway_types::MethodSpec;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle::ListProxies, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle::ListProxies as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle::ListProxiesResult> {
        ctx.near_client()
            .proxy_oracle(request.params.oracle_id)
            .list_proxies(ListProxiesArgs {
                offset: request.params.offset,
                count: request.params.count,
            })
            .await
            .map(|proxies| proxy_oracle::ListProxiesResult { proxies })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle::GetProxy, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle::GetProxy as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle::GetProxyResult> {
        let params = request.params;
        ctx.near_client()
            .proxy_oracle(params.oracle_id)
            .cached_get_proxy(GetProxyArgs { id: params.id })
            .await
            .map(|proxy| proxy_oracle::GetProxyResult { proxy })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<proxy_oracle::PriceFeedExists, C> for Dispatch {
    async fn dispatch(
        request: <proxy_oracle::PriceFeedExists as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<proxy_oracle::PriceFeedExistsResult> {
        let params = request.params;
        ctx.near_client()
            .proxy_oracle(params.oracle_id)
            .price_feed_exists(PriceFeedExistsArgs {
                price_identifier: params.price_identifier,
            })
            .await
            .map(|exists| proxy_oracle::PriceFeedExistsResult { exists })
    }
}
