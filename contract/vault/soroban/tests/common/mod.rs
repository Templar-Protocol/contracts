use std::vec::Vec;

use soroban_sdk::Address;
use templar_soroban_runtime::{AddressRegistrar, EffectContext, EffectInterpreter, EffectResult};
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
