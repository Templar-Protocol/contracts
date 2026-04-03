//! Fenced market leases for serialized policy and executor state transitions.
//!
//! These types do not provide synchronization by themselves. Safety comes from
//! storing the registry in serialized executor state and enforcing issued
//! fencing tokens on downstream mutations.

use alloc::vec::Vec;
use templar_vault_kernel::{TargetId, TimestampNs};

use super::state::OrderedMap;

#[templar_vault_macros::vault_derive(borsh, postcard, schemars, serde, std_borsh_schema)]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LeaseOwner(pub u64);

#[templar_vault_macros::vault_derive(borsh, postcard, schemars, serde, std_borsh_schema)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FencingToken(pub u64);

#[templar_vault_macros::vault_derive(borsh, postcard, schemars, serde, std_borsh_schema)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LeaseDurationNs(pub u64);

#[templar_vault_macros::vault_derive(borsh, postcard, schemars, serde, std_borsh_schema)]
#[derive(Clone, PartialEq, Eq)]
pub struct MarketLease {
    target_id: TargetId,
    owner: LeaseOwner,
    op_id: Option<u64>,
    acquired_at: TimestampNs,
    expires_at: TimestampNs,
    fencing_token: FencingToken,
}

impl MarketLease {
    #[must_use]
    pub fn target_id(&self) -> TargetId {
        self.target_id
    }

    #[must_use]
    pub fn owner(&self) -> &LeaseOwner {
        &self.owner
    }

    #[must_use]
    pub fn op_id(&self) -> Option<u64> {
        self.op_id
    }

    #[must_use]
    pub fn acquired_at(&self) -> TimestampNs {
        self.acquired_at
    }

    #[must_use]
    pub fn expires_at(&self) -> TimestampNs {
        self.expires_at
    }

    #[must_use]
    pub fn fencing_token(&self) -> FencingToken {
        self.fencing_token
    }

    #[must_use]
    pub fn is_expired(&self, now: TimestampNs) -> bool {
        now >= self.expires_at
    }

    #[must_use]
    pub fn remaining(&self, now: TimestampNs) -> LeaseDurationNs {
        LeaseDurationNs(u64::from(self.expires_at).saturating_sub(u64::from(now)))
    }
}

#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum AcquireLeaseError {
    ZeroTtl,
    ExpiryOverflow,
    AlreadyLeased { existing: MarketLease },
}

#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum ReleaseLeaseError {
    NotFound {
        target_id: TargetId,
    },
    OwnerMismatch {
        target_id: TargetId,
        expected_owner: LeaseOwner,
        actual_owner: LeaseOwner,
    },
}

#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum FencingError {
    NotCurrent {
        target_id: TargetId,
        presented: FencingToken,
        current: FencingToken,
    },
    NotFound {
        target_id: TargetId,
    },
}

#[templar_vault_macros::vault_derive(borsh, postcard, schemars, serde, std_borsh_schema)]
#[derive(Clone, Default, PartialEq, Eq)]
pub struct MarketLeaseRegistry {
    leases_by_target: OrderedMap<TargetId, MarketLease>,
    next_fencing_token: u64,
}

