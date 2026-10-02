//! NEAR interpreter for kernel effects.
//!
//! This is intentionally minimal: it focuses on share/accounting effects and
//! documents no-op handling for chain-specific calls that are orchestrated
//! elsewhere in the NEAR contract flows.

use std::fmt;

use near_sdk::{
    json_types::{U128, U64},
    near, AccountId, AccountIdRef,
};
use near_sdk_contract_tools::ft::{Nep141Burn, Nep141Controller, Nep141Mint, Nep141Transfer};

use templar_vault_kernel::effects::{KernelEffect, KernelEvent, WithdrawalSkipReason};
use templar_vault_kernel::types::Address;
use templar_vault_kernel::AddressBook;

use crate::governance::Gate;
use crate::Contract;

pub enum KernelEffectError {
    MissingAccount(Address),
    MintFailed,
    BurnFailed,
    UnsupportedEffect(&'static str),
    EpochRequiresIdle,
    EpochCutoffRequiresIdle,
    EpochCutoffUnauthorized,
    EpochCutoffRejected,
    EpochSettlementRejected,
    EpochIntakeNotOpen,
    EpochDrainRequired,
    EpochSettledNavImmutable,
    WithdrawalEpochUnsettled,
    WithdrawalBelowMinAssetsOut,
    CancelCallerNotOwner,
    CancelRequestNotFound,
    CancelInFlightRequest,
    CancelQueueRepairFailed,
    PayoutBurnMustMatchFullEscrow,
    PayoutClaimMismatch,
}

impl fmt::Display for KernelEffectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAccount(address) => {
                write!(f, "missing account for address {:02x?}", address.as_bytes())
            }
            Self::MintFailed => f.write_str("failed to mint shares"),
            Self::BurnFailed => f.write_str("failed to burn shares"),
            Self::UnsupportedEffect(kind) => write!(f, "unsupported kernel effect: {kind}"),
            Self::EpochRequiresIdle => f.write_str("epoch settlement requires Idle"),
            Self::EpochCutoffRequiresIdle => f.write_str("epoch cutoff requires Idle"),
            Self::EpochCutoffUnauthorized => f.write_str("epoch cutoff not authorized"),
            Self::EpochCutoffRejected => f.write_str("epoch cutoff rejected by settlement law"),
            Self::EpochSettlementRejected => {
                f.write_str("epoch settlement rejected by settlement law")
            }
            Self::EpochIntakeNotOpen => f.write_str("epoch intake is not open"),
            Self::EpochDrainRequired => {
                f.write_str("older withdrawal intake must drain before epoch advancement")
            }
            Self::EpochSettledNavImmutable => f.write_str("settled epoch snapshot is immutable"),
            Self::WithdrawalEpochUnsettled => {
                f.write_str("withdrawal claim awaits epoch settlement")
            }
            Self::WithdrawalBelowMinAssetsOut => {
                f.write_str("settled claim below withdrawal min_assets_out")
            }
            Self::CancelCallerNotOwner => {
                f.write_str("withdrawal cancellation caller is not the owner")
            }
            Self::CancelRequestNotFound => {
                f.write_str("cancellation target withdrawal is not pending")
            }
            Self::CancelInFlightRequest => {
                f.write_str("withdrawal is in flight and cannot be cancelled")
            }
            Self::CancelQueueRepairFailed => {
                f.write_str("withdrawal queue repair failed after cancellation")
            }
            Self::PayoutBurnMustMatchFullEscrow => {
                f.write_str("payout success must burn all escrow shares")
            }
            Self::PayoutClaimMismatch => {
                f.write_str("payout amount does not match the settled claim")
            }
        }
    }
}

