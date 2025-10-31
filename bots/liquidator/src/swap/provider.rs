// SPDX-License-Identifier: MIT
//! Concrete swap provider enum for dynamic dispatch.
//!
//! Since the `SwapProvider` trait has generic methods, it cannot be made into
//! a trait object. This module provides a concrete enum that can be used
//! for dynamic dispatch while maintaining type safety.

use near_primitives::views::FinalExecutionStatus;
use near_sdk::{json_types::U128, AccountId};
use templar_common::asset::{AssetClass, FungibleAsset};

use crate::rpc::AppResult;

use super::{oneclick::OneClickSwap, rhea::RheaSwap, SwapProvider};

/// Concrete swap provider implementation that can be used for dynamic dispatch.
///
/// This enum wraps all supported swap providers and implements `SwapProvider`,
/// allowing it to be used where dynamic dispatch is needed.
#[derive(Debug, Clone)]
pub enum SwapProviderImpl {
    /// Rhea Finance DEX provider
    Rhea(RheaSwap),
    /// 1-Click API provider (recommended)
    OneClick(OneClickSwap),
}

impl SwapProviderImpl {
    /// Creates a Rhea swap provider variant.
    pub fn rhea(provider: RheaSwap) -> Self {
        Self::Rhea(provider)
    }

    /// Creates a 1-Click API provider variant (recommended).
    pub fn oneclick(provider: OneClickSwap) -> Self {
        Self::OneClick(provider)
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
            Self::OneClick(provider) => provider.quote(from_asset, to_asset, output_amount).await,
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
            Self::OneClick(provider) => provider.swap(from_asset, to_asset, amount).await,
        }
    }

    fn provider_name(&self) -> &'static str {
        match self {
            Self::Rhea(provider) => provider.provider_name(),
            Self::OneClick(provider) => provider.provider_name(),
        }
    }

    fn supports_assets<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
    ) -> bool {
        match self {
            Self::Rhea(provider) => provider.supports_assets(from_asset, to_asset),
            Self::OneClick(provider) => provider.supports_assets(from_asset, to_asset),
        }
    }

    async fn ensure_storage_registration<F: AssetClass>(
        &self,
        token_contract: &FungibleAsset<F>,
        account_id: &AccountId,
    ) -> AppResult<()> {
        match self {
            Self::Rhea(provider) => {
                provider
                    .ensure_storage_registration(token_contract, account_id)
                    .await
            }
            Self::OneClick(provider) => {
                provider
                    .ensure_storage_registration(token_contract, account_id)
                    .await
            }
        }
    }
}
