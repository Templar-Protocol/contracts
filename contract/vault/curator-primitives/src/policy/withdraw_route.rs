//! Withdraw route planning for collecting assets from markets.
//!
//! Withdraw routes define the order and amounts to collect from markets
//! when satisfying withdrawal requests. This allows curators to optimize
//! liquidity collection and minimize market impact.

use alloc::vec::Vec;
use templar_vault_kernel::TargetId;

/// An entry in a withdraw route.
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawRouteEntry {
    /// Target market/strategy ID to withdraw from.
    pub target_id: TargetId,
    /// Maximum amount to withdraw from this target.
    pub max_amount: u128,
    /// Available liquidity at this target (if known).
    pub available_liquidity: Option<u128>,
}

impl WithdrawRouteEntry {
    /// Create a new withdraw route entry.
    pub fn new(target_id: TargetId, max_amount: u128) -> Self {
        Self {
            target_id,
            max_amount,
            available_liquidity: None,
        }
    }

    /// Create a new entry with known liquidity.
    pub fn with_liquidity(
        target_id: TargetId,
        max_amount: u128,
        available_liquidity: u128,
    ) -> Self {
        Self {
            target_id,
            max_amount,
            available_liquidity: Some(available_liquidity),
        }
    }
}

impl From<(TargetId, u128)> for WithdrawRouteEntry {
    fn from(value: (TargetId, u128)) -> Self {
        Self::new(value.0, value.1)
    }
}

/// A planned route for withdrawing assets.
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct WithdrawRoute {
    /// Ordered list of targets to withdraw from.
    pub entries: Vec<WithdrawRouteEntry>,
    /// Total amount needed for the withdrawal.
    pub target_amount: u128,
}

impl WithdrawRoute {
    /// Create a new empty withdraw route.
    pub fn new(target_amount: u128) -> Self {
        Self {
            entries: Vec::new(),
            target_amount,
        }
    }

    /// Create a withdraw route from entries.
    pub fn from_entries(entries: Vec<WithdrawRouteEntry>, target_amount: u128) -> Self {
        Self {
            entries,
            target_amount,
        }
    }

    /// Returns true if the route is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the number of entries in the route.
    pub fn len(&self) -> usize {
        self.entries.len()
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

/// Compute the total maximum amount available in a withdraw route.
///
/// # Arguments
/// * `route` - The withdraw route
///
/// # Returns
/// Sum of all max_amount values, using saturating addition.
pub fn compute_route_total(route: &WithdrawRoute) -> u128 {
    route
        .entries
        .iter()
        .fold(0u128, |acc, e| acc.saturating_add(e.max_amount))
}

/// Compute the total available liquidity in a withdraw route.
///
/// Only includes entries where liquidity is known.
///
/// # Arguments
/// * `route` - The withdraw route
///
/// # Returns
/// Sum of all known available_liquidity values.
pub fn compute_available_liquidity(route: &WithdrawRoute) -> u128 {
    route
        .entries
        .iter()
        .filter_map(|e| e.available_liquidity)
        .fold(0u128, |acc, l| acc.saturating_add(l))
}

/// Validate a withdraw route.
///
/// Checks:
/// - Target amount is non-zero
/// - Route is not empty
/// - Route total is at least target amount
/// - No duplicate targets
/// - No zero max_amount entries
///
/// # Arguments
/// * `route` - The withdraw route to validate
///
/// # Returns
/// `Ok(())` if valid, or the first error found.
pub fn validate_withdraw_route(route: &WithdrawRoute) -> Result<(), WithdrawRouteError> {
    if route.target_amount == 0 {
        return Err(WithdrawRouteError::ZeroTargetAmount);
    }

    if route.is_empty() {
        return Err(WithdrawRouteError::EmptyRoute);
    }

    // Check for zero amounts and duplicates
    let mut seen_targets: Vec<TargetId> = Vec::new();
    for entry in &route.entries {
        if entry.max_amount == 0 {
            return Err(WithdrawRouteError::ZeroMaxAmount {
                target_id: entry.target_id,
            });
        }

        if seen_targets.contains(&entry.target_id) {
            return Err(WithdrawRouteError::DuplicateTarget {
                target_id: entry.target_id,
            });
        }
        seen_targets.push(entry.target_id);
    }

    // Check route total covers target
    let route_total = compute_route_total(route);
    if route_total < route.target_amount {
        return Err(WithdrawRouteError::InsufficientRouteTotal {
            route_total,
            target_amount: route.target_amount,
        });
    }

    Ok(())
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
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    let entries: Vec<WithdrawRouteEntry> = sorted
        .into_iter()
        .map(|(target_id, principal)| WithdrawRouteEntry::new(target_id, principal))
        .collect();

    if entries.is_empty() {
        return Err(WithdrawRouteError::EmptyRoute);
    }

    Ok(WithdrawRoute {
        entries,
        target_amount,
    })
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
    sorted.sort_by(|a, b| b.2.cmp(&a.2));

    let _total_available: u128 = sorted
        .iter()
        .fold(0u128, |acc, (_, _, l)| acc.saturating_add(*l));

    // Use the minimum of principal and available liquidity for each entry
    let entries: Vec<WithdrawRouteEntry> = sorted
        .into_iter()
        .map(|(target_id, principal, liquidity)| {
            let max_amount = principal.min(liquidity);
            WithdrawRouteEntry::with_liquidity(target_id, max_amount, liquidity)
        })
        .filter(|e| e.max_amount > 0)
        .collect();

    if entries.is_empty() {
        return Err(WithdrawRouteError::EmptyRoute);
    }

    let route_total = compute_route_total(&WithdrawRoute {
        entries: entries.clone(),
        target_amount,
    });

    if route_total < target_amount {
        return Err(WithdrawRouteError::InsufficientRouteTotal {
            route_total,
            target_amount,
        });
    }

    Ok(WithdrawRoute {
        entries,
        target_amount,
    })
}

/// Convert a withdraw route to a list of (target_id, amount) pairs.
///
/// This is useful for passing to the withdrawal state machine.
pub fn to_withdrawal_plan(route: &WithdrawRoute) -> Vec<(TargetId, u128)> {
    route
        .entries
        .iter()
        .map(|e| (e.target_id, e.max_amount))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_new_route() {
        let route = WithdrawRoute::new(1000);
        assert!(route.is_empty());
        assert_eq!(route.target_amount, 1000);
    }

    #[test]
    fn test_compute_route_total() {
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 500),
                WithdrawRouteEntry::new(2, 300),
                WithdrawRouteEntry::new(3, 200),
            ],
            1000,
        );

