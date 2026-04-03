use alloc::boxed::Box;
use alloc::vec::Vec;
use templar_vault_kernel::{TargetId, TimestampNs};

use super::{duplicate::find_first_duplicate, market_lock::MarketLeaseRegistry};

#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, PartialEq, Eq)]
pub struct WithdrawRouteEntry {
    target_id: TargetId,
    max_amount: u128,
    available_liquidity: Option<u128>,
}

impl WithdrawRouteEntry {
    pub fn new(target_id: TargetId, max_amount: u128) -> Result<Self, WithdrawRouteError> {
        if max_amount == 0 {
            return Err(WithdrawRouteError::ZeroMaxAmount { target_id });
        }

        Ok(Self {
            target_id,
            max_amount,
            available_liquidity: None,
        })
    }

    pub fn with_liquidity(mut self, available_liquidity: u128) -> Result<Self, WithdrawRouteError> {
        if self.max_amount > available_liquidity {
            return Err(WithdrawRouteError::LiquidityLessThanMaxAmount {
                target_id: self.target_id,
                max_amount: self.max_amount,
                available_liquidity,
            });
        }

        self.available_liquidity = Some(available_liquidity);
        Ok(self)
    }

    #[must_use]
    pub fn target_id(&self) -> TargetId {
        self.target_id
    }

    #[must_use]
    pub fn max_amount(&self) -> u128 {
        self.max_amount
    }

    #[must_use]
    pub fn available_liquidity(&self) -> Option<u128> {
        self.available_liquidity
    }
}

impl From<(TargetId, u128)> for WithdrawRouteEntry {
    fn from(value: (TargetId, u128)) -> Self {
        Self::new(value.0, value.1).expect("tuple conversion requires non-zero max amount")
    }
}

#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone)]
pub struct WithdrawRoute {
    entries: Vec<WithdrawRouteEntry>,
    target_amount: u128,
}

impl WithdrawRoute {
    pub fn new(
        entries: Vec<WithdrawRouteEntry>,
        target_amount: u128,
    ) -> Result<Self, WithdrawRouteError> {
        let route = Self {
            entries,
            target_amount,
        };
        route.validate()?;
        Ok(route)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn entries(&self) -> &[WithdrawRouteEntry] {
        &self.entries
    }

    #[must_use]
    pub fn target_amount(&self) -> u128 {
        self.target_amount
    }

    pub fn checked_total(&self) -> Result<u128, WithdrawRouteError> {
        checked_total_amount(self.entries.iter().map(WithdrawRouteEntry::max_amount))
    }

    pub fn known_available_liquidity(&self) -> Result<Option<u128>, WithdrawRouteError> {
        self.entries
            .iter()
            .map(WithdrawRouteEntry::available_liquidity)
            .try_fold(Some(0u128), |acc, maybe_liquidity| {
                match (acc, maybe_liquidity) {
                    (Some(sum), Some(liquidity)) => sum
                        .checked_add(liquidity)
                        .map(Some)
                        .ok_or(WithdrawRouteError::AmountOverflow),
                    _ => Ok(None),
                }
            })
    }

    #[must_use]
    pub fn can_satisfy(&self) -> bool {
        reaches_target(
            self.entries.iter().map(WithdrawRouteEntry::max_amount),
            self.target_amount,
        )
    }

    pub fn validate(&self) -> Result<(), WithdrawRouteError> {
        if self.target_amount == 0 {
            return Err(WithdrawRouteError::ZeroTargetAmount);
        }

        if self.is_empty() {
            return Err(WithdrawRouteError::EmptyRoute);
        }

        for entry in &self.entries {
            if entry.max_amount() == 0 {
                return Err(WithdrawRouteError::ZeroMaxAmount {
                    target_id: entry.target_id(),
                });
            }

            if let Some(available_liquidity) = entry.available_liquidity() {
                if entry.max_amount() > available_liquidity {
                    return Err(WithdrawRouteError::LiquidityLessThanMaxAmount {
                        target_id: entry.target_id(),
                        max_amount: entry.max_amount(),
                        available_liquidity,
                    });
                }
            }
        }

        let targets: Vec<TargetId> = self
            .entries
            .iter()
            .map(WithdrawRouteEntry::target_id)
            .collect();
        if let Some(target_id) = find_first_duplicate(&targets) {
            return Err(WithdrawRouteError::DuplicateTarget { target_id });
        }

        if !self.can_satisfy() {
            return Err(WithdrawRouteError::InsufficientRouteTotal {
                route_total: capped_total(
                    self.entries.iter().map(WithdrawRouteEntry::max_amount),
                    self.target_amount,
                ),
                target_amount: self.target_amount,
            });
        }

        Ok(())
    }

    #[must_use]
    pub fn to_target_amount_pairs(&self) -> Vec<(TargetId, u128)> {
        self.entries
            .iter()
            .map(|entry| (entry.target_id(), entry.max_amount()))
            .collect()
    }

    #[must_use]
    pub fn get_entry(&self, target_id: TargetId) -> Option<&WithdrawRouteEntry> {
        self.entries
            .iter()
            .find(|entry| entry.target_id() == target_id)
    }

    #[must_use]
    pub fn has_target(&self, target_id: TargetId) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.target_id() == target_id)
    }

    #[must_use]
    pub fn excluding_leased(
        &self,
        leases: &MarketLeaseRegistry,
        now_ns: TimestampNs,
    ) -> Result<Self, WithdrawRouteError> {
        let filtered_entries = self
            .entries
            .iter()
            .filter(|entry| leases.is_unleased(entry.target_id(), now_ns))
            .cloned()
            .collect();

        Self::new(filtered_entries, self.target_amount).map_err(|source| {
            WithdrawRouteError::LockedTargetsExcluded {
                source: Box::new(source),
            }
        })
    }

    #[must_use]
    pub fn to_target_amount_pairs_excluding_leased(
        &self,
        leases: &MarketLeaseRegistry,
        now_ns: TimestampNs,
    ) -> Result<Vec<(TargetId, u128)>, WithdrawRouteError> {
        Ok(self
            .excluding_leased(leases, now_ns)?
            .to_target_amount_pairs())
    }
}

