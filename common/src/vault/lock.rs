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
        self.to_lock.clear();
    }

    pub fn is_locked(&self, market: MarketId) -> bool {
        self.to_lock.contains(&market)
    }

    pub fn is_locked_all(&self) -> bool {
        !self.to_lock.is_empty()
    }
}
