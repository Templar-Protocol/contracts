use std::time::Duration;

use near_sdk::AccountId;
use templar_gateway_client::SigningClient;
use templar_gateway_types::CryptoHash;

use super::UpdateError;

mod pyth;
pub use pyth::*;
mod redstone;
pub use redstone::*;

pub trait Spec: Send + Sync + 'static {
    type FeedId: std::hash::Hash + std::fmt::Debug + std::cmp::Eq + Clone + Send + Sync;

    fn name() -> &'static str;
    fn refresh(&self) -> Duration;

    /// Fetch the off-chain update payload for `feed_ids` and submit it on-chain
    /// through the gateway, returning the resulting tx hash (or `None` when
    /// nothing was sent). The gateway owns signing, submission, and finality.
    fn execute_update(
        &self,
        gateway: &SigningClient,
        oracle_id: AccountId,
        feed_ids: &[Self::FeedId],
    ) -> impl std::future::Future<Output = Result<Option<CryptoHash>, UpdateError>> + Send;
}
