//! OpenZeppelin-based share token support for the Soroban vault.

use crate::effects::{EffectResult, Sep41Token};
use soroban_sdk::{Address, Env};
use stellar_tokens::fungible::burnable::emit_burn;
use stellar_tokens::fungible::{emit_mint, emit_transfer, Base};

/// Share token adapter that applies share effects directly in-contract.
pub struct ShareTokenAdapter<'a> {
    env: &'a Env,
}

impl<'a> ShareTokenAdapter<'a> {
    /// Create a new share token adapter for the current contract.
    #[inline]
    #[must_use]
    pub fn new(env: &'a Env) -> Self {
        Self { env }
    }
}

impl Sep41Token for ShareTokenAdapter<'_> {
    fn mint(&self, to: &Address, amount: i128) -> EffectResult<()> {
        Base::update(self.env, None, Some(to), amount);
        emit_mint(self.env, to, amount);
        Ok(())
    }

    fn burn(&self, from: &Address, amount: i128) -> EffectResult<()> {
        Base::update(self.env, Some(from), None, amount);
        emit_burn(self.env, from, amount);
        Ok(())
    }

    fn transfer(&self, from: &Address, to: &Address, amount: i128) -> EffectResult<()> {
        Base::update(self.env, Some(from), Some(to), amount);
        emit_transfer(self.env, from, to, amount);
        Ok(())
    }

    fn balance(&self, addr: &Address) -> EffectResult<i128> {
        Ok(Base::balance(self.env, addr))
    }
}
