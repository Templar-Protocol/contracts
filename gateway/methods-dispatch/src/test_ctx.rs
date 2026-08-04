use near_api::{NetworkConfig, RPCEndpoint};
use templar_gateway_core::{HasNearClient, NearClient};

#[derive(Clone)]
pub struct TestCtx(pub NearClient);

impl HasNearClient for TestCtx {
    fn near_client(&self) -> &NearClient {
        &self.0
    }
}

const CLOSED_PORT: &str = "http://127.0.0.1:1";

/// Points at a closed port, so any path that touches the network fails.
pub fn offline_ctx() -> TestCtx {
    let mut network =
        NetworkConfig::from_rpc_url("test", CLOSED_PORT.parse().expect("valid rpc url"));
    // One retry, not the default five: the backoff would otherwise spend ~310ms per test waiting
    // out connection-refused on a port nothing is listening to.
    network.rpc_endpoints =
        vec![RPCEndpoint::new(CLOSED_PORT.parse().expect("valid rpc url")).with_retries(1)];
    TestCtx(NearClient::new(network))
}
