pub mod chain;
pub mod market;
pub mod registry;
pub mod storage;
pub mod universal_account;

use crate::NearReadClient;

#[derive(Debug, Clone)]
pub struct GatewayService {
    near: NearReadClient,
}

impl GatewayService {
    pub fn new(near: NearReadClient) -> Self {
        Self { near }
    }

    pub fn near(&self) -> &NearReadClient {
        &self.near
    }
}
