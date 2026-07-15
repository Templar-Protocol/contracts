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

struct StorageStatus {
    min_yocto: u128,
    registered: bool,
    available_yocto: u128,
}

/// Shared prelude of the `ensure_storage_*` helpers: one bounds + one balance
/// fetch. `None` when the contract does not implement storage management.
async fn storage_status<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
    account_id: AccountId,
) -> GatewayResult<Option<StorageStatus>> {
    let Some(bounds) = storage_balance_bounds_if_supported(ctx, contract_id.clone()).await? else {
        return Ok(None);
    };

    let balance = ctx
        .near_client()
        .storage(contract_id)
        .storage_balance_of(StorageBalanceOfArgs { account_id })
        .await?;

    Ok(Some(StorageStatus {
        min_yocto: bounds.min.as_yoctonear(),
        registered: balance.is_some(),
        available_yocto: balance.map_or(0, |balance| balance.available.as_yoctonear()),
    }))
}

/// Register `account_id` on `contract_id` if absent. Gates on presence, correct
/// where the minimum is consumed by the account entry itself (e.g. an FT slot);
/// use [`ensure_storage_headroom`] where the contract spends further storage from
/// the balance.
pub(crate) async fn ensure_storage_registration<C: HasNearClient>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    contract_id: AccountId,
    account_id: AccountId,
) -> GatewayResult<Option<PlannedTransaction>> {
    let Some(status) = storage_status(ctx, contract_id.clone(), account_id.clone()).await? else {
        return Ok(None);
    };
    if status.registered {
        return Ok(None);
    }

    Ok(Some(plan_storage_deposit(
        ctx,
        signer_account_id,
        contract_id,
        account_id,
        status.min_yocto,
        true,
    )?))
}

/// Top `account_id`'s *available* balance on `contract_id` up to the minimum
/// (also registering a fresh account, so it subsumes registration here). The
/// market charges per-position storage from this balance and its minimum covers
/// one position, so a signer who supplied first could not otherwise collateralize
/// without a top-up.
pub(crate) async fn ensure_storage_headroom<C: HasNearClient>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    contract_id: AccountId,
    account_id: AccountId,
) -> GatewayResult<Option<PlannedTransaction>> {
    let Some(status) = storage_status(ctx, contract_id.clone(), account_id.clone()).await? else {
        return Ok(None);
    };

    let deficit = status.min_yocto.saturating_sub(status.available_yocto);
    if deficit == 0 {
        return Ok(None);
    }

    // `registration_only: false` credits the whole deposit to `available` instead
    // of refunding the excess above the bare registration min.
    Ok(Some(plan_storage_deposit(
        ctx,
        signer_account_id,
        contract_id,
        account_id,
        deficit,
        false,
    )?))
}

/// Shared tail of the `ensure_storage_*` helpers.
fn plan_storage_deposit<C: HasNearClient>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    contract_id: AccountId,
    account_id: AccountId,
    deposit_yocto: u128,
    registration_only: bool,
) -> GatewayResult<PlannedTransaction> {
    ctx.near_client().storage(contract_id).storage_deposit(
        ContractWriteOptions::new(signer_account_id)
            .tgas(100)
            .deposit(NearToken::from_yoctonear(deposit_yocto)),
        StorageDepositArgs {
            account_id: Some(account_id),
            registration_only,
        },
    )
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
