// SPDX-License-Identifier: MIT
//! Concrete swap provider enum for dynamic dispatch.
//!
//! Since the `SwapProvider` trait has generic methods, it cannot be made into
//! a trait object. This module provides a concrete enum that can be used
//! for dynamic dispatch while maintaining type safety.

use near_primitives::views::FinalExecutionStatus;
use near_sdk::json_types::U128;
use templar_common::asset::{AssetClass, FungibleAsset};

use crate::rpc::AppResult;

use super::{intents::IntentsSwap, rhea::RheaSwap, SwapProvider};

/// Concrete swap provider implementation that can be used for dynamic dispatch.
///
/// This enum wraps all supported swap providers and implements `SwapProvider`,
/// allowing it to be used where dynamic dispatch is needed.
#[derive(Debug, Clone)]
pub enum SwapProviderImpl {
    /// Rhea Finance DEX provider
    Rhea(RheaSwap),
    /// NEAR Intents cross-chain provider
    Intents(IntentsSwap),
}

impl SwapProviderImpl {
    /// Creates a Rhea swap provider variant.
    pub fn rhea(provider: RheaSwap) -> Self {
        Self::Rhea(provider)
    }

    /// Creates a NEAR Intents provider variant.
    pub fn intents(provider: IntentsSwap) -> Self {
        Self::Intents(provider)
    }
}

#[async_trait::async_trait]
impl SwapProvider for SwapProviderImpl {
    async fn quote<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: U128,
    ) -> AppResult<U128> {
        match self {
            Self::Rhea(provider) => provider.quote(from_asset, to_asset, output_amount).await,
            Self::Intents(provider) => provider.quote(from_asset, to_asset, output_amount).await,
        }
    }

    async fn swap<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        amount: U128,
    ) -> AppResult<FinalExecutionStatus> {
        match self {
            Self::Rhea(provider) => provider.swap(from_asset, to_asset, amount).await,
            Self::Intents(provider) => provider.swap(from_asset, to_asset, amount).await,
        }
    }

    fn provider_name(&self) -> &'static str {
        match self {
            Self::Rhea(provider) => provider.provider_name(),
            Self::Intents(provider) => provider.provider_name(),
        }
    }

    fn supports_assets<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
    ) -> bool {
        match self {
            Self::Rhea(provider) => provider.supports_assets(from_asset, to_asset),
            Self::Intents(provider) => provider.supports_assets(from_asset, to_asset),
        }
    }
}