impl fmt::Debug for KernelEffectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[near(event_json(standard = "templar-vault-kernel"))]
pub enum KernelEventLog {
    #[event_version("1.0.0")]
    AllocationStarted {
        op_id: U64,
        total: U128,
        plan_len: u32,
    },
    #[event_version("1.0.0")]
    AllocationStepFailed {
        op_id: U64,
        index: u32,
        remaining: U128,
        total_allocated: U128,
    },
    #[event_version("1.0.0")]
    AllocationCompleted { op_id: U64, has_withdrawal: bool },
    #[event_version("1.0.0")]
    WithdrawalStarted {
        op_id: U64,
        amount: U128,
        escrow_shares: U128,
        owner: AccountId,
        receiver: AccountId,
    },
    #[event_version("1.0.0")]
    WithdrawalCollected {
        op_id: U64,
        burn_shares: U128,
        collected: U128,
    },
    #[event_version("1.0.0")]
    WithdrawalStopped { op_id: U64, escrow_shares: U128 },
    #[event_version("1.1.0")]
    WithdrawalSkipped {
        id: U64,
        owner: AccountId,
        receiver: AccountId,
        escrow_shares: U128,
        reason: String,
    },
    #[event_version("1.0.0")]
    RefreshStarted { op_id: U64, plan_len: u32 },
    #[event_version("1.0.0")]
    RefreshCompleted { op_id: U64 },
    #[event_version("1.0.0")]
    PayoutCompleted {
        op_id: U64,
        success: bool,
        burn_shares: U128,
        refund_shares: U128,
        amount: U128,
    },
    #[event_version("1.0.0")]
    DepositProcessed {
        owner: AccountId,
        receiver: AccountId,
        assets_in: U128,
        shares_out: U128,
    },
    #[event_version("1.0.0")]
    AtomicWithdrawProcessed {
        owner: AccountId,
        receiver: AccountId,
        shares_burned: U128,
        assets_out: U128,
    },
    #[event_version("1.1.0")]
    WithdrawalRequested {
        id: U64,
        owner: AccountId,
        receiver: AccountId,
        shares: U128,
        /// Epoch the request was queued in. Never a payout figure: the request
        /// is unpriced until an accepted epoch settlement covers it.
        epoch_id: U64,
    },
    #[event_version("1.0.0")]
    ExternalAssetsSynced {
        op_id: U64,
        new_external_assets: U128,
        total_assets: U128,
    },
    #[event_version("1.0.0")]
    FeesRefreshed { now_ns: U64, total_assets: U128 },
    #[event_version("1.0.0")]
    PauseUpdated { paused: bool },
    #[event_version("1.0.0")]
    EmergencyResetCompleted { op_id: U64, from_state: u32 },
    #[event_version("1.0.0")]
    EpochCutoffStarted { epoch_id: U64, cutoff_ns: U64 },
    #[event_version("1.0.0")]
    EpochSettled {
        epoch_id: U64,
        report_seq: U64,
        settlement_nav: U128,
        eligible_supply: U128,
        cutoff_ns: U64,
        as_of_ns: U64,
    },
    #[event_version("1.0.0")]
    WithdrawalCancelled {
        id: U64,
        owner: AccountId,
        escrow_shares: U128,
        epoch_id: U64,
    },
    #[event_version("1.0.0")]
    PendingDepositRecorded {
        owner: AccountId,
        assets: U128,
        requested_at_ns: U64,
        epoch_id: U64,
    },
    #[event_version("1.0.0")]
    PendingDepositRefunded {
        owner: AccountId,
        assets: U128,
        request_id: U64,
    },
    #[event_version("1.0.0")]
    DepositAdmitted {
        receiver: AccountId,
        assets_in: U128,
        shares_out: U128,
        request_epoch_id: U64,
        settlement_epoch_id: U64,
    },
    #[event_version("1.0.0")]
    EpochSupplySeeded {
        receiver: AccountId,
        assets_in: U128,
        shares_out: U128,
    },
}

/// Address resolution context for kernel effects.
#[derive(Clone, Default)]
pub struct KernelEffectContext {
    accounts: AddressBook<AccountId>,
}

impl KernelEffectContext {
    pub fn insert(&mut self, address: Address, account: AccountId) {
        self.accounts.insert(address, account);
    }

