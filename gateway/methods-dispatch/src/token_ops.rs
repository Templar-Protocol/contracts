//! Shared transaction-plan building blocks for asset deposit flows.
//!
//! Several dispatchers (market, vault, …) plan a deposit as an optional storage
//! registration followed by a standard-agnostic `transfer_call` of the asset.
//! These helpers capture that plumbing in one place so it doesn't drift between
//! contract dispatchers.

use near_account_id::AccountId;
use serde::Serialize;
use templar_common::asset::{AssetClass, FungibleAsset};
use templar_gateway_core::{
    client::{
        storage::{StorageBalanceBoundsView, StorageBalanceOfArgs, StorageDepositArgs},
        ContractWriteOptions,
    },
    GatewayResult, HasNearClient, PlannedTransaction,
};
use templar_gateway_types::{ManagedAccountId, NearToken};

/// Plan a registration `storage_deposit` for `account_id` on `contract_id` when
/// the contract implements storage management and the account is not yet
/// registered. Returns `None` when no registration transaction is required.
pub(crate) async fn ensure_storage_registration<C: HasNearClient>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    contract_id: AccountId,
    account_id: AccountId,
) -> GatewayResult<Option<PlannedTransaction>> {
    let Some(bounds) = storage_balance_bounds_if_supported(ctx, contract_id.clone()).await? else {
        return Ok(None);
    };

    let balance = ctx
        .near_client()
        .storage(contract_id.clone())
        .storage_balance_of(StorageBalanceOfArgs {
            account_id: account_id.clone(),
        })
        .await?;

    if balance.is_some() {
        return Ok(None);
    }

    let tx_result = ctx.near_client().storage(contract_id).storage_deposit(
        ContractWriteOptions::new(signer_account_id)
            .tgas(100)
            .deposit(NearToken::from_yoctonear(bounds.min.as_yoctonear())),
        StorageDepositArgs {
            account_id: Some(account_id),
            registration_only: true,
        },
    )?;
    Ok(Some(tx_result))
}

/// Fetch a contract's storage-balance bounds, or `None` if it does not implement
/// storage management.
pub(crate) async fn storage_balance_bounds_if_supported<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
) -> GatewayResult<Option<StorageBalanceBoundsView>> {
    ctx.near_client()
        .storage(contract_id)
        .cached_storage_balance_bounds_if_supported()
        .await
}

/// Plan a `transfer_call` of `amount` of `asset` to `receiver_id`, carrying
/// `msg` as the JSON-encoded deposit message. The `token()` client dispatches to
/// `ft_transfer_call` or `mt_transfer_call` depending on the asset's token
/// standard (NEP-141 vs NEP-245), so this is not FT-specific.
///
/// Uses the default execution-status wait (`ExecutedOptimistic`), which already
/// covers the full `transfer_call` receipt chain.
pub(crate) fn transfer_call_asset<C, T, M>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    asset: FungibleAsset<T>,
    receiver_id: AccountId,
    amount: impl Into<u128>,
    msg: &M,
) -> GatewayResult<PlannedTransaction>
where
    C: HasNearClient,
    T: AssetClass,
    M: Serialize,
{
    ctx.near_client().token(asset).transfer_call(
        ContractWriteOptions::new(signer_account_id)
            .tgas(300)
            .one_yocto(),
        receiver_id,
        amount,
        serde_json::to_string(msg)?,
    )
}
