use alloc::vec::Vec;

use templar_vault_kernel::{TargetId, TimestampNs};

use super::market_lock::MarketLeaseRegistry;

impl MarketLeaseRegistry {
    #[inline]
    #[must_use]
    pub fn is_unleased(&self, target_id: TargetId, now_ns: TimestampNs) -> bool {
        !self.is_leased(target_id, now_ns)
    }

    #[must_use]
    pub fn excluding_leased_targets(
        &self,
        targets: &[TargetId],
        now_ns: TimestampNs,
    ) -> Vec<TargetId> {
        targets
            .iter()
            .copied()
            .filter(|target_id| self.is_unleased(*target_id, now_ns))
            .collect()
    }
}
