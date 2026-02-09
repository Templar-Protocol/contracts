//! NEAR interpreter for kernel effects.
//!
//! This is intentionally minimal: it focuses on share/accounting effects and
//! documents no-op handling for chain-specific calls that are orchestrated
//! elsewhere in the NEAR contract flows.

use std::collections::BTreeMap;

use near_sdk::{
    json_types::{U128, U64},
    near, AccountId, AccountIdRef,
};
use near_sdk_contract_tools::ft::{Nep141Burn, Nep141Controller, Nep141Mint, Nep141Transfer};

use templar_vault_kernel::effects::{KernelEffect, KernelEvent};
use templar_vault_kernel::AddressBook;
use templar_vault_kernel::types::Address;

use crate::governance::Gate;
use crate::Contract;

#[derive(Debug)]
pub enum KernelEffectError {
    MissingAccount(Address),
    MintFailed,
    BurnFailed,
    TransferFailed,
    UnsupportedEffect(&'static str),
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
    AllocationCompleted {
        op_id: U64,
        has_withdrawal: bool,
    },
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
    WithdrawalStopped {
        op_id: U64,
        escrow_shares: U128,
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
    WithdrawalRequested {
        id: U64,
        owner: AccountId,
        receiver: AccountId,
        shares: U128,
        expected_assets: U128,
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
}

/// Address resolution context for kernel effects.
#[derive(Clone, Debug, Default)]
pub struct KernelEffectContext {
    accounts: AddressBook<AccountId>,
}

impl KernelEffectContext {
    #[must_use]
    pub fn new(accounts: BTreeMap<Address, AccountId>) -> Self {
        Self {
            accounts: AddressBook::from(accounts),
        }
    }

    pub fn insert(&mut self, address: Address, account: AccountId) {
        self.accounts.insert(address, account);
    }

    fn resolve(&self, address: &Address) -> Result<&AccountId, KernelEffectError> {
        self.accounts
            .resolve(address)
            .ok_or(KernelEffectError::MissingAccount(*address))
    }
}

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
            .emit()
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
        KernelEvent::RefreshStarted { op_id, plan_len } => {
            KernelEventLog::RefreshStarted {
                op_id: U64(*op_id),
                plan_len: *plan_len,
            }
            .emit()
        }
        KernelEvent::RefreshCompleted { op_id } => {
            KernelEventLog::RefreshCompleted { op_id: U64(*op_id) }.emit()
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
            .emit()
        }
        KernelEvent::WithdrawalRequested {
            id,
            owner,
            receiver,
            shares,
            expected_assets,
        } => {
            let owner = ctx.resolve(owner)?.clone();
            let receiver = ctx.resolve(receiver)?.clone();
            KernelEventLog::WithdrawalRequested {
                id: U64(*id),
                owner,
                receiver,
                shares: U128(*shares),
                expected_assets: U128(*expected_assets),
            }
            .emit()
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
            KernelEventLog::PauseUpdated { paused: *paused }.emit()
        }
        KernelEvent::EmergencyResetCompleted { op_id, from_state } => {
            KernelEventLog::EmergencyResetCompleted {
                op_id: U64(*op_id),
                from_state: *from_state,
            }
            .emit()
        }
    }

    Ok(())
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
        let ctx = context_for(&[alice.clone()]);

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
        let ctx = context_for(&[alice.clone()]);

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
