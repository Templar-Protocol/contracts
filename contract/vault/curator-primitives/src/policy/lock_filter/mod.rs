use alloc::vec::Vec;

use templar_vault_kernel::TargetId;

use super::market_lock::MarketLockSet;

impl MarketLockSet {
    #[inline]
    #[must_use]
    pub fn is_unlocked(&self, target_id: TargetId, current_ns: u64) -> bool {
        !self.is_locked(target_id, current_ns)
    }

    /// Filter a target list to only unlocked targets.
    #[must_use]
    pub fn filter_targets(&self, targets: &[TargetId], current_ns: u64) -> Vec<TargetId> {
        targets
            .iter()
            .copied()
            .filter(|target_id| self.is_unlocked(*target_id, current_ns))
            .collect()
    }
}
