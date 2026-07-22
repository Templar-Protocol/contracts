use std::sync::Arc;

use near_api::NetworkConfig;
use templar_gateway_types::ManagedAccountId;

use crate::{client::tx::TxClient, FinalityPolicy, GatewayResult, HasNearClient, NearClient};

#[derive(Debug, Clone)]
pub struct GatewayContextBuilder<C> {
    context: C,
}

#[derive(Debug, Clone)]
pub struct GatewayContext {
    near: NearClient,
}

impl GatewayContext {
    pub fn new(network: NetworkConfig) -> GatewayResult<Self> {
        Ok(Self {
            near: NearClient::new(network),
        })
    }

    pub fn builder(network: NetworkConfig) -> GatewayContextBuilder<Self> {
        GatewayContextBuilder::new(Self::from_near_client(NearClient::new(network)))
    }

    pub fn from_near_client(near: NearClient) -> Self {
        Self { near }
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
}

impl GatewayContextBuilder<GatewayContext> {
    /// Select the transaction and query finality used by this context.
    #[must_use]
    pub fn finality_policy(mut self, finality_policy: FinalityPolicy) -> Self {
        self.context.near =
            NearClient::with_finality_policy(self.context.near.network().clone(), finality_policy);
        self
    }
}

impl<C> GatewayContextBuilder<C> {
    pub fn new(context: C) -> Self {
        Self { context }
    }

    pub fn map<T>(self, f: impl FnOnce(C) -> T) -> GatewayContextBuilder<T> {
        GatewayContextBuilder {
            context: f(self.context),
        }
    }

    pub fn build(self) -> C {
        self.context
    }
}

impl HasNearClient for GatewayContext {
    fn near_client(&self) -> &NearClient {
        &self.near
    }
}