    fn resolve(&self, address: &Address) -> Result<&AccountId, KernelEffectError> {
        self.accounts
            .resolve(address)
            .ok_or(KernelEffectError::MissingAccount(*address))
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "event emission is a direct variant-to-log dispatch table"
)]
fn emit_kernel_event(
    event: &KernelEvent,
    ctx: &KernelEffectContext,
) -> Result<(), KernelEffectError> {
    match event {
        KernelEvent::AllocationStarted {
            op_id,
            total,
            plan_len,
        } => KernelEventLog::AllocationStarted {
            op_id: U64(*op_id),
            total: U128(*total),
            plan_len: *plan_len,
        }
        .emit(),
        KernelEvent::AllocationStepFailed {
            op_id,
            index,
            remaining,
            total_allocated,
        } => KernelEventLog::AllocationStepFailed {
            op_id: U64(*op_id),
            index: *index,
            remaining: U128(*remaining),
            total_allocated: U128(*total_allocated),
        }
        .emit(),
        KernelEvent::AllocationCompleted {
            op_id,
            has_withdrawal,
        } => KernelEventLog::AllocationCompleted {
            op_id: U64(*op_id),
            has_withdrawal: *has_withdrawal,
        }
        .emit(),
        KernelEvent::WithdrawalStarted {
            op_id,
            amount,
            escrow_shares,
            owner,
            receiver,
        } => {
            let owner = ctx.resolve(owner)?.clone();
            let receiver = ctx.resolve(receiver)?.clone();
            KernelEventLog::WithdrawalStarted {
                op_id: U64(*op_id),
                amount: U128(*amount),
                escrow_shares: U128(*escrow_shares),
                owner,
                receiver,
            }
            .emit();
        }
        KernelEvent::WithdrawalCollected {
            op_id,
            burn_shares,
            collected,
        } => KernelEventLog::WithdrawalCollected {
            op_id: U64(*op_id),
            burn_shares: U128(*burn_shares),
            collected: U128(*collected),
        }
        .emit(),
        KernelEvent::WithdrawalStopped {
            op_id,
            escrow_shares,
        } => KernelEventLog::WithdrawalStopped {
            op_id: U64(*op_id),
            escrow_shares: U128(*escrow_shares),
        }
        .emit(),
        KernelEvent::WithdrawalSkipped {
            id,
            owner,
            receiver,
            escrow_shares,
            reason,
        } => {
            let owner = ctx.resolve(owner)?.clone();
            let receiver = ctx.resolve(receiver)?.clone();
            KernelEventLog::WithdrawalSkipped {
                id: U64(*id),
                owner,
                receiver,
                escrow_shares: U128(*escrow_shares),
                reason: match reason {
                    WithdrawalSkipReason::Restricted => "restricted",
                }
                .to_string(),
            }
            .emit();
        }
        KernelEvent::RefreshStarted { op_id, plan_len } => KernelEventLog::RefreshStarted {
            op_id: U64(*op_id),
            plan_len: *plan_len,
        }
        .emit(),
        KernelEvent::RefreshCompleted { op_id } => {
            KernelEventLog::RefreshCompleted { op_id: U64(*op_id) }.emit();
        }
        KernelEvent::PayoutCompleted {
            op_id,
            success,
            burn_shares,
            refund_shares,
            amount,
        } => KernelEventLog::PayoutCompleted {
            op_id: U64(*op_id),
            success: *success,
            burn_shares: U128(*burn_shares),
            refund_shares: U128(*refund_shares),
            amount: U128(*amount),
        }
        .emit(),
        KernelEvent::DepositProcessed {
            owner,
            receiver,
            assets_in,
            shares_out,
        } => {
            let owner = ctx.resolve(owner)?.clone();
            let receiver = ctx.resolve(receiver)?.clone();
            KernelEventLog::DepositProcessed {
                owner,
                receiver,
                assets_in: U128(*assets_in),
                shares_out: U128(*shares_out),
            }
            .emit();
        }
        KernelEvent::AtomicWithdrawProcessed {
            owner,
            receiver,
            shares_burned,
            assets_out,
        } => {
            let owner = ctx.resolve(owner)?.clone();
            let receiver = ctx.resolve(receiver)?.clone();
            KernelEventLog::AtomicWithdrawProcessed {
                owner,
                receiver,
                shares_burned: U128(*shares_burned),
                assets_out: U128(*assets_out),
            }
            .emit();
        }
        KernelEvent::EpochCutoffStarted { epoch_id, cutoff_ns } => {
            KernelEventLog::EpochCutoffStarted {
                epoch_id: U64(*epoch_id),
                cutoff_ns: U64(*cutoff_ns),
            }
            .emit();
        }
        KernelEvent::EpochSettled {
            epoch_id,
            report_seq,
            settlement_nav,
            eligible_supply,
            cutoff_ns,
            as_of_ns,
            report_hash: _,
        } => {
            KernelEventLog::EpochSettled {
                epoch_id: U64(*epoch_id),
                report_seq: U64(*report_seq),
                settlement_nav: U128(*settlement_nav),
                eligible_supply: U128(*eligible_supply),
                cutoff_ns: U64(*cutoff_ns),
                as_of_ns: U64(*as_of_ns),
            }
            .emit();
        }
        KernelEvent::WithdrawalCancelled {
            id,
            owner,
            escrow_shares,
            epoch_id,
        } => {
            let owner = ctx.resolve(owner)?.clone();
            KernelEventLog::WithdrawalCancelled {
                id: U64(*id),
                owner,
                escrow_shares: U128(*escrow_shares),
                epoch_id: U64(*epoch_id),
            }
            .emit();
        }
        KernelEvent::DepositAdmitted {
            receiver,
            assets_in,
            shares_out,
            request_epoch_id,
            settlement_epoch_id,
        } => {
            let receiver = ctx.resolve(receiver)?.clone();
            KernelEventLog::DepositAdmitted {
                receiver,
                assets_in: U128(*assets_in),
                shares_out: U128(*shares_out),
                request_epoch_id: U64(*request_epoch_id),
                settlement_epoch_id: U64(*settlement_epoch_id),
            }
            .emit();
        }
        KernelEvent::WithdrawalRequested {
            id,
            owner,
            receiver,
            shares,
            epoch_id,
        } => {
            let owner = ctx.resolve(owner)?.clone();
            let receiver = ctx.resolve(receiver)?.clone();
            KernelEventLog::WithdrawalRequested {
                id: U64(*id),
                owner,
                receiver,
                shares: U128(*shares),
                epoch_id: U64(*epoch_id),
            }
            .emit();
        }

        KernelEvent::ExternalAssetsSynced {
            op_id,
            new_external_assets,
            total_assets,
        } => KernelEventLog::ExternalAssetsSynced {
            op_id: U64(*op_id),
            new_external_assets: U128(*new_external_assets),
            total_assets: U128(*total_assets),
        }
        .emit(),
        KernelEvent::FeesRefreshed {
            now_ns,
            total_assets,
        } => KernelEventLog::FeesRefreshed {
            now_ns: U64(*now_ns),
            total_assets: U128(*total_assets),
        }
        .emit(),
        KernelEvent::PauseUpdated { paused } => {
            KernelEventLog::PauseUpdated { paused: *paused }.emit();
        }
        KernelEvent::EmergencyResetCompleted { op_id, from_state } => {
            KernelEventLog::EmergencyResetCompleted {
                op_id: U64(*op_id),
                from_state: *from_state,
            }
            .emit();
        }
        KernelEvent::EpochSupplySeeded {
            receiver,
            assets_in,
            shares_out,
        } => {
            let receiver = ctx.resolve(receiver)?.clone();
            KernelEventLog::EpochSupplySeeded {
                receiver,
                assets_in: U128(*assets_in),
                shares_out: U128(*shares_out),
            }
            .emit();
        }
    }

    Ok(())
}

