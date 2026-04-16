pub mod actor;
pub mod client;
mod dispatch;
pub mod error;
pub mod service;

pub use actor::ManagedSigner;
pub use client::NearClient;
pub use error::{GatewayError, GatewayResult};
pub use service::GatewayService;
