//! Withdraw route planning for collecting assets from markets.

use alloc::vec::Vec;
use templar_vault_kernel::TargetId;
use typed_builder::TypedBuilder;

use super::target_set::find_first_duplicate;

/// An entry in a withdraw route.
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq, TypedBuilder)]
#[builder(field_defaults(setter(into)))]
pub struct WithdrawRouteEntry {
    pub target_id: TargetId,
    pub max_amount: u128,
    #[builder(default)]
    pub available_liquidity: Option<u128>,
}

impl WithdrawRouteEntry {
    #[must_use]
    pub fn new(target_id: TargetId, max_amount: u128) -> Self {
        Self {
            target_id,
            max_amount,
            available_liquidity: None,
        }
    }

    #[must_use]
    pub fn with_liquidity(mut self, available_liquidity: u128) -> Self {
        self.available_liquidity = Some(available_liquidity);
        self
    }
}

impl From<(TargetId, u128)> for WithdrawRouteEntry {
    fn from(value: (TargetId, u128)) -> Self {
        Self::new(value.0, value.1)
    }
}

/// A planned route for withdrawing assets.
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct WithdrawRoute {
    pub entries: Vec<WithdrawRouteEntry>,
    pub target_amount: u128,
}

impl WithdrawRoute {
    #[must_use]
    pub fn new(target_amount: u128) -> Self {
        Self {
            entries: Vec::new(),
            target_amount,
        }
    }

    #[must_use]
    pub fn from_entries(entries: Vec<WithdrawRouteEntry>, target_amount: u128) -> Self {
        Self {
            entries,
            target_amount,
        }
    }

    #[must_use]
    pub fn with_entry(mut self, entry: WithdrawRouteEntry) -> Self {
        self.entries.push(entry);
        self
    }

    #[must_use]
    pub fn with_entries(mut self, entries: impl IntoIterator<Item = WithdrawRouteEntry>) -> Self {
        self.entries.extend(entries);
        self
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
    pub fn total(&self) -> u128 {
        self.entries
            .iter()
            .fold(0u128, |acc, e| acc.saturating_add(e.max_amount))
    }

    #[must_use]
    pub fn available_liquidity(&self) -> u128 {
        self.entries
            .iter()
            .filter_map(|e| e.available_liquidity)
            .fold(0u128, |acc, l| acc.saturating_add(l))
    }

    #[must_use]
    pub fn can_satisfy(&self) -> bool {
        self.total() >= self.target_amount
    }

    /// Validate the withdraw route.
    ///
    /// Checks:
    /// - Target amount is non-zero
    /// - Route is not empty
    /// - Route total is at least target amount
    /// - No duplicate targets
    /// - No zero max_amount entries
    pub fn validate(&self) -> Result<(), WithdrawRouteError> {
        if self.target_amount == 0 {
            return Err(WithdrawRouteError::ZeroTargetAmount);
        }

        if self.is_empty() {
            return Err(WithdrawRouteError::EmptyRoute);
        }

        // Check for zero amounts.
        for entry in &self.entries {
            if entry.max_amount == 0 {
                return Err(WithdrawRouteError::ZeroMaxAmount {
                    target_id: entry.target_id,
                });
            }
        }

        // Check duplicates via shared target-set helper.
        let targets: Vec<TargetId> = self.entries.iter().map(|e| e.target_id).collect();
        if let Some(target_id) = find_first_duplicate(&targets) {
            return Err(WithdrawRouteError::DuplicateTarget { target_id });
        }

        // Check route total covers target
        if !self.can_satisfy() {
            return Err(WithdrawRouteError::InsufficientRouteTotal {
                route_total: self.total(),
                target_amount: self.target_amount,
            });
        }

        Ok(())
    }

    /// Convert to a list of (target_id, amount) pairs.
    ///
    /// This is useful for passing to the withdrawal state machine.
    #[must_use]
    pub fn to_withdrawal_plan(&self) -> Vec<(TargetId, u128)> {
        self.entries
            .iter()
            .map(|e| (e.target_id, e.max_amount))
            .collect()
    }

    /// Get entry for a specific target.
    #[must_use]
    pub fn get_entry(&self, target_id: TargetId) -> Option<&WithdrawRouteEntry> {
        self.entries.iter().find(|e| e.target_id == target_id)
    }

    /// Check if a target is in the route.
    #[must_use]
    pub fn has_target(&self, target_id: TargetId) -> bool {
        self.entries.iter().any(|e| e.target_id == target_id)
    }
}

impl From<(Vec<WithdrawRouteEntry>, u128)> for WithdrawRoute {
    fn from(value: (Vec<WithdrawRouteEntry>, u128)) -> Self {
        Self::from_entries(value.0, value.1)
    }
}

/// Errors that can occur during withdraw route operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WithdrawRouteError {
    /// Target amount must be greater than zero.
    ZeroTargetAmount,
    /// Route contains no entries.
    EmptyRoute,
    /// Route total is less than the target amount.
    InsufficientRouteTotal {
        route_total: u128,
        target_amount: u128,
    },
    /// Duplicate target in route.
    DuplicateTarget { target_id: TargetId },
    /// Entry has zero max amount.
    ZeroMaxAmount { target_id: TargetId },
}