/// Map a kernel error to a typed NEAR effect error.
///
/// Epoch lifecycle, cancellation, and payout-integrity rejections surface as
/// dedicated typed errors instead of collapsing into a generic failure.
#[must_use]
pub(crate) fn kernel_effect_error_for_kernel_error(
    error: &templar_vault_kernel::error::KernelError,
) -> Option<KernelEffectError> {
    use templar_vault_kernel::error::{InvalidStateCode as Code, KernelError};

    let code = match error {
        KernelError::InvalidState(code) => *code,
        _ => return None,
    };

    let mapped = match code {
        Code::EpochRequiresIdle => KernelEffectError::EpochRequiresIdle,
        Code::EpochCutoffRequiresIdle => KernelEffectError::EpochCutoffRequiresIdle,
        Code::EpochCutoffUnauthorized => KernelEffectError::EpochCutoffUnauthorized,
        Code::EpochCutoffRejected => KernelEffectError::EpochCutoffRejected,
        Code::EpochSettlementRejected => KernelEffectError::EpochSettlementRejected,
        Code::EpochIntakeNotOpen => KernelEffectError::EpochIntakeNotOpen,
        Code::EpochDrainRequired => KernelEffectError::EpochDrainRequired,
        Code::EpochSettledNavImmutable => KernelEffectError::EpochSettledNavImmutable,
        Code::WithdrawalEpochUnsettled => KernelEffectError::WithdrawalEpochUnsettled,
        Code::WithdrawalBelowMinAssetsOut => KernelEffectError::WithdrawalBelowMinAssetsOut,
        Code::CancelCallerNotOwner => KernelEffectError::CancelCallerNotOwner,
        Code::CancelRequestNotFound => KernelEffectError::CancelRequestNotFound,
        Code::CancelInFlightRequest => KernelEffectError::CancelInFlightRequest,
        Code::CancelQueueRepairFailed => KernelEffectError::CancelQueueRepairFailed,
        Code::PayoutBurnMustMatchFullEscrow => KernelEffectError::PayoutBurnMustMatchFullEscrow,
        Code::PayoutClaimMismatch => KernelEffectError::PayoutClaimMismatch,
        _ => return None,
    };

    Some(mapped)
}

