mod context_impl;
mod oracle_impl;
mod source_provider_impl;

pub struct Dispatch;

pub use context_impl::{GatewayContextBuilderOracleExt, WithPythSource, WithRedStoneSource};
pub use source_provider_impl::{ProvidesPythSource, ProvidesRedStoneSource};
pub use templar_gateway_oracle_pyth::PythHttpClient;
pub use templar_gateway_oracle_redstone::RedStoneBridgeClient;