impl MarketLeaseRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.leases_by_target.is_empty()
    }

    #[must_use]
    pub fn stored_len(&self) -> usize {
        self.leases_by_target.len()
    }

    #[must_use]
    pub fn active_len(&self, now: TimestampNs) -> usize {
        self.leases_by_target
            .values()
            .filter(|lease| !lease.is_expired(now))
            .count()
    }

    #[must_use]
    pub fn get(&self, target_id: TargetId) -> Option<&MarketLease> {
        self.leases_by_target.get(&target_id)
    }

    #[must_use]
    pub fn get_active(&self, target_id: TargetId, now: TimestampNs) -> Option<&MarketLease> {
        self.get(target_id).filter(|lease| !lease.is_expired(now))
    }

    #[must_use]
    pub fn is_leased(&self, target_id: TargetId, now: TimestampNs) -> bool {
        self.get_active(target_id, now).is_some()
    }

    #[must_use]
    pub fn is_leased_by_owner(
        &self,
        target_id: TargetId,
        owner: &LeaseOwner,
        now: TimestampNs,
    ) -> bool {
        self.get_active(target_id, now)
            .is_some_and(|lease| lease.owner() == owner)
    }

    #[must_use]
    pub fn leased_targets(&self, now: TimestampNs) -> Vec<TargetId> {
        self.leases_by_target
            .iter()
            .filter_map(|(target_id, lease)| (!lease.is_expired(now)).then_some(*target_id))
            .collect()
    }

    #[must_use]
    pub fn find_leased_targets(&self, targets: &[TargetId], now: TimestampNs) -> Vec<TargetId> {
        targets
            .iter()
            .copied()
            .filter(|target_id| self.is_leased(*target_id, now))
            .collect()
    }

    #[must_use]
    pub fn cleanup_expired(&self, now: TimestampNs) -> Self {
        let mut next = self.clone();
        next.leases_by_target
            .retain(|_, lease| !lease.is_expired(now));
        next
    }

    #[must_use]
    pub fn clear(&self) -> Self {
        let mut next = self.clone();
        next.leases_by_target.clear();
        next
    }

    pub fn try_acquire(
        &self,
        target_id: TargetId,
        owner: LeaseOwner,
        op_id: Option<u64>,
        now: TimestampNs,
        ttl: LeaseDurationNs,
    ) -> Result<(Self, MarketLease), AcquireLeaseError> {
        if ttl.0 == 0 {
            return Err(AcquireLeaseError::ZeroTtl);
        }

        let expires_at = u64::from(now)
            .checked_add(ttl.0)
            .map(TimestampNs)
            .ok_or(AcquireLeaseError::ExpiryOverflow)?;

        let cleaned = self.cleanup_expired(now);

        if let Some(existing) = cleaned.get_active(target_id, now) {
            if existing.owner() != &owner {
                return Err(AcquireLeaseError::AlreadyLeased {
                    existing: existing.clone(),
                });
            }
        }

        let next_fencing_token = cleaned
            .next_fencing_token
            .checked_add(1)
            .expect("fencing token overflow should be unreachable");

        let lease = MarketLease {
            target_id,
            owner,
            op_id,
            acquired_at: now,
            expires_at,
            fencing_token: FencingToken(next_fencing_token),
        };

        let mut next = cleaned;
        next.next_fencing_token = next_fencing_token;
        next.leases_by_target.insert(target_id, lease.clone());
        Ok((next, lease))
    }

    pub fn release_if_owned(
        &self,
        target_id: TargetId,
        owner: &LeaseOwner,
    ) -> Result<Self, ReleaseLeaseError> {
        let Some(existing) = self.leases_by_target.get(&target_id) else {
            return Err(ReleaseLeaseError::NotFound { target_id });
        };

        if existing.owner() != owner {
            return Err(ReleaseLeaseError::OwnerMismatch {
                target_id,
                expected_owner: existing.owner().clone(),
                actual_owner: owner.clone(),
            });
        }

        let mut next = self.clone();
        next.leases_by_target.remove(&target_id);
        Ok(next)
    }

    #[must_use]
    pub fn force_release(&self, target_id: TargetId) -> Self {
        let mut next = self.clone();
        next.leases_by_target.remove(&target_id);
        next
    }

    #[must_use]
    pub fn force_release_by_op(&self, op_id: u64) -> Self {
        let mut next = self.clone();
        next.leases_by_target
            .retain(|_, lease| lease.op_id() != Some(op_id));
        next
    }

    pub fn assert_token_current(
        &self,
        target_id: TargetId,
        token: FencingToken,
        now: TimestampNs,
    ) -> Result<(), FencingError> {
        let Some(current) = self.get_active(target_id, now) else {
            return Err(FencingError::NotFound { target_id });
        };

        if current.fencing_token() != token {
            return Err(FencingError::NotCurrent {
                target_id,
                presented: token,
                current: current.fencing_token(),
            });
        }

        Ok(())
    }
}