        assert_eq!(compute_route_total(&route), 1000);
    }

    #[test]
    fn test_validate_withdraw_route_success() {
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 500),
                WithdrawRouteEntry::new(2, 600),
            ],
            1000,
        );

        assert!(validate_withdraw_route(&route).is_ok());
    }

    #[test]
    fn test_validate_withdraw_route_zero_target() {
        let route = WithdrawRoute::from_entries(vec![WithdrawRouteEntry::new(1, 500)], 0);

        assert!(matches!(
            validate_withdraw_route(&route),
            Err(WithdrawRouteError::ZeroTargetAmount)
        ));
    }

    #[test]
    fn test_validate_withdraw_route_empty() {
        let route = WithdrawRoute::new(1000);

        assert!(matches!(
            validate_withdraw_route(&route),
            Err(WithdrawRouteError::EmptyRoute)
        ));
    }

    #[test]
    fn test_validate_withdraw_route_insufficient() {
        let route = WithdrawRoute::from_entries(
            vec![WithdrawRouteEntry::new(1, 500)],
            1000, // target > route total
        );

        assert!(matches!(
            validate_withdraw_route(&route),
            Err(WithdrawRouteError::InsufficientRouteTotal { .. })
        ));
    }

    #[test]
    fn test_validate_withdraw_route_duplicate() {
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 500),
                WithdrawRouteEntry::new(1, 600), // duplicate target
            ],
            1000,
        );

        assert!(matches!(
            validate_withdraw_route(&route),
            Err(WithdrawRouteError::DuplicateTarget { target_id: 1 })
        ));
    }

    #[test]
    fn test_validate_withdraw_route_zero_max() {
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 500),
                WithdrawRouteEntry::new(2, 0), // zero max
            ],
            500,
        );

        assert!(matches!(
            validate_withdraw_route(&route),
            Err(WithdrawRouteError::ZeroMaxAmount { target_id: 2 })
        ));
    }

    #[test]
    fn test_build_withdraw_route() {
        let principals = vec![(1, 1000), (2, 500), (3, 300)];

        let route = build_withdraw_route(&principals, 800).unwrap();

        // Should be sorted by principal (largest first)
        assert_eq!(route.entries[0].target_id, 1);
        assert_eq!(route.entries[1].target_id, 2);
        assert_eq!(route.entries[2].target_id, 3);
        assert_eq!(route.target_amount, 800);
    }

    #[test]
    fn test_build_withdraw_route_insufficient() {
        let principals = vec![(1, 100), (2, 50)];

        let result = build_withdraw_route(&principals, 200);

        assert!(matches!(
            result,
            Err(WithdrawRouteError::InsufficientRouteTotal { .. })
        ));
    }

    #[test]
    fn test_build_withdraw_route_with_liquidity() {
        let market_data = vec![
            (1, 1000, 800), // principal 1000, liquidity 800
            (2, 500, 500),  // principal 500, liquidity 500
            (3, 300, 100),  // principal 300, liquidity 100
        ];

        let route = build_withdraw_route_with_liquidity(&market_data, 500).unwrap();

        // Should be sorted by liquidity (highest first)
        assert_eq!(route.entries[0].target_id, 1);
        assert_eq!(route.entries[0].max_amount, 800); // min(1000, 800)
        assert_eq!(route.entries[0].available_liquidity, Some(800));
    }

    #[test]
    fn test_compute_available_liquidity() {
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::with_liquidity(1, 500, 400),
                WithdrawRouteEntry::new(2, 300), // no liquidity info
                WithdrawRouteEntry::with_liquidity(3, 200, 200),
            ],
            1000,
        );

        assert_eq!(compute_available_liquidity(&route), 600);
    }

    #[test]
    fn test_to_withdrawal_plan() {
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 500),
                WithdrawRouteEntry::new(2, 300),
            ],
            800,
        );

        let plan = to_withdrawal_plan(&route);

        assert_eq!(plan, vec![(1, 500), (2, 300)]);
    }
}
