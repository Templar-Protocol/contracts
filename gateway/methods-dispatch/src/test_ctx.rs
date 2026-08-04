use near_api::NetworkConfig;
use templar_gateway_core::{HasNearClient, NearClient};

#[derive(Clone)]
pub struct TestCtx(NearClient);

impl HasNearClient for TestCtx {
    fn near_client(&self) -> &NearClient {
        &self.0
    }
}

/// Points at a closed port, so any path that touches the network fails.
pub fn offline_ctx() -> TestCtx {
    let mut network =
        NetworkConfig::from_rpc_url("test", "http://127.0.0.1:1".parse().expect("valid rpc url"));
    // The default five attempts spend ~310ms of backoff per test on connection-refused.
    network.rpc_endpoints = network
        .rpc_endpoints
        .into_iter()
        .map(|endpoint| endpoint.with_retries(1))
        .collect();
    TestCtx(NearClient::new(network))
}
