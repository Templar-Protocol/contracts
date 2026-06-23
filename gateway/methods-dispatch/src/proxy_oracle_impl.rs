use async_trait::async_trait;
use templar_gateway_core::{
    client::proxy_oracle::{GetProxyArgs, ListProxiesArgs, PriceFeedExistsArgs},
    DispatchRead, GatewayResult, HasNearClient,
};
use templar_gateway_methods_spec::proxy_oracle;

use crate::Dispatch;

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
