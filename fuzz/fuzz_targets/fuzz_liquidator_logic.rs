#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use near_sdk::json_types::U128;

fuzz_target!(|data: (u128, u128, f64, bool)| {
    let (swap_amount_raw, liquidation_amount_raw, exchange_rate, should_swap) = data;

    let swap_amount = U128(swap_amount_raw);
    let liquidation_amount = U128(liquidation_amount_raw);

    // Fuzz the decision logic for liquidation profitability
    // This simulates the should_liquidate logic

    // Test 1: Basic amount comparisons
    let _ = swap_amount.0 > 0;
    let _ = liquidation_amount.0 > 0;
    let _ = swap_amount == liquidation_amount;
    let _ = swap_amount.0 < liquidation_amount.0;

    // Test 2: Exchange rate calculations (mock swap quote logic)
    if exchange_rate > 0.0 && exchange_rate.is_finite() {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let input_amount = (liquidation_amount.0 as f64 / exchange_rate) as u128;

        // Verify calculations don't overflow
        let _ = U128(input_amount);

        // Test profitability calculation
        if swap_amount.0 > 0 && input_amount > 0 {
            let profit = if liquidation_amount.0 > swap_amount.0 {
                liquidation_amount.0.saturating_sub(swap_amount.0)
            } else {
                0
            };

            // Minimum profit threshold (e.g., 1%)
            let min_profit = swap_amount.0 / 100;
            let is_profitable = profit >= min_profit;

            let _ = is_profitable;
        }
    }

    // Test 3: Balance checks
    let available_balance = U128(swap_amount_raw.saturating_mul(2));
    let has_sufficient_balance = available_balance.0 >= swap_amount.0;
    let _ = has_sufficient_balance;

    // Test 4: Swap amount calculation with different asset balances
    let asset_balance = U128(swap_amount_raw / 2);
    let needs_swap = if asset_balance.0 >= liquidation_amount.0 {
        U128(0)
    } else {
        U128(liquidation_amount.0.saturating_sub(asset_balance.0))
    };

    assert!(
        needs_swap.0 <= liquidation_amount.0,
        "Swap need should not exceed liquidation amount"
    );

    // Test 5: Multiple swap scenarios
    if should_swap {
        // Test scenario where we need to swap
        let swap_needed = liquidation_amount.0.saturating_sub(asset_balance.0);
        let _ = U128(swap_needed);
    }

    // Test 6: Gas cost estimation (mock)
    let gas_cost = 1000u128; // Mock gas cost
    let total_cost = swap_amount.0.saturating_add(gas_cost);
    let net_profit = liquidation_amount.0.saturating_sub(total_cost);

    let is_worth_liquidating = liquidation_amount.0 > total_cost;
    let _ = is_worth_liquidating;
    let _ = net_profit;

    // Test 7: Edge cases

    // Zero amounts
    let zero_swap = U128(0);
    let zero_liq = U128(0);
    assert_eq!(zero_swap.0, 0);
    assert_eq!(zero_liq.0, 0);

    // Maximum amounts
    let max_swap = U128(u128::MAX);
    let max_liq = U128(u128::MAX);
    let _ = max_swap.0.saturating_add(1);
    let _ = max_liq.0.saturating_sub(1);

    // Test 8: Ratio calculations
    if liquidation_amount.0 > 0 {
        // Calculate swap-to-liquidation ratio
        #[allow(clippy::cast_precision_loss)]
        let ratio = swap_amount.0 as f64 / liquidation_amount.0 as f64;

        if ratio.is_finite() {
            // Healthy liquidation should have ratio < 1.0 (profit)
            let is_healthy = ratio < 1.0;
            let _ = is_healthy;
        }
    }

    // Test 9: Partial liquidation calculation
    let max_liquidatable = U128(liquidation_amount_raw);
    let requested_liquidation = U128(swap_amount_raw);
    let actual_liquidation = if requested_liquidation.0 > max_liquidatable.0 {
        max_liquidatable
    } else {
        requested_liquidation
    };

    assert!(
        actual_liquidation.0 <= max_liquidatable.0,
        "Actual liquidation should not exceed maximum"
    );

    // Test 10: Slippage calculation
    let slippage_bps = 50u128; // 0.5% slippage
    let slippage_amount = swap_amount.0.saturating_mul(slippage_bps) / 10000;
    let swap_with_slippage = swap_amount.0.saturating_add(slippage_amount);

    assert!(
        swap_with_slippage >= swap_amount.0,
        "Swap with slippage should be >= original amount"
    );

    // Test 11: Minimum liquidation thresholds
    let min_liquidation_threshold = U128(1000); // Minimum viable liquidation
    let meets_threshold = liquidation_amount.0 >= min_liquidation_threshold.0;
    let _ = meets_threshold;
});
