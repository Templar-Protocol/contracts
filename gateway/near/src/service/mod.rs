pub mod chain;
pub mod market;
pub mod registry;
pub mod storage;
pub mod tx;
pub mod universal_account;

use crate::{NearReadClient, NearWriteClient};

#[derive(Clone)]
pub struct GatewayService {
    near: NearReadClient,
    writer: NearWriteClient,
}

impl GatewayService {
    pub fn new(near: NearReadClient, writer: NearWriteClient) -> Self {
        Self { near, writer }
    }

    pub fn near(&self) -> &NearReadClient {
        &self.near
    }

    pub fn writer(&self) -> &NearWriteClient {
        &self.writer
    }
}
