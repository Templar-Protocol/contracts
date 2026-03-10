use super::*;

/// Tracks which markets are currently locked for exclusive operation (e.g. during rebalance withdrawals).
#[derive(Default)]
#[near(serializers = [borsh, serde])]
pub struct Locker {
    to_lock: Vec<MarketId>,
}

impl Locker {
    pub fn lock(&mut self, market: MarketId) {
        if self.is_locked(market) {
            env::panic_str("Market is locked");
        }
        Event::LockChange {
            is_locked: true,
            market,
        }
        .emit();
        self.to_lock.push(market);
    }

    pub fn unlock(&mut self, market: MarketId) {
        if !self.is_locked(market) {
            return;
        }
        Event::LockChange {
            is_locked: false,
            market,
        }
        .emit();
        self.to_lock.retain(|&x| x != market);
    }

    /// Clears the lock status for all markets.
    /// This method should be used with caution as it will unlock all markets
    pub fn clear(&mut self) {
        for market in self.to_lock.iter().copied() {
            Event::LockChange {
                is_locked: false,
                market,
            }
            .emit();
        }
        self.to_lock.clear();
    }

    pub fn is_locked(&self, market: MarketId) -> bool {
        self.to_lock.contains(&market)
    }

    pub fn is_locked_all(&self) -> bool {
        !self.to_lock.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::Locker;
    use crate::vault::MarketId;
    use near_sdk::{test_utils::VMContextBuilder, testing_env};

    #[test]
    fn lock_unlock_and_clear_track_state() {
        testing_env!(VMContextBuilder::new().build());

        let first = MarketId(1);
        let second = MarketId(2);
        let mut locker = Locker::default();

        locker.lock(first);
        locker.lock(second);
        assert!(locker.is_locked(first));
        assert!(locker.is_locked(second));
        assert!(locker.is_locked_all());

        locker.unlock(first);
        assert!(!locker.is_locked(first));
        assert!(locker.is_locked(second));

        locker.clear();
        assert!(!locker.is_locked_all());
        assert!(!locker.is_locked(second));
    }

    #[test]
    fn unlock_only_emits_when_state_changes() {
        testing_env!(VMContextBuilder::new().build());

        let market = MarketId(7);
        let mut locker = Locker::default();

        locker.unlock(market);
        assert!(near_sdk::test_utils::get_logs().is_empty());

        locker.lock(market);
        locker.unlock(market);

        let logs = near_sdk::test_utils::get_logs().join("\n");
        assert!(logs.contains("\"event\":\"lock_change\""));
        assert!(logs.contains("\"is_locked\":false"));
    }
}
