use crate::{FinalityPolicy, GatewayResult, HasNearClient, NearClient, NearRpc};

#[derive(Debug, Clone)]
pub struct GatewayContextBuilder<C> {
    context: C,
}

#[derive(Debug, Clone)]
pub struct GatewayContext {
    near: NearClient,
}

impl GatewayContext {
    pub fn new(rpc: impl Into<NearRpc>) -> GatewayResult<Self> {
        Ok(Self {
            near: NearClient::new(rpc),
        })
    }

    pub fn builder(rpc: impl Into<NearRpc>) -> GatewayContextBuilder<Self> {
        GatewayContextBuilder::new(Self::from_near_client(NearClient::new(rpc)))
    }

    pub fn from_near_client(near: NearClient) -> Self {
        Self { near }
    }
}

impl GatewayContextBuilder<GatewayContext> {
    /// Select the transaction and query finality used by this context.
    #[must_use]
    pub fn finality_policy(mut self, finality_policy: FinalityPolicy) -> Self {
        self.context.near =
            NearClient::with_finality_policy(self.context.near.rpc().clone(), finality_policy);
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
