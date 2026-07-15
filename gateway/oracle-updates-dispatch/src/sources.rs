//! In-process oracle payload source configuration and the layered context an
//! oracle-updates client/dispatch plans against.

use std::path::PathBuf;

use templar_gateway_core::{GatewayContext, GatewayContextBuilder, GatewayResult};

use crate::{
    GatewayContextBuilderOracleExt as _, LazerSourceConfig, WithLazerSource, WithPythSource,
    WithRedStoneSource,
};

/// The layered gateway context: the base context plus the in-process Pyth,
/// RedStone, and Lazer payload sources.
pub type OracleUpdatesContext = WithLazerSource<WithRedStoneSource<WithPythSource<GatewayContext>>>;

/// Configuration for the in-process oracle payload sources fetched from when
/// planning `oracle.*` updates.
#[derive(Debug, Clone)]
pub struct OracleSourceConfig {
    /// Pyth Hermes API URL.
    pub pyth_hermes_url: url::Url,
    /// Path to the Node.js interpreter (or equivalent) that runs the RedStone bridge.
    pub redstone_node_path: PathBuf,
    /// Pyth Pro/Lazer websocket source configuration.
    pub lazer: LazerSourceConfig,
}

/// Layer the in-process payload sources onto `base`, producing the context an
/// oracle-updates client plans against. Layering onto a clone of an existing
/// base context lets the methods and oracle-updates clients share the same
/// `Arc`-backed NEAR read cache.
pub fn build_oracle_updates_context(
    base: GatewayContext,
    sources: OracleSourceConfig,
) -> GatewayResult<OracleUpdatesContext> {
    Ok(GatewayContextBuilder::new(base)
        .with_pyth_source(sources.pyth_hermes_url)
        .with_redstone_source(&sources.redstone_node_path)?
        .with_lazer_source(sources.lazer)
        .build())
}

#[cfg(feature = "clap")]
pub use args::{LazerSourceArgs, OracleSourceArgs, RedStoneSourceArgs};

#[cfg(feature = "clap")]
mod args {
    use std::{path::PathBuf, time::Duration};

    use clap::Args;
    use templar_gateway_core::RedactedString;
    use url::Url;

    use super::OracleSourceConfig;
    use crate::{LazerSourceConfig, LazerSubscriptionConfig};

    /// CLI surface for the in-process RedStone bridge payload source.
    #[derive(Args, Debug, Clone)]
    pub struct RedStoneSourceArgs {
        /// Path to the Node.js interpreter (or equivalent) that runs the RedStone bridge.
        #[arg(
            long = "redstone-node-path",
            env = "REDSTONE_NODE_PATH",
            default_value = "node"
        )]
        pub redstone_node_path: PathBuf,
    }

    /// CLI surface for the in-process Pyth Pro/Lazer websocket payload source.
    #[derive(Args, Debug, Clone)]
    pub struct LazerSourceArgs {
        /// Bearer token for Pyth Pro/Lazer websocket payload updates.
        ///
        /// Optional at parse time so a consumer that never builds a Lazer source needn't
        /// supply it; [`LazerSourceArgs::build`] rejects its absence.
        #[arg(long = "pyth-lazer-api-key", env = "PYTH_LAZER_API_KEY")]
        pub pyth_lazer_api_key: Option<RedactedString>,

        /// Pyth Pro/Lazer websocket endpoint. Configures one endpoint only; automatic
        /// multi-endpoint failover is not implemented.
        #[arg(
            long = "pyth-lazer-ws-url",
            env = "PYTH_LAZER_WS_URL",
            default_value = "wss://pyth-lazer-0.dourolabs.app/v1/stream"
        )]
        pub pyth_lazer_ws_url: Url,

        /// Pyth Pro/Lazer websocket channel. One of: "real_time", "fixed_rate@50ms",
        /// "fixed_rate@200ms", "fixed_rate@1000ms". Validated when the source is built.
        #[arg(
            long = "pyth-lazer-channel",
            env = "PYTH_LAZER_CHANNEL",
            default_value = "fixed_rate@200ms"
        )]
        pub pyth_lazer_channel: String,

        /// Maximum age, in milliseconds, for cached Pyth Pro/Lazer payloads.
        #[arg(
            long = "pyth-lazer-max-payload-age-ms",
            env = "PYTH_LAZER_MAX_PAYLOAD_AGE_MS",
            default_value = "5000"
        )]
        pub pyth_lazer_max_payload_age_ms: u64,
    }

    impl LazerSourceArgs {
        /// Validate and assemble the runtime [`LazerSourceConfig`].
        ///
        /// # Errors
        /// Returns an error when the API token is absent, or when the websocket
        /// configuration is invalid (empty token, non-`wss://` URL, unsupported channel,
        /// or zero payload age).
        pub fn build(&self) -> anyhow::Result<LazerSourceConfig> {
            let api_token = self.pyth_lazer_api_key.clone().ok_or_else(|| {
                anyhow::anyhow!("missing --pyth-lazer-api-key (or PYTH_LAZER_API_KEY)")
            })?;
            LazerSourceConfig::new(
                self.pyth_lazer_ws_url.clone(),
                api_token,
                LazerSubscriptionConfig {
                    channel: self.pyth_lazer_channel.clone(),
                    max_payload_age: Duration::from_millis(self.pyth_lazer_max_payload_age_ms),
                },
            )
            .map_err(anyhow::Error::from)
        }
    }

    /// Every in-process oracle payload source at once. Flatten it into a consumer's
    /// `clap` configuration and call [`OracleSourceArgs::build`]; flatten a single
    /// member instead when only one source is reachable. The Pyth Hermes URL is inline
    /// because no consumer builds a Hermes source on its own today — only because
    /// `oracle.updatePyth` takes its VAA from the request body instead of fetching it,
    /// which ENG-462 fixes. Expect a `PythSourceArgs` sibling then.
    #[derive(Args, Debug, Clone)]
    pub struct OracleSourceArgs {
        /// Pyth Hermes API URL. See: <https://docs.pyth.network/price-feeds/core/api-reference>
        #[arg(
            long = "pyth-hermes-url",
            env = "PYTH_HERMES_URL",
            default_value = "https://hermes-beta.pyth.network"
        )]
        pub pyth_hermes_url: Url,
        #[command(flatten)]
        pub redstone: RedStoneSourceArgs,
        #[command(flatten)]
        pub lazer: LazerSourceArgs,
    }

    impl OracleSourceArgs {
        /// Validate and assemble the runtime [`OracleSourceConfig`].
        ///
        /// # Errors
        /// Returns an error if the Lazer websocket configuration is invalid — see
        /// [`LazerSourceArgs::build`].
        pub fn build(&self) -> anyhow::Result<OracleSourceConfig> {
            Ok(OracleSourceConfig {
                pyth_hermes_url: self.pyth_hermes_url.clone(),
                redstone_node_path: self.redstone.redstone_node_path.clone(),
                lazer: self.lazer.build()?,
            })
        }
    }
}
