use near_sdk::{env, AccountId};
use std::{collections::BTreeSet, vec::Vec};
use templar_common::vault::{
    AllocatingState as CommonAllocatingState, MarketId, OpState as CommonOpState,
    PayoutState as CommonPayoutState, RefreshingState as CommonRefreshingState,
    Restrictions as CommonRestrictions, WithdrawingState as CommonWithdrawingState,
};
use templar_vault_kernel::{
    AllocatingState as KernelAllocatingState, Address, OpState as KernelOpState,
    PayoutState as KernelPayoutState, RefreshingState as KernelRefreshingState,
    Restrictions as KernelRestrictions, TargetId,
    WithdrawingState as KernelWithdrawingState,
};

/// Convert executor-facing identifiers into kernel TargetId.
pub trait IntoTargetId {
    fn into_target_id(self) -> TargetId;
}

impl IntoTargetId for MarketId {
    fn into_target_id(self) -> TargetId {
        u32::from(self)
    }
}

impl IntoTargetId for &MarketId {
    fn into_target_id(self) -> TargetId {
        u32::from(*self)
    }
}

impl IntoTargetId for TargetId {
    fn into_target_id(self) -> TargetId {
        self
    }
}

/// Convert kernel TargetId into executor MarketId.
pub trait IntoMarketId {
    fn into_market_id(self) -> MarketId;
}

impl IntoMarketId for TargetId {
    fn into_market_id(self) -> MarketId {
        MarketId::from(self)
    }
}

impl IntoMarketId for &TargetId {
    fn into_market_id(self) -> MarketId {
        MarketId::from(*self)
    }
}

const ADDRESS_DOMAIN: &[u8] = b"templar:near:account-id";

/// Deterministic one-way mapping for AccountId -> Address.
///
/// This keeps NEAR storage/API types unchanged (AccountId/U128/U64) while allowing
/// kernel logic (Address-based) to be applied. The mapping is *not reversible*,
/// so kernel effects that need AccountId must use executor context, not Address.
pub(crate) fn account_id_to_address(account: &AccountId) -> Address {
    let mut bytes = Vec::with_capacity(ADDRESS_DOMAIN.len() + account.as_bytes().len());
    bytes.extend_from_slice(ADDRESS_DOMAIN);
    bytes.extend_from_slice(account.as_bytes());
    let hash = env::sha256(&bytes);
    hash.as_slice()
        .try_into()
        .unwrap_or_else(|_| panic!("expected 32-byte sha256 hash"))
}

fn map_account_set(set: &BTreeSet<AccountId>) -> BTreeSet<Address> {
    set.iter().map(account_id_to_address).collect()
}

/// Convert NEAR restrictions (AccountId-based) into kernel restrictions (Address-based).
///
/// The mapping is one-way, so kernel restrictions are only suitable for
/// validation checks; executor code must still use AccountIds for effects.
pub(crate) fn to_kernel_restrictions(restrictions: &CommonRestrictions) -> KernelRestrictions {
    match restrictions {
        CommonRestrictions::Paused => KernelRestrictions::Paused,
        CommonRestrictions::BlackList(set) => KernelRestrictions::BlackList(map_account_set(set)),
        CommonRestrictions::WhiteList(set) => KernelRestrictions::WhiteList(map_account_set(set)),
    }
}

pub(crate) fn to_kernel_restrictions_opt(
    restrictions: Option<&CommonRestrictions>,
) -> Option<KernelRestrictions> {
    restrictions.map(to_kernel_restrictions)
}

/// Convert common OpState into kernel OpState for recovery/action dispatch.
pub fn to_kernel_op_state(state: &CommonOpState) -> KernelOpState {
    match state {
        CommonOpState::Idle => KernelOpState::Idle,
        CommonOpState::Allocating(CommonAllocatingState {
            op_id,
            index,
            remaining,
            plan,
        }) => KernelOpState::Allocating(KernelAllocatingState {
            op_id: *op_id,
            index: *index,
            remaining: *remaining,
            plan: plan
                .iter()
                .map(|(market, amount)| (market.into_target_id(), *amount))
                .collect(),
        }),
        CommonOpState::Withdrawing(CommonWithdrawingState {
            op_id,
            index,
            remaining,
            collected,
            receiver,
            owner,
            escrow_shares,
        }) => KernelOpState::Withdrawing(KernelWithdrawingState {
            op_id: *op_id,
            index: *index,
            remaining: *remaining,
            collected: *collected,
            receiver: account_id_to_address(receiver),
            owner: account_id_to_address(owner),
            escrow_shares: *escrow_shares,
        }),
        CommonOpState::Refreshing(CommonRefreshingState { op_id, index, plan }) => {
            KernelOpState::Refreshing(KernelRefreshingState {
                op_id: *op_id,
                index: *index,
                plan: plan.iter().map(IntoTargetId::into_target_id).collect(),
            })
        }
        CommonOpState::Payout(CommonPayoutState {
            op_id,
            receiver,
            amount,
            owner,
            escrow_shares,
            burn_shares,
        }) => KernelOpState::Payout(KernelPayoutState {
            op_id: *op_id,
            receiver: account_id_to_address(receiver),
            amount: *amount,
            owner: account_id_to_address(owner),
            escrow_shares: *escrow_shares,
            burn_shares: *burn_shares,
        }),
    }
}
