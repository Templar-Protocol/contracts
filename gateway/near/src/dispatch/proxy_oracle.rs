use blockchain_gateway_core::proxy_oracle;
use futures::future::BoxFuture;

use crate::{
    actor::DispatchRead,
    client::proxy_oracle::{GetProxyArgs, ListProxiesArgs, PriceFeedExistsArgs},
    GatewayResult, NearClient,
};

impl DispatchRead for proxy_oracle::ListProxies {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
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

impl DispatchRead for proxy_oracle::GetProxy {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            client
                .proxy_oracle(params.oracle_id)
                .get_proxy(GetProxyArgs { id: params.id })
                .await
                .map(|proxy| proxy_oracle::GetProxyResult { proxy })
        })
    }
}

impl DispatchRead for proxy_oracle::PriceFeedExists {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            client
                .proxy_oracle(params.oracle_id)
                .price_feed_exists(PriceFeedExistsArgs {
                    price_identifier: params.price_identifier,
                })
                .await
                .map(|exists| proxy_oracle::PriceFeedExistsResult { exists })
        })
    }
}
