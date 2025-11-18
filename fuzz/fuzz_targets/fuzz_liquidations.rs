// Fuzzes liquidation logic to ensure it's profitable and fair

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct LiquidationScenario {
    // Borrower's position
    collateral_amount: u64,
    borrowed_amount: u64,

    // Price oracle data
    collateral_price: u32, // Price in USD (scaled by 1e6)
    borrow_price: u32,

    // Liquidation attempt
    liquidation_amount: u64,

    // Protocol parameters
    collateral_ratio: u16,
    liquidation_threshold: u16,
    liquidation_bonus: u16,
}

fuzz_target!(|scenario: LiquidationScenario| {
    // Validate inputs
    if scenario.collateral_price == 0 || scenario.borrow_price == 0 {
        return;
    }
    if scenario.collateral_ratio < 10000 {
        // Must be > 100%
        return;
    }
    if scenario.liquidation_threshold >= scenario.collateral_ratio {
        return;
    }
    if scenario.liquidation_bonus < 10000 || scenario.liquidation_bonus > 12000 {
        return;
    }
    let u64_max = u128::from(u64::MAX);

    // Scale amounts to avoid overflow
    let collateral = u128::from(scenario.collateral_amount).min(u64_max / 1_000_000);
    let borrowed = u128::from(scenario.borrowed_amount).min(u64_max / 1_000_000);
    let liquidate_amount = u128::from(scenario.liquidation_amount).min(borrowed);

    let collateral_price = u128::from(scenario.collateral_price);
    let borrow_price = u128::from(scenario.borrow_price);

    // Calculate position values
    let collateral_value = collateral.saturating_mul(collateral_price);
    let borrowed_value = borrowed.saturating_mul(borrow_price);

    if borrowed_value == 0 {
        return;
    }

    // Calculate health factor
    // health_factor = (collateral_value * liquidation_threshold) / (borrowed_value * 10000)
    let health_numerator =
        collateral_value.saturating_mul(u128::from(scenario.liquidation_threshold));
    let health_denominator = borrowed_value.saturating_mul(10000);

    if health_denominator == 0 {
        return;
    }

    let health_factor = health_numerator / health_denominator;

    // Test 2: Calculate maximum liquidatable amount
    // Typically limited to 50% of debt or full debt if near insolvency
    let max_liquidate = if health_factor < 5000 {
        borrowed // Can liquidate full position if very underwater
    } else {
        borrowed / 2 // Max 50% otherwise
    };

    let actual_liquidate = liquidate_amount.min(max_liquidate);

    // Test 3: Calculate collateral to seize
    // seized_collateral = (liquidate_amount * borrow_price * liquidation_bonus) / collateral_price
    let seized_value = actual_liquidate
        .saturating_mul(borrow_price)
        .saturating_mul(u128::from(scenario.liquidation_bonus))
        / 10000;

    let seized_collateral = if collateral_price > 0 {
        seized_value / collateral_price
    } else {
        return;
    };

    // Invariant 1: Seized collateral shouldn't exceed available collateral
    assert!(
        seized_collateral <= collateral,
        "Liquidation tried to seize more collateral than available: {seized_collateral} > {collateral}",
    );

    // Invariant 2: Liquidator profit is bounded by liquidation bonus
    let liquidator_profit_value =
        seized_value.saturating_sub(actual_liquidate.saturating_mul(borrow_price));
    let max_profit = actual_liquidate
        .saturating_mul(borrow_price)
        .saturating_mul(u128::from(scenario.liquidation_bonus.saturating_sub(10000)))
        / 10000;

    assert!(
        liquidator_profit_value <= max_profit,
        "Liquidator profit exceeds bonus: {liquidator_profit_value} > {max_profit}",
    );

    // Invariant 3: Borrowed amount decreases by liquidation amount
    let new_borrowed = borrowed.saturating_sub(actual_liquidate);
    assert!(
        new_borrowed < borrowed || actual_liquidate == 0,
        "Borrowed amount didn't decrease"
    );

    // Invariant 4: After liquidation, remaining position should be healthier
    let remaining_collateral = collateral.saturating_sub(seized_collateral);
    let remaining_borrowed_value = new_borrowed.saturating_mul(borrow_price);

    if remaining_borrowed_value > 0 && remaining_collateral > 0 {
        let remaining_collateral_value = remaining_collateral.saturating_mul(collateral_price);
        let new_health_numerator =
            remaining_collateral_value.saturating_mul(u128::from(scenario.liquidation_threshold));
        let new_health_factor =
            new_health_numerator / remaining_borrowed_value.saturating_mul(10000);

        // New health should be >= old health (position improved)
        // Allow small margin for rounding
        assert!(
            new_health_factor >= health_factor.saturating_sub(100),
            "Liquidation made position worse: old_health={health_factor} new_health={new_health_factor}",
        );
    }

    // Invariant 5: Protocol shouldn't lose money
    // Total value of (borrowed repaid + remaining collateral) >= original collateral value
    let repaid_value = actual_liquidate.saturating_mul(borrow_price);
    let remaining_value = remaining_collateral.saturating_mul(collateral_price);
    let total_recovered = repaid_value.saturating_add(remaining_value);

    // This might not hold for severely underwater positions, but check it's reasonable
    if health_factor > 7000 {
        // If not too underwater
        assert!(
            total_recovered <= collateral_value.saturating_mul(12000) / 10000,
            "Protocol recovered too much value somehow"
        );
    }

    // Test 4: Partial liquidations
    if actual_liquidate < borrowed {
        // After partial liquidation, some debt should remain
        assert!(new_borrowed > 0, "Partial liquidation cleared all debt");

        // And some collateral should remain
        assert!(
            remaining_collateral > 0,
            "Partial liquidation took all collateral"
        );
    }

    // Test 5: Multiple liquidations
    // Simulate multiple small liquidations vs one large one
    let num_liquidations = 3u128;
    let small_liquidation = actual_liquidate / num_liquidations;

    if small_liquidation > 0 {
        let mut running_collateral = collateral;
        let mut running_debt = borrowed;

        for _ in 0..num_liquidations {
            if running_debt == 0 {
                break;
            }

            let small_seized_value = small_liquidation
                .saturating_mul(borrow_price)
                .saturating_mul(u128::from(scenario.liquidation_bonus))
                / 10000;
            let small_seized = small_seized_value / collateral_price;

            running_collateral = running_collateral.saturating_sub(small_seized);
            running_debt = running_debt.saturating_sub(small_liquidation);
        }

        // Multiple small liquidations shouldn't be significantly worse than one large one
        // (Within rounding error)
        let diff = remaining_collateral.abs_diff(running_collateral);

        // Allow 1% difference for rounding
        assert!(
            diff <= collateral / 100,
            "Multiple liquidations deviate too much from single liquidation"
        );
    }
});
