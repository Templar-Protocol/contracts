use std::{path::Path, sync::Arc};

use near_api::NetworkConfig;
use templar_gateway_types::ManagedAccountId;
use url::Url;

use crate::{
    client::tx::TxClient, GatewayResult, NearClient, PythHttpClient, RedStoneBridgeClient,
};

#[derive(Debug, Clone)]
pub struct GatewayContext {
    near: NearClient,
    pyth_http: PythHttpClient,
    redstone_bridge: RedStoneBridgeClient,
}

impl GatewayContext {
    pub fn new(
        network: NetworkConfig,
        pyth_hermes_url: Url,
        node_path: &Path,
    ) -> GatewayResult<Self> {
        let near = NearClient::new(network);
        let pyth_http = PythHttpClient::new(pyth_hermes_url);
        let redstone_bridge = RedStoneBridgeClient::new(node_path)?;

        Ok(Self {
            near,
            pyth_http,
            redstone_bridge,
        })
    }

    pub fn near(&self) -> &NearClient {
        &self.near
    }

    pub fn network(&self) -> &NetworkConfig {
        self.near.network()
    }

    pub fn tx(
        &self,
        signer_account_id: ManagedAccountId,
        signer: Arc<near_api::Signer>,
    ) -> TxClient<'_> {
        self.near.tx(signer_account_id, signer)
    }

    pub fn pyth_source(&self) -> &PythHttpClient {
        &self.pyth_http
    }

    pub fn redstone_source(&self) -> &RedStoneBridgeClient {
        &self.redstone_bridge
    }
}
