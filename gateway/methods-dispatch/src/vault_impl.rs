use async_trait::async_trait;
use templar_common::{
    vault::{self as common_vault, DepositMsg, VaultConfiguration},
    SU64,
};
use templar_gateway_core::{
    client::{
        vault::{
            AccountArg, AllocatorArg, CapArgs, CapGroupUpdateArg, CapGroupUpdateKeyArg,
            CapMarketArg, DeltaArg, ExecuteMarketWithdrawalArgs, ExecuteRebalanceWithdrawalArgs,
            ExecuteWithdrawalArgs, FeesArg, MarketAccountArg, MarketIdArg, MarketsArg, RedeemArgs,
            RestrictionsArg, SubmitTimelockArgs, TokenArg, U128AssetsArg, U128SharesArg,
            WithdrawArgs,
        },
        ContractWriteOptions,
    },
    DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::vault;
use templar_primitives::SU128;

use crate::token_ops::{ensure_storage_registration, transfer_call_asset};
use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::GetConfiguration, C> for Dispatch {
    async fn dispatch(
        request: vault::GetConfiguration,
        ctx: C,
    ) -> GatewayResult<VaultConfiguration> {
        ctx.near_client()
            .vault(request.vault_id)
            .get_configuration(())
            .await
    }
}

macro_rules! passthrough_read {
    ($spec:ty, $method:ident, $output:ty) => {
        #[async_trait]
        impl<C: HasNearClient> DispatchRead<$spec, C> for Dispatch {
            async fn dispatch(request: $spec, ctx: C) -> GatewayResult<$output> {
                ctx.near_client().vault(request.vault_id).$method(()).await
            }
        }
    };
}

passthrough_read!(vault::GetTotalAssets, get_total_assets, SU128);
passthrough_read!(vault::GetLastTotalAssets, get_last_total_assets, SU128);
passthrough_read!(vault::GetIdleBalance, get_idle_balance, SU128);
passthrough_read!(vault::GetTotalSupply, get_total_supply, SU128);
passthrough_read!(vault::GetMaxDeposit, get_max_deposit, SU128);
passthrough_read!(
    vault::GetMaxSingleMarketDeposit,
    get_max_single_market_deposit,
    SU128
);
passthrough_read!(vault::GetFeeAnchorTimestamp, get_fee_anchor_timestamp, SU64);
passthrough_read!(vault::GetFees, get_fees, common_vault::Fees<SU128>);
passthrough_read!(
    vault::HasPendingMarketWithdrawal,
    has_pending_market_withdrawal,
    bool
);
passthrough_read!(vault::QueueTail, queue_tail, u64);
passthrough_read!(
    vault::BuildRealAssetsReport,
    build_real_assets_report,
    common_vault::RealAssetsReport
);

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::ConvertToShares, C> for Dispatch {
    async fn dispatch(request: vault::ConvertToShares, ctx: C) -> GatewayResult<SU128> {
        ctx.near_client()
            .vault(request.vault_id)
            .convert_to_shares(U128AssetsArg {
                assets: request.assets,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::ConvertToAssets, C> for Dispatch {
    async fn dispatch(request: vault::ConvertToAssets, ctx: C) -> GatewayResult<SU128> {
        ctx.near_client()
            .vault(request.vault_id)
            .convert_to_assets(U128SharesArg {
                shares: request.shares,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::PreviewDeposit, C> for Dispatch {
    async fn dispatch(request: vault::PreviewDeposit, ctx: C) -> GatewayResult<SU128> {
        ctx.near_client()
            .vault(request.vault_id)
            .preview_deposit(U128AssetsArg {
                assets: request.assets,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::PreviewMint, C> for Dispatch {
    async fn dispatch(request: vault::PreviewMint, ctx: C) -> GatewayResult<SU128> {
        ctx.near_client()
            .vault(request.vault_id)
            .preview_mint(U128SharesArg {
                shares: request.shares,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::PreviewWithdraw, C> for Dispatch {
    async fn dispatch(request: vault::PreviewWithdraw, ctx: C) -> GatewayResult<SU128> {
        ctx.near_client()
            .vault(request.vault_id)
            .preview_withdraw(U128AssetsArg {
                assets: request.assets,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::PreviewRedeem, C> for Dispatch {
    async fn dispatch(request: vault::PreviewRedeem, ctx: C) -> GatewayResult<SU128> {
        ctx.near_client()
            .vault(request.vault_id)
            .preview_redeem(U128SharesArg {
                shares: request.shares,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::GetCapGroups, C> for Dispatch {
    async fn dispatch(
        request: vault::GetCapGroups,
        ctx: C,
    ) -> GatewayResult<vault::GetCapGroupsResult> {
        ctx.near_client()
            .vault(request.vault_id)
            .get_cap_groups(())
            .await
            .map(|cap_groups| vault::GetCapGroupsResult { cap_groups })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::GetRestrictions, C> for Dispatch {
    async fn dispatch(
        request: vault::GetRestrictions,
        ctx: C,
    ) -> GatewayResult<vault::GetRestrictionsResult> {
        ctx.near_client()
            .vault(request.vault_id)
            .get_restrictions(())
            .await
            .map(|restrictions| vault::GetRestrictionsResult { restrictions })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::GetWithdrawingOpId, C> for Dispatch {
    async fn dispatch(
        request: vault::GetWithdrawingOpId,
        ctx: C,
    ) -> GatewayResult<vault::GetWithdrawingOpIdResult> {
        ctx.near_client()
            .vault(request.vault_id)
            .get_withdrawing_op_id(())
            .await
            .map(|op_id| vault::GetWithdrawingOpIdResult { op_id })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::GetCurrentWithdrawRequestId, C> for Dispatch {
    async fn dispatch(
        request: vault::GetCurrentWithdrawRequestId,
        ctx: C,
    ) -> GatewayResult<vault::GetCurrentWithdrawRequestIdResult> {
        ctx.near_client()
            .vault(request.vault_id)
            .get_current_withdraw_request_id(())
            .await
            .map(|request_id| vault::GetCurrentWithdrawRequestIdResult { request_id })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::PeekNextPendingWithdrawalId, C> for Dispatch {
    async fn dispatch(
        request: vault::PeekNextPendingWithdrawalId,
        ctx: C,
    ) -> GatewayResult<vault::PeekNextPendingWithdrawalIdResult> {
        ctx.near_client()
            .vault(request.vault_id)
            .peek_next_pending_withdrawal_id(())
            .await
            .map(|request_id| vault::PeekNextPendingWithdrawalIdResult { request_id })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::GetMarketIdOfAccount, C> for Dispatch {
    async fn dispatch(
        request: vault::GetMarketIdOfAccount,
        ctx: C,
    ) -> GatewayResult<vault::GetMarketIdOfAccountResult> {
        ctx.near_client()
            .vault(request.vault_id)
            .get_market_id_of_account(MarketAccountArg {
                market: request.market,
            })
            .await
            .map(|market_id| vault::GetMarketIdOfAccountResult { market_id })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::GetMarketAccountById, C> for Dispatch {
    async fn dispatch(
        request: vault::GetMarketAccountById,
        ctx: C,
    ) -> GatewayResult<vault::GetMarketAccountByIdResult> {
        ctx.near_client()
            .vault(request.vault_id)
            .get_market_account_by_id(MarketIdArg {
                market_id: request.market_id,
            })
            .await
            .map(|account_id| vault::GetMarketAccountByIdResult { account_id })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<vault::ListMarketsWithIds, C> for Dispatch {
    async fn dispatch(
        request: vault::ListMarketsWithIds,
        ctx: C,
    ) -> GatewayResult<vault::ListMarketsWithIdsResult> {
        ctx.near_client()
            .vault(request.vault_id)
            .list_markets_with_ids(())
            .await
            .map(|markets| vault::ListMarketsWithIdsResult { markets })
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<vault::Deposit, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<vault::Deposit>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let configuration = ctx
            .near_client()
            .vault(body.vault_id.clone())
            .cached_get_configuration()
            .await?;
        let mut steps = Vec::new();

        if let Some(tx_result) = ensure_storage_registration(
            &ctx,
            request.signer_account_id.clone(),
            body.vault_id.clone(),
            request.signer_account_id.0.clone(),
        )
        .await?
        {
            steps.push(tx_result);
        }

        steps.push(transfer_call_asset(
            &ctx,
            request.signer_account_id,
            configuration.underlying_token,
            body.vault_id,
            body.amount.0,
            &DepositMsg::Supply,
        )?);

        Ok(OperationPlan { steps })
    }
}

macro_rules! direct_write {
    ($spec:ty, $method:ident, $args:expr, $tgas:expr $(, $deposit:expr)?) => {
        #[async_trait]
        impl<C: HasNearClient> PlanWrite<$spec, C> for Dispatch {
            async fn plan(
                request: templar_gateway_types::common::WriteRequest<$spec>,
                ctx: C,
            ) -> GatewayResult<OperationPlan> {
                let body = request.body;
                let vault_id = body.vault_id.clone();
                let options = ContractWriteOptions::new(request.signer_account_id)
                    .tgas($tgas)
                    $(.deposit($deposit))?;
                ctx.near_client()
                    .vault(vault_id)
                    .$method(options, ($args)(body))
                    .map(OperationPlan::from)
            }
        }
    };
}

direct_write!(
    vault::Allocate,
    allocate,
    |body: vault::Allocate| DeltaArg { delta: body.delta },
    300
);
direct_write!(
    vault::Withdraw,
    withdraw,
    |body: vault::Withdraw| WithdrawArgs {
        amount: body.amount,
        receiver: body.receiver
    },
    30,
    vault::WITHDRAW_REQUEST_DEPOSIT
);
direct_write!(
    vault::Redeem,
    redeem,
    |body: vault::Redeem| RedeemArgs {
        shares: body.shares,
        receiver: body.receiver
    },
    300,
    vault::WITHDRAW_REQUEST_DEPOSIT
);
direct_write!(
    vault::ExecuteWithdrawal,
    execute_withdrawal,
    |body: vault::ExecuteWithdrawal| ExecuteWithdrawalArgs { route: body.route },
    300
);
direct_write!(
    vault::ExecuteMarketWithdrawal,
    execute_market_withdrawal,
    |body: vault::ExecuteMarketWithdrawal| ExecuteMarketWithdrawalArgs {
        op_id: body.op_id,
        market: body.market,
        batch_limit: body.batch_limit
    },
    300
);
direct_write!(
    vault::ExecuteRebalanceWithdrawal,
    execute_rebalance_withdrawal,
    |body: vault::ExecuteRebalanceWithdrawal| ExecuteRebalanceWithdrawalArgs {
        market_id: body.market_id,
        batch_limit: body.batch_limit
    },
    300
);
direct_write!(
    vault::ResyncIdleBalance,
    resync_idle_balance,
    |_body: vault::ResyncIdleBalance| (),
    30
);
direct_write!(
    vault::RefreshMarkets,
    refresh_markets,
    |body: vault::RefreshMarkets| MarketsArg {
        markets: body.markets
    },
    300
);
direct_write!(vault::Unbrick, unbrick, |_body: vault::Unbrick| (), 300);
direct_write!(
    vault::Skim,
    skim,
    |body: vault::Skim| TokenArg { token: body.token },
    50
);
direct_write!(
    vault::AccrueFee,
    internal_accrue_fee,
    |_body: vault::AccrueFee| (),
    20
);
direct_write!(
    vault::SetSupplyQueue,
    set_supply_queue,
    |body: vault::SetSupplyQueue| MarketsArg {
        markets: body.markets
    },
    50,
    vault::SET_SUPPLY_QUEUE_DEPOSIT
);
direct_write!(
    vault::SubmitCap,
    submit_cap,
    |body: vault::SubmitCap| CapArgs {
        market: body.market,
        new_cap: body.new_cap
    },
    5
);
direct_write!(
    vault::AcceptCap,
    accept_cap,
    |body: vault::AcceptCap| CapMarketArg {
        market: body.market
    },
    5
);
direct_write!(
    vault::RevokePendingCap,
    revoke_pending_cap,
    |body: vault::RevokePendingCap| CapMarketArg {
        market: body.market
    },
    5
);
direct_write!(
    vault::SubmitCapGroupUpdate,
    submit_cap_group_update,
    |body: vault::SubmitCapGroupUpdate| CapGroupUpdateArg {
        update: body.update
    },
    50
);
direct_write!(
    vault::AcceptCapGroupUpdate,
    accept_cap_group_update,
    |body: vault::AcceptCapGroupUpdate| CapGroupUpdateKeyArg {
        update: body.update
    },
    50
);
direct_write!(
    vault::RevokePendingCapGroupUpdate,
    revoke_pending_cap_group_update,
    |body: vault::RevokePendingCapGroupUpdate| CapGroupUpdateKeyArg {
        update: body.update
    },
    50
);
direct_write!(
    vault::SubmitMarketRemoval,
    submit_market_removal,
    |body: vault::SubmitMarketRemoval| CapMarketArg {
        market: body.market
    },
    50
);
direct_write!(
    vault::AcceptMarketRemoval,
    accept_market_removal,
    |body: vault::AcceptMarketRemoval| CapMarketArg {
        market: body.market
    },
    50
);
direct_write!(
    vault::RevokePendingMarketRemoval,
    revoke_pending_market_removal,
    |body: vault::RevokePendingMarketRemoval| CapMarketArg {
        market: body.market
    },
    50
);
direct_write!(
    vault::SetCurator,
    set_curator,
    |body: vault::SetCurator| AccountArg {
        account: body.account
    },
    50
);
direct_write!(
    vault::SetIsAllocator,
    set_is_allocator,
    |body: vault::SetIsAllocator| AllocatorArg {
        account: body.account,
        allowed: body.allowed
    },
    50
);
direct_write!(
    vault::SubmitSentinel,
    submit_sentinel,
    |body: vault::SubmitSentinel| AccountArg {
        account: body.account
    },
    50
);
direct_write!(
    vault::AcceptSentinel,
    accept_sentinel,
    |_body: vault::AcceptSentinel| (),
    50
);
direct_write!(
    vault::RevokePendingSentinel,
    revoke_pending_sentinel,
    |_body: vault::RevokePendingSentinel| (),
    50
);
direct_write!(
    vault::SetSkimRecipient,
    set_skim_recipient,
    |body: vault::SetSkimRecipient| AccountArg {
        account: body.account
    },
    50
);
direct_write!(
    vault::SetFees,
    set_fees,
    |body: vault::SetFees| FeesArg { fees: body.fees },
    50
);
direct_write!(
    vault::AcceptFees,
    accept_fees,
    |_body: vault::AcceptFees| (),
    50
);
direct_write!(
    vault::RevokePendingFees,
    revoke_pending_fees,
    |_body: vault::RevokePendingFees| (),
    50
);
direct_write!(
    vault::SetRestrictions,
    set_restrictions,
    |body: vault::SetRestrictions| RestrictionsArg {
        restrictions: body.restrictions
    },
    50
);
direct_write!(
    vault::AcceptRestrictions,
    accept_restrictions,
    |_body: vault::AcceptRestrictions| (),
    50
);
direct_write!(
    vault::RevokePendingRestrictions,
    revoke_pending_restrictions,
    |_body: vault::RevokePendingRestrictions| (),
    50
);
direct_write!(
    vault::SubmitTimelock,
    submit_timelock,
    |body: vault::SubmitTimelock| SubmitTimelockArgs {
        new_timelock_ns: body.new_timelock_ns,
        kind: body.kind
    },
    50
);
direct_write!(
    vault::AcceptTimelock,
    accept_timelock,
    |_body: vault::AcceptTimelock| (),
    50
);
direct_write!(
    vault::RevokePendingTimelock,
    revoke_pending_timelock,
    |_body: vault::RevokePendingTimelock| (),
    50
);