#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum WithdrawRouteError {
    ZeroTargetAmount,
    EmptyRoute,
    InsufficientRouteTotal {
        route_total: u128,
        target_amount: u128,
    },
    DuplicateTarget {
        target_id: TargetId,
    },
    ZeroMaxAmount {
        target_id: TargetId,
    },
    LiquidityLessThanMaxAmount {
        target_id: TargetId,
        max_amount: u128,
        available_liquidity: u128,
    },
    AmountOverflow,
    LockedTargetsExcluded {
        source: Box<WithdrawRouteError>,
    },
}

fn checked_total_amount<I>(amounts: I) -> Result<u128, WithdrawRouteError>
where
    I: IntoIterator<Item = u128>,
{
    amounts.into_iter().try_fold(0u128, |acc, amount| {
        acc.checked_add(amount)
            .ok_or(WithdrawRouteError::AmountOverflow)
    })
}

fn reaches_target<I>(amounts: I, target_amount: u128) -> bool
where
    I: IntoIterator<Item = u128>,
{
    capped_total(amounts, target_amount) >= target_amount
}

fn capped_total<I>(amounts: I, target_amount: u128) -> u128
where
    I: IntoIterator<Item = u128>,
{
    amounts.into_iter().fold(0u128, |acc, amount| {
        acc.saturating_add(amount).min(target_amount)
    })
}

pub fn build_withdraw_route(
    principals: &[(TargetId, u128)],
    target_amount: u128,
) -> Result<WithdrawRoute, WithdrawRouteError> {
    if target_amount == 0 {
        return Err(WithdrawRouteError::ZeroTargetAmount);
    }

    let total_principal = capped_total(
        principals.iter().map(|(_, principal)| *principal),
        target_amount,
    );

    if total_principal < target_amount {
        return Err(WithdrawRouteError::InsufficientRouteTotal {
            route_total: total_principal,
            target_amount,
        });
    }

    let mut sorted: Vec<(TargetId, u128)> = principals
        .iter()
        .filter(|(_, principal)| *principal > 0)
        .cloned()
        .collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let entries: Vec<WithdrawRouteEntry> = sorted
        .into_iter()
        .map(|(target_id, principal)| WithdrawRouteEntry::new(target_id, principal))
        .collect::<Result<_, _>>()?;

    if entries.is_empty() {
        return Err(WithdrawRouteError::EmptyRoute);
    }

    WithdrawRoute::new(entries, target_amount)
}

pub fn build_withdraw_route_with_liquidity(
    market_data: &[(TargetId, u128, u128)],
    target_amount: u128,
) -> Result<WithdrawRoute, WithdrawRouteError> {
    if target_amount == 0 {
        return Err(WithdrawRouteError::ZeroTargetAmount);
    }

    let mut sorted: Vec<(TargetId, u128, u128)> = market_data
        .iter()
        .filter(|(_, principal, _)| *principal > 0)
        .cloned()
        .collect();
    sorted.sort_by(|a, b| {
        let a_effective = a.1.min(a.2);
        let b_effective = b.1.min(b.2);

        b_effective.cmp(&a_effective).then_with(|| a.0.cmp(&b.0))
    });

    let entries: Vec<WithdrawRouteEntry> = sorted
        .into_iter()
        .filter_map(|(target_id, principal, liquidity)| {
            let max_amount = principal.min(liquidity);
            (max_amount > 0).then_some((target_id, max_amount, liquidity))
        })
        .map(|(target_id, max_amount, liquidity)| {
            WithdrawRouteEntry::new(target_id, max_amount)?.with_liquidity(liquidity)
        })
        .collect::<Result<_, _>>()?;

    if entries.is_empty() {
        return Err(WithdrawRouteError::EmptyRoute);
    }

    let route_total = capped_total(
        entries.iter().map(WithdrawRouteEntry::max_amount),
        target_amount,
    );
    if route_total < target_amount {
        return Err(WithdrawRouteError::InsufficientRouteTotal {
            route_total,
            target_amount,
        });
    }

    WithdrawRoute::new(entries, target_amount)
}
