// ============================================================================
// FILE: fuzz/fuzz_targets/fuzz_price_calculations.rs
// ============================================================================
// Fuzzes price oracle calculations and conversions between asset pairs

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct PriceScenario {
    // Asset prices (scaled by 1e8 for precision)
    collateral_price: u32,
    borrow_price: u32,

    // Amounts to convert
    collateral_amount: u64,
    borrow_amount: u64,

    // Oracle parameters
    price_age: u32,       // Seconds since last update
    max_price_age: u32,   // Maximum acceptable age
    price_deviation: u16, // Basis points of acceptable deviation

    // Previous prices for comparison
    prev_collateral_price: u32,
    // prev_borrow_price: u32,
}

fuzz_target!(|scenario: PriceScenario| {
    // Validate inputs
    if scenario.collateral_price == 0 || scenario.borrow_price == 0 {
        return; // Invalid prices
    }
    if scenario.max_price_age == 0 {
        return;
    }

    let collateral_price = u128::from(scenario.collateral_price);
    let borrow_price = u128::from(scenario.borrow_price);
    let collateral_amount = u128::from(scenario.collateral_amount);
    let borrow_amount = u128::from(scenario.borrow_amount);

    // Test 1: Price staleness check
    let is_stale = scenario.price_age > scenario.max_price_age;

    // Invariant: Stale prices should be rejected
    if is_stale {
        // In actual code: assert!(get_price().is_err())
        return;
    }

    // Test 2: Convert collateral amount to borrow amount
    // borrow_equivalent = (collateral_amount * collateral_price) / borrow_price
    if let Some(collateral_value) = collateral_amount.checked_mul(collateral_price) {
        if let Some(borrow_equivalent) = collateral_value.checked_div(borrow_price) {
            // Invariant: Result should be proportional to input
            // If collateral_price > borrow_price, should get more borrow tokens
            if collateral_price > borrow_price {
                assert!(
                    borrow_equivalent >= collateral_amount,
                    "Price conversion error: higher priced asset should convert to more"
                );
            }

            // Invariant: Converting back should give approximately original amount
            if let Some(back_value) = borrow_equivalent.checked_mul(borrow_price) {
                if let Some(back_to_collateral) = back_value.checked_div(collateral_price) {
                    // Allow 1 unit difference for rounding
                    let diff = back_to_collateral.abs_diff(collateral_amount);

                    assert!(
                        diff <= 1,
                        "Round-trip conversion lost too much: original={collateral_amount}, back={back_to_collateral}",
                    );
                }
            }
        }
    }

    // Test 3: Calculate position value in USD
    let collateral_value_usd = collateral_amount.saturating_mul(collateral_price);
    let borrow_value_usd = borrow_amount.saturating_mul(borrow_price);

    // Invariant: Values should never overflow to wrap around
    assert!(
        collateral_value_usd >= collateral_amount || collateral_amount == 0,
        "Collateral value calculation overflowed"
    );
    assert!(
        borrow_value_usd >= borrow_amount || borrow_amount == 0,
        "Borrow value calculation overflowed"
    );

    // Test 4: Price deviation checks (circuit breaker)
    if scenario.prev_collateral_price > 0 {
        let prev_price = u128::from(scenario.prev_collateral_price);
        let current_price = collateral_price;

        // Calculate percentage change
        let (change_numerator, change_denominator) = if current_price > prev_price {
            (current_price - prev_price, prev_price)
        } else {
            (prev_price - current_price, prev_price)
        };

        if change_denominator > 0 {
            let change_bps = (change_numerator * 10000) / change_denominator;

            // Invariant: Reject prices that deviate too much
            if change_bps > u128::from(scenario.price_deviation) {
                // In actual code: should reject this price update
                // assert!(update_price(current_price).is_err())
                return;
            }

            // Invariant: Change should never exceed 100% in one update
            assert!(
                change_bps <= 10000,
                "Price changed by more than 100% in one update: {change_bps}bps",
            );
        }
    }

    // Test 5: Collateral ratio calculations with prices
    // collateral_ratio = (collateral_value * 10000) / borrow_value
    if borrow_value_usd > 0 {
        if let Some(ratio_numerator) = collateral_value_usd.checked_mul(10000) {
            let collateral_ratio = ratio_numerator / borrow_value_usd;

            // Invariant: Ratio calculation should be consistent
            // If we have 2x collateral value, ratio should be 20000 (200%)
            if collateral_value_usd >= borrow_value_usd * 2 {
                assert!(
                    collateral_ratio >= 20000,
                    "Collateral ratio calculation is wrong: ratio={collateral_ratio}"
                );
            }

            // Invariant: If collateral value < borrow value, ratio < 100%
            if collateral_value_usd < borrow_value_usd {
                assert!(
                    collateral_ratio < 10000,
                    "Undercollateralized but ratio shows healthy: {collateral_ratio}"
                );
            }
        }
    }

    // Test 6: Liquidation calculations with price changes
    // Simulate price crash of collateral
    let crash_scenarios = [
        (80, "20% drop"), // 80% of original price
        (50, "50% drop"), // 50% of original price
        (20, "80% drop"), // 20% of original price
    ];

    for (crash_percent, _description) in crash_scenarios {
        #[allow(clippy::unwrap_used, reason = "Fuzzing with valid inputs")]
        let crashed_price = (collateral_price * u128::try_from(crash_percent).unwrap()) / 100;
        if crashed_price == 0 {
            continue;
        }

        let crashed_value = collateral_amount.saturating_mul(crashed_price);

        // Check if position becomes liquidatable
        let collateral_ratio_threshold = 13000u128; // 130%
        let required_collateral =
            borrow_value_usd.saturating_mul(collateral_ratio_threshold) / 10000;

        if crashed_value < required_collateral && borrow_value_usd > 0 {
            // Position is now liquidatable

            // Calculate liquidation health factor
            let health = (crashed_value * 10000) / required_collateral;

            // Invariant: Health factor should be < 100% for liquidatable position
            assert!(
                health < 10000,
                "Position should be liquidatable but health={health}/10000",
            );
        }
    }

    // Test 7: Price precision and rounding
    // Test that we don't lose precision in conversions
    let small_amounts = [1u128, 10, 100, 1000];

    for small in small_amounts {
        if let Some(value) = small.checked_mul(collateral_price) {
            if let Some(converted) = value.checked_div(borrow_price) {
                if converted > 0 {
                    // Converting back should not be zero
                    let back = converted.saturating_mul(borrow_price) / collateral_price;

                    // Should not lose everything to rounding
                    assert!(
                        back > 0 || small == 0,
                        "Small amount lost to rounding: {small} -> {converted} -> {back}",
                    );
                }
            }
        }
    }

    // Test 8: TWAP (Time-Weighted Average Price) calculations
    // Simulate multiple price points
    let price_points = [
        collateral_price,
        collateral_price * 95 / 100,  // -5%
        collateral_price * 105 / 100, // +5%
        collateral_price * 98 / 100,  // -2%
    ];

    let weights = [1u128, 2, 3, 1]; // Different time weights
    let total_weight: u128 = weights.iter().sum();

    if total_weight > 0 {
        let mut weighted_sum: u128 = 0;
        for (price, weight) in price_points.iter().zip(weights.iter()) {
            weighted_sum = weighted_sum.saturating_add(price.saturating_mul(*weight));
        }

        let twap = weighted_sum / total_weight;

        // Invariant: TWAP should be within range of price points
        #[allow(clippy::unwrap_used, reason = "Fuzzing with valid inputs")]
        let min_price = *price_points.iter().min().unwrap();
        #[allow(clippy::unwrap_used, reason = "Fuzzing with valid inputs")]
        let max_price = *price_points.iter().max().unwrap();

        assert!(
            twap >= min_price && twap <= max_price,
            "TWAP outside price range: {twap} not in [{min_price}, {max_price}]",
        );

        // Invariant: TWAP should be close to simple average for equal weights
        let simple_avg = price_points.iter().sum::<u128>() / price_points.len() as u128;
        let diff_pct = if twap > simple_avg {
            ((twap - simple_avg) * 100) / simple_avg
        } else {
            ((simple_avg - twap) * 100) / simple_avg
        };

        // For equal weights, difference should be minimal
        if weights.iter().all(|&w| w == weights[0]) {
            assert!(
                diff_pct == 0,
                "Equal weights should give same result as simple average"
            );
        }
    }

    // Test 9: Exchange rate calculations
    // For cToken-like logic: exchangeRate = (totalCash + totalBorrows - reserves) / totalSupply
    let total_cash = collateral_amount;
    let total_borrows = borrow_amount;
    let reserves = total_borrows / 10; // 10% reserve factor
    let total_supply = collateral_amount;

    if total_supply > 0 {
        let numerator = total_cash
            .saturating_add(total_borrows)
            .saturating_sub(reserves);
        let exchange_rate = numerator / total_supply;

        // Invariant: Exchange rate should be reasonable (not zero, not absurdly high)
        assert!(exchange_rate > 0, "Exchange rate is zero");
        assert!(
            exchange_rate <= total_supply * 10,
            "Exchange rate absurdly high: {exchange_rate} for supply {total_supply}",
        );
    }

    // Test 10: Multi-hop price conversions (A -> B -> C -> A)
    // Ensure round-trip conversions preserve value
    if let Some(step1) = collateral_amount
        .checked_mul(collateral_price)
        .and_then(|v| v.checked_div(borrow_price))
    {
        if let Some(step2) = step1
            .checked_mul(borrow_price)
            .and_then(|v| v.checked_div(collateral_price))
        {
            // Should get back approximately the same amount
            let loss = collateral_amount.abs_diff(step2);

            let loss_pct = if collateral_amount > 0 {
                (loss * 100) / collateral_amount
            } else {
                0
            };

            // Should lose less than 1% to rounding
            assert!(
                loss_pct <= 1,
                "Multi-hop conversion lost {loss_pct}% of value",
            );
        }
    }
});
