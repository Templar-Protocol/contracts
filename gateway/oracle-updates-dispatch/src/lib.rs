mod context_impl;
mod lazer_client;
mod lazer_wire;
mod oracle_impl;
mod pyth_client;
mod redstone_client;
mod source_provider_impl;
mod sources;

pub struct Dispatch;

pub use context_impl::{
    GatewayContextBuilderOracleExt, WithLazerSource, WithPythSource, WithRedStoneSource,
};
pub use lazer_client::{
    LazerClientError, LazerPayloadSource, LazerResult, LazerSourceConfig, LazerSubscriptionConfig,
};
pub use pyth_client::{PythClientError, PythHttpClient, PythResult};
pub use redstone_client::{RedStoneBridgeClient, RedStoneBridgeError, RedStoneResult};
pub use source_provider_impl::{ProvidesLazerSource, ProvidesPythSource, ProvidesRedStoneSource};
pub use sources::{build_oracle_updates_context, OracleSourceConfig, OracleUpdatesContext};
#[cfg(feature = "clap")]
pub use sources::{LazerSourceArgs, OracleSourceArgs, RedStoneSourceArgs};
