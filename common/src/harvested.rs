use near_sdk::{env, near, require};

use crate::asset::BorrowAssetAmount;

#[derive(Clone, Debug, Default)]
#[near(serializers = [borsh])]
pub struct Harvested {
    oldest_snapshot_index: u32,
    harvests: Vec<BorrowAssetAmount>,
}

impl Harvested {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_deposit_amount_harvest(
        &mut self,
        from_snapshot_index: u32,
        until_snapshot_index: u32,
        deposit_amount: BorrowAssetAmount,
    ) {
        require!(
            from_snapshot_index < until_snapshot_index,
            "Invalid snapshot range",
        );

        let Some(from_index) = from_snapshot_index.checked_sub(self.oldest_snapshot_index) else {
            env::panic_str(&format!("Invariant violation: Attempt to record harvest in expired/deleted snapshot: Requested {from_snapshot_index}, but oldest available is {}.", self.oldest_snapshot_index));
        };
        let from_index =
            usize::try_from(from_index).unwrap_or_else(|e| env::panic_str(&e.to_string()));

        let until_index = (until_snapshot_index - self.oldest_snapshot_index) as usize;
        if until_index > self.harvests.len() {
            self.harvests.resize(until_index, BorrowAssetAmount::zero());
        }
        for harvest in &mut self.harvests[from_index..until_index] {
            harvest.join(deposit_amount);
        }
    }

    pub fn clear_until(&mut self, until_snapshot_index: u32) {
        let Some(count) = until_snapshot_index.checked_sub(self.oldest_snapshot_index) else {
            return;
        };
        let count = count as usize;
        self.harvests.rotate_left(count);
        self.harvests.truncate(self.harvests.len() - count);
    }
}
