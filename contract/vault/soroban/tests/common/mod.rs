use std::vec::Vec;

use soroban_sdk::Address;
use templar_soroban_runtime::{
    auth::AuthResult, ActionKind, AddressRegistrar, AuthAdapter, EffectContext, EffectInterpreter,
    EffectResult,
};
use templar_vault_kernel::effects::KernelEffect;

#[derive(Clone, Debug, Default)]
pub struct MockInterpreter {
    pub should_fail: bool,
    pub effects: Vec<KernelEffect>,
}

impl MockInterpreter {
    pub fn new() -> Self {
        Self {
            should_fail: false,
            effects: Vec::new(),
        }
    }
}

impl EffectInterpreter for MockInterpreter {
    fn execute_effect(&mut self, effect: &KernelEffect, _ctx: &EffectContext) -> EffectResult<()> {
        if self.should_fail {
            return Err(templar_soroban_runtime::RuntimeError::effect_failed(
                "mock interpreter failed",
            ));
        }
        self.effects.push(effect.clone());
        Ok(())
    }
}

impl AddressRegistrar for MockInterpreter {
    fn register_address(&mut self, _kernel_addr: [u8; 32], _soroban_addr: Address) {}

    fn has_address(&self, _kernel_addr: &[u8; 32]) -> bool {
        true
    }
}

#[derive(Clone, Copy, Default)]
pub struct TestPermissiveAuth;

impl AuthAdapter for TestPermissiveAuth {
    fn authorize(
        &self,
        _action: ActionKind,
        _caller: [u8; 32],
        _proof: Option<&[u8]>,
    ) -> AuthResult<()> {
        Ok(())
    }

    fn is_paused(&self) -> bool {
        false
    }
}