/// Apply kernel effects to NEAR storage.
pub(crate) fn apply_kernel_effects(
    contract: &mut Contract,
    effects: &[KernelEffect],
    ctx: &KernelEffectContext,
) -> Result<(), KernelEffectError> {
    for effect in effects {
        #[allow(unreachable_patterns)]
        match effect {
            KernelEffect::MintShares { owner, shares } => {
                let receiver = ctx.resolve(owner)?;
                contract
                    .mint(&Nep141Mint::new(*shares, receiver))
                    .map_err(|_| KernelEffectError::MintFailed)?;
            }
            KernelEffect::BurnShares { owner, shares } => {
                let account = ctx.resolve(owner)?;
                contract
                    .burn(&Nep141Burn::new(*shares, account))
                    .map_err(|_| KernelEffectError::BurnFailed)?;
            }
            KernelEffect::BurnSharesFrom { owner, shares, .. } => {
                let account = ctx.resolve(owner)?;
                contract
                    .burn(&Nep141Burn::new(*shares, account))
                    .map_err(|_| KernelEffectError::BurnFailed)?;
            }
            KernelEffect::TransferShares { from, to, shares } => {
                let sender = ctx.resolve(from)?;
                let receiver = ctx.resolve(to)?;
                let sender_ref: &AccountIdRef = sender.as_ref();
                let receiver_ref: &AccountIdRef = receiver.as_ref();
                let transfer = Nep141Transfer::new(*shares, sender_ref, receiver_ref);
                Gate::bypass_transfer(contract, &transfer);
            }
            KernelEffect::EmitEvent { event } => {
                emit_kernel_event(event, ctx)?;
            }
            KernelEffect::TransferAssetsFrom { .. } => {
                // Assets are transferred via ft_on_transfer before kernel execution.
            }
            KernelEffect::TransferAssets { to, amount } => {
                // NEAR transfers are orchestrated explicitly in contract flows (see `pay` and
                // withdrawal callbacks). Kernel-driven execution should not schedule raw asset
                // transfers without a callback, so we treat this as a documented no-op.
                let _ = ctx.resolve(to)?;
                let _ = amount;
            }
            KernelEffect::ExternalCall {
                target,
                selector: _,
                args: _,
                attached_value: _,
                callback: _,
            } => {
                // External calls are handled by explicit Promise flows in the NEAR contract.
                // This synchronous interpreter does not schedule async cross-contract calls.
                let _ = ctx.resolve(target)?;
            }
            KernelEffect::ChargeStorage { payer, bytes: _ } => {
                // Storage charging is enforced at entrypoints (NEP-145). Kernel effects do not
                // have access to attached deposits, so this is a documented no-op here.
                let _ = ctx.resolve(payer)?;
            }
            _ => {
                return Err(KernelEffectError::UnsupportedEffect(
                    "unhandled kernel effect",
                ));
            }
        }
    }

    Ok(())
}

