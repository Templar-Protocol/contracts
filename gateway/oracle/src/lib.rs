mod context;
mod dispatch;
mod source_provider;

pub use context::{GatewayContextBuilderOracleExt, WithPythSource, WithRedStoneSource};
pub use dispatch::Dispatch;
pub use source_provider::{ProvidesPythSource, ProvidesRedStoneSource};
pub use templar_gateway_oracle_pyth::PythHttpClient;
pub use templar_gateway_oracle_redstone::RedStoneBridgeClient;

pub mod prelude {
    pub use crate::GatewayContextBuilderOracleExt;
}
