//! NEAR interpreter for kernel effects.
//!
//! This is intentionally minimal: it focuses on share/accounting effects and
//! leaves chain-specific external calls as stubs until kernel-driven execution
//! is fully integrated.

use std::collections::BTreeMap;

use near_sdk::{AccountId, AccountIdRef};
use near_sdk_contract_tools::ft::{Nep141Burn, Nep141Controller, Nep141Mint, Nep141Transfer};

use templar_vault_kernel::effects::KernelEffect;
use templar_vault_kernel::types::Address;

use crate::governance::Gate;
use crate::Contract;

#[derive(Debug)]
pub enum KernelEffectError {
    MissingAccount(Address),
    MintFailed,
    BurnFailed,
    TransferFailed,
}

/// Address resolution context for kernel effects.
#[derive(Clone, Debug, Default)]
pub struct KernelEffectContext {
    accounts: BTreeMap<Address, AccountId>,
}

impl KernelEffectContext {
    #[must_use]
    pub fn new(accounts: BTreeMap<Address, AccountId>) -> Self {
        Self { accounts }
    }

    pub fn insert(&mut self, address: Address, account: AccountId) {
        self.accounts.insert(address, account);
    }

    fn resolve(&self, address: &Address) -> Result<&AccountId, KernelEffectError> {
        self.accounts
            .get(address)
            .ok_or(KernelEffectError::MissingAccount(*address))
    }
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
            KernelEffect::EmitEvent { event: _ } => {
                // Kernel events are emitted by kernel transitions. NEAR already
                // emits its own events; explicit mapping will come later.
            }
            _ => {
                // Stub for kernel-driven external calls / storage charging.
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
}
