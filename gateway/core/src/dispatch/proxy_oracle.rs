use futures::future::BoxFuture;
use templar_gateway_types::proxy_oracle;

use crate::DispatchRead;
use crate::{
    client::proxy_oracle::{GetProxyArgs, ListProxiesArgs, PriceFeedExistsArgs},
    GatewayResult, HasNearClient,
};

impl<C: HasNearClient> DispatchRead<C> for proxy_oracle::ListProxies {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            ctx.near_client()
                .proxy_oracle(request.params.oracle_id)
                .list_proxies(ListProxiesArgs {
                    offset: request.params.offset,
                    count: request.params.count,
                })
                .await
                .map(|proxies| proxy_oracle::ListProxiesResult { proxies })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for proxy_oracle::GetProxy {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            ctx.near_client()
                .proxy_oracle(params.oracle_id)
                .cached_get_proxy(GetProxyArgs { id: params.id })
                .await
                .map(|proxy| proxy_oracle::GetProxyResult { proxy })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for proxy_oracle::PriceFeedExists {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            ctx.near_client()
                .proxy_oracle(params.oracle_id)
                .price_feed_exists(PriceFeedExistsArgs {
                    price_identifier: params.price_identifier,
                })
                .await
                .map(|exists| proxy_oracle::PriceFeedExistsResult { exists })
        })
    }
}