/// Build a withdraw route from market principals.
///
/// Creates a route that attempts to withdraw proportionally from each market
/// based on its principal, up to the target amount.
///
/// # Arguments
/// * `principals` - List of (target_id, principal_amount) pairs
/// * `target_amount` - Total amount to withdraw
///
/// # Returns
/// A withdraw route, or an error if the route cannot satisfy the target.
pub fn build_withdraw_route(
    principals: &[(TargetId, u128)],
    target_amount: u128,
) -> Result<WithdrawRoute, WithdrawRouteError> {
    if target_amount == 0 {
        return Err(WithdrawRouteError::ZeroTargetAmount);
    }

    let total_principal: u128 = principals
        .iter()
        .fold(0u128, |acc, (_, p)| acc.saturating_add(*p));

    if total_principal < target_amount {
        return Err(WithdrawRouteError::InsufficientRouteTotal {
            route_total: total_principal,
            target_amount,
        });
    }

    // Create entries sorted by principal (largest first)
    let mut sorted: Vec<(TargetId, u128)> =
        principals.iter().filter(|(_, p)| *p > 0).cloned().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let entries: Vec<WithdrawRouteEntry> = sorted
        .into_iter()
        .map(|(target_id, principal)| WithdrawRouteEntry::new(target_id, principal))
        .collect();

    if entries.is_empty() {
        return Err(WithdrawRouteError::EmptyRoute);
    }

    Ok(WithdrawRoute::from_entries(entries, target_amount))
}

/// Build a withdraw route with liquidity constraints.
///
/// Similar to `build_withdraw_route`, but also considers available liquidity
/// at each market.
///
/// # Arguments
/// * `market_data` - List of (target_id, principal, available_liquidity) tuples
/// * `target_amount` - Total amount to withdraw
///
/// # Returns
/// A withdraw route optimized for liquidity, or an error.
pub fn build_withdraw_route_with_liquidity(
    market_data: &[(TargetId, u128, u128)],
    target_amount: u128,
) -> Result<WithdrawRoute, WithdrawRouteError> {
    if target_amount == 0 {
        return Err(WithdrawRouteError::ZeroTargetAmount);
    }

    // Sort by available liquidity (highest first)
    let mut sorted: Vec<(TargetId, u128, u128)> = market_data
        .iter()
        .filter(|(_, p, _)| *p > 0)
        .cloned()
        .collect();
    sorted.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));

    // Use the minimum of principal and available liquidity for each entry
    let entries: Vec<WithdrawRouteEntry> = sorted
        .into_iter()
        .map(|(target_id, principal, liquidity)| {
            let max_amount = principal.min(liquidity);
            WithdrawRouteEntry::new(target_id, max_amount).with_liquidity(liquidity)
        })
        .filter(|e| e.max_amount > 0)
        .collect();

    if entries.is_empty() {
        return Err(WithdrawRouteError::EmptyRoute);
    }

    let route = WithdrawRoute::from_entries(entries, target_amount);

    if !route.can_satisfy() {
        return Err(WithdrawRouteError::InsufficientRouteTotal {
            route_total: route.total(),
            target_amount,
        });
    }

    Ok(route)
}

#[cfg(test)]
mod tests;
