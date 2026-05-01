mod context_impl;
mod oracle_impl;
mod pyth_client;
mod redstone_client;
mod source_provider_impl;

pub struct Dispatch;

pub use context_impl::{GatewayContextBuilderOracleExt, WithPythSource, WithRedStoneSource};
pub use pyth_client::{PythClientError, PythHttpClient, PythResult};
pub use redstone_client::{RedStoneBridgeClient, RedStoneBridgeError, RedStoneResult};
pub use source_provider_impl::{ProvidesPythSource, ProvidesRedStoneSource};