/// Log that assets were taken into custody for a deposit that cannot be priced.
///
/// The notification is separate from the ledger write: the liability is
/// recorded whether or not the log lands, so a depositor's claim cannot be lost
/// or weakened by an emission failure.
pub(crate) fn emit_pending_deposit_recorded(
    owner: &AccountId,
    assets: u128,
    requested_at_ns: u64,
    epoch_id: u64,
) {
    KernelEventLog::PendingDepositRecorded {
        owner: owner.clone(),
        assets: U128(assets),
        requested_at_ns: U64(requested_at_ns),
        epoch_id: U64(epoch_id),
    }
    .emit();
}

/// Log that a pending-deposit liability was released after its transfer to the
/// depositor succeeded.
///
/// This is called only after the ledger has released the holding, so the
/// notification never gates the release it describes.
pub(crate) fn emit_pending_deposit_refunded(owner: &AccountId, assets: u128, request_id: u64) {
    KernelEventLog::PendingDepositRefunded {
        owner: owner.clone(),
        assets: U128(assets),
        request_id: U64(request_id),
    }
    .emit();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::account_id_to_address;
    use crate::test_utils::{mk, new_test_contract};
    use near_sdk_contract_tools::ft::Nep141 as _;

    fn context_for(accounts: &[AccountId]) -> KernelEffectContext {
        let mut ctx = KernelEffectContext::default();
        for account in accounts {
            let address = account_id_to_address(account);
            ctx.insert(address, account.clone());
        }
        ctx
    }

    #[test]
    fn test_apply_kernel_effects_mint_burn_transfer() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);

        let alice = mk(1);
        let bob = mk(2);

        let ctx = context_for(&[alice.clone(), bob.clone()]);

        let effects = vec![
            KernelEffect::MintShares {
                owner: account_id_to_address(&alice),
                shares: 1_000,
            },
            KernelEffect::TransferShares {
                from: account_id_to_address(&alice),
                to: account_id_to_address(&bob),
                shares: 400,
            },
            KernelEffect::BurnShares {
                owner: account_id_to_address(&bob),
                shares: 100,
            },
        ];

        apply_kernel_effects(&mut c, &effects, &ctx).expect("apply effects");

        assert_eq!(c.ft_total_supply().0, 900);
        assert_eq!(c.ft_balance_of(alice.clone()).0, 600);
        assert_eq!(c.ft_balance_of(bob.clone()).0, 300);
    }

    #[test]
    fn test_apply_kernel_effects_transfer_assets_noop() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);

        let alice = mk(1);
        let ctx = context_for(std::slice::from_ref(&alice));

        let effects = vec![KernelEffect::TransferAssets {
            to: account_id_to_address(&alice),
            amount: 10,
        }];

        apply_kernel_effects(&mut c, &effects, &ctx).expect("apply effects");
    }

    #[test]
    fn test_apply_kernel_effects_external_call_noop() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);

        let alice = mk(1);
        let ctx = context_for(std::slice::from_ref(&alice));

        let effects = vec![KernelEffect::ExternalCall {
            target: account_id_to_address(&alice),
            selector: 0,
            args: Vec::new(),
            attached_value: 0,
            callback: None,
        }];

        apply_kernel_effects(&mut c, &effects, &ctx).expect("apply effects");
    }

    #[test]
    fn test_apply_kernel_effects_charge_storage_requires_account() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);

        let ctx = KernelEffectContext::default();
        let effects = vec![KernelEffect::ChargeStorage {
            payer: account_id_to_address(&mk(1)),
            bytes: 10,
        }];

        let err = apply_kernel_effects(&mut c, &effects, &ctx).expect_err("missing account");
        assert!(matches!(err, KernelEffectError::MissingAccount(_)));
    }

    #[test]
    fn test_apply_kernel_effects_emits_kernel_event() {
        let vault_id = mk(0);
        let mut c = new_test_contract(&vault_id);

        let alice = mk(1);
        let bob = mk(2);

        let ctx = context_for(&[alice.clone(), bob.clone()]);

        let effects = vec![KernelEffect::EmitEvent {
            event: KernelEvent::DepositProcessed {
                owner: account_id_to_address(&alice),
                receiver: account_id_to_address(&bob),
                assets_in: 10,
                shares_out: 9,
            },
        }];

        apply_kernel_effects(&mut c, &effects, &ctx).expect("apply effects");
    }
}
