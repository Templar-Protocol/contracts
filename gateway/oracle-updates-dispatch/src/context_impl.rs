use std::path::Path;

use templar_gateway_core::{GatewayContextBuilder, GatewayError, HasNearClient, NearClient};
use url::Url;

use crate::{ProvidesPythSource, ProvidesRedStoneSource, PythHttpClient, RedStoneBridgeClient};

#[derive(Debug, Clone)]
pub struct WithPythSource<C> {
    inner: C,
    pyth_source: PythHttpClient,
}

#[derive(Debug, Clone)]
pub struct WithRedStoneSource<C> {
    inner: C,
    redstone_source: RedStoneBridgeClient,
}

pub trait GatewayContextBuilderOracleExt<C>: Sized {
    fn with_pyth_source(self, pyth_hermes_url: Url) -> GatewayContextBuilder<WithPythSource<C>>;

    fn with_redstone_source(
        self,
        redstone_node_path: impl AsRef<Path>,
    ) -> Result<GatewayContextBuilder<WithRedStoneSource<C>>, GatewayError>;
}

impl<C> GatewayContextBuilderOracleExt<C> for GatewayContextBuilder<C> {
    fn with_pyth_source(self, pyth_hermes_url: Url) -> GatewayContextBuilder<WithPythSource<C>> {
        self.map(|inner| WithPythSource {
            inner,
            pyth_source: PythHttpClient::new(pyth_hermes_url),
        })
    }

    fn with_redstone_source(
        self,
        redstone_node_path: impl AsRef<Path>,
    ) -> Result<GatewayContextBuilder<WithRedStoneSource<C>>, GatewayError> {
        let redstone_source = RedStoneBridgeClient::new(redstone_node_path.as_ref())
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        Ok(self.map(|inner| WithRedStoneSource {
            inner,
            redstone_source,
        }))
    }
}

impl<C: HasNearClient> HasNearClient for WithPythSource<C> {
    fn near_client(&self) -> &NearClient {
        self.inner.near_client()
    }
}

impl<C> ProvidesPythSource for WithPythSource<C> {
    type PythSource = PythHttpClient;

    fn pyth_source(&self) -> &Self::PythSource {
        &self.pyth_source
    }
}

impl<C: HasNearClient> HasNearClient for WithRedStoneSource<C> {
    fn near_client(&self) -> &NearClient {
        self.inner.near_client()
    }
}

impl<C: ProvidesPythSource> ProvidesPythSource for WithRedStoneSource<C> {
    type PythSource = C::PythSource;

    fn pyth_source(&self) -> &Self::PythSource {
        self.inner.pyth_source()
    }
}

impl<C> ProvidesRedStoneSource for WithRedStoneSource<C> {
    type RedStoneSource = RedStoneBridgeClient;

    fn redstone_source(&self) -> &Self::RedStoneSource {
        &self.redstone_source
    }
}
