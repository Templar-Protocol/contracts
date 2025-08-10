use clap::ValueEnum;
use near_sdk::{AccountId, json_types::U128};

use crate::near::RpcResult;

mod intents;
mod rhea;

pub use intents::*;
pub use rhea::*;

pub trait QuoteOutput: Send + Sync {
    /// Converts the quote output to a `U128` value.
    fn to_u128(&self) -> U128;
}

#[async_trait::async_trait]
pub trait Swap {
    type QuoteOutput: QuoteOutput;
    type SwapOutput: Send + Sync;

    /// Quotes the amount of `from` token to `to` token.
    async fn quote(
        &self,
        from: &AccountId,
        to: &AccountId,
        amount: U128,
    ) -> RpcResult<Self::QuoteOutput>;

    /// Swaps `from` token to `to` token.
    async fn swap(
        &self,
        from: &AccountId,
        to: &AccountId,
        amount: U128,
    ) -> RpcResult<Self::SwapOutput>;
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SwapType {
    RheaSwap,
    NearIntents,
}
