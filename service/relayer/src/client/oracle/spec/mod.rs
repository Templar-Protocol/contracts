use std::time::Duration;

use near_primitives::action::Action;

mod pyth;
pub use pyth::*;
mod redstone;
pub use redstone::*;

pub trait Spec: Send + Sync + 'static {
    type PriceIdentifier: std::hash::Hash + std::fmt::Debug + std::cmp::Eq + Clone + Send + Sync;
    type Error: std::error::Error + 'static + Send + Sync;

    fn name() -> &'static str;
    fn refresh(&self) -> Duration;
    fn oracle_id(&self) -> &near_sdk::AccountIdRef;
    fn update_actions(
        &self,
        price_ids: &[Self::PriceIdentifier],
    ) -> impl std::future::Future<Output = Result<Vec<Action>, Self::Error>> + Send + Sync;
}
