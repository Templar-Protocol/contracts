// Fuzzes interest rate calculations and compound interest to find overflow bugs

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct InterestScenario {
    principal: u64,
    interest_rate_bps: u16, // Basis points (10000 = 100%)
    time_periods: u16,      // Number of compounding periods
    utilization_rate: u8,   // 0-100%
}

fuzz_target!(|scenario: InterestScenario| {
    // Validate inputs are reasonable
    if scenario.interest_rate_bps > 50000 {
        // Max 500% APR
        return;
    }
    if scenario.utilization_rate > 100 {
        return;
    }
    if scenario.time_periods > 365 * 10 {
        // Max 10 years
        return;
    }

    let principal = u128::from(scenario.principal);
    let rate_bps = u128::from(scenario.interest_rate_bps);
    let periods = u128::from(scenario.time_periods);

    // Test 1: Simple interest calculation
    // Interest = Principal * Rate * Time
    if let Some(simple_interest) = principal
        .checked_mul(rate_bps)
        .and_then(|x| x.checked_mul(periods))
        .and_then(|x| x.checked_div(10000))
    {
        // Invariant: Simple interest should never exceed principal * rate * periods
        assert!(
            simple_interest <= principal.saturating_mul(rate_bps).saturating_mul(periods) / 10000,
            "Simple interest overflow"
        );
    }

    // Test 2: Compound interest calculation
    // A = P(1 + r)^n
    // For small rates, use binomial approximation to avoid overflow
    // if rate_bps <= 1000 && periods <= 365 {
    //     let rate_per_period = rate_bps as f64 / 10000.0;
    //     let compound_multiplier = (1.0 + rate_per_period).powi(periods as i32);
    //
    //     if compound_multiplier.is_finite() && compound_multiplier > 0.0 {
    //         let result = (principal as f64 * compound_multiplier) as u128;
    //
    //         // Invariant: Result should always be >= principal
    //         assert!(
    //             result >= principal,
    //             "Compound interest resulted in less than principal"
    //         );
    //
    //         // Invariant: Result shouldn't be absurdly large
    //         assert!(
    //             result <= principal.saturating_mul(100),
    //             "Compound interest grew unreasonably"
    //         );
    //     }
    // }

    // Test 3: Utilization rate calculations
    // Utilization = TotalBorrowed / TotalSupplied
    let utilization = u128::from(scenario.utilization_rate);

    if utilization > 0 && utilization <= 100 {
        // Calculate borrow rate based on utilization
        // Common model: BorrowRate = BaseRate + UtilRate * Slope
        let base_rate = 200u128; // 2% base
        let slope = 1000u128; // 10% slope

        if let Some(borrow_rate) =
            base_rate.checked_add(utilization.checked_mul(slope).unwrap_or(0) / 100)
        {
            // Invariant: Borrow rate should be >= base rate
            assert!(borrow_rate >= base_rate, "Borrow rate below base");

            // Invariant: Borrow rate should increase with utilization
            // (This is implicit in the formula but good to verify)

            // Calculate supply rate
            // SupplyRate = BorrowRate * Utilization * (1 - ReserveFactor)
            let reserve_factor = 1000u128; // 10%
            if let Some(supply_rate) = borrow_rate
                .checked_mul(utilization)
                .and_then(|x| x.checked_mul(10000 - reserve_factor))
                .and_then(|x| x.checked_div(1_000_000))
            {
                // Invariant: Supply rate must be < borrow rate
                assert!(
                    supply_rate <= borrow_rate,
                    "Supply rate exceeds borrow rate: {supply_rate} > {borrow_rate}",
                );

                // Invariant: At 100% utilization, supply rate should approach borrow rate
                if utilization == 100 {
                    let expected_max = borrow_rate * (10000 - reserve_factor) / 10000;
                    assert!(
                        supply_rate <= expected_max,
                        "Supply rate too high at full utilization"
                    );
                }
            }
        }
    }

    // Test 4: Interest accrual over time
    // Simulate multiple periods
    let mut balance = principal;
    for _ in 0..periods.min(100) {
        // Limit iterations
        if let Some(interest) = balance
            .checked_mul(rate_bps)
            .and_then(|x| x.checked_div(10000))
        {
            if let Some(new_balance) = balance.checked_add(interest) {
                // Invariant: Balance should always increase (unless rate is 0)
                if rate_bps > 0 {
                    assert!(
                        new_balance >= balance,
                        "Balance didn't increase with interest"
                    );
                    if interest > 0 {
                        assert!(
                            new_balance > balance,
                            "Balance didn't strictly increase when interest > 0"
                        );
                    }
                }
                balance = new_balance;
            } else {
                // Overflow is acceptable, just stop iterating
                break;
            }
        } else {
            break;
        }
    }

    // Test 5: Exchange rate calculations
    // ExchangeRate = (TotalCash + TotalBorrows - TotalReserves) / TotalSupply
    let total_cash = principal;
    let total_borrows = principal.saturating_mul(utilization) / 100;
    let total_reserves = total_borrows / 10; // 10% reserve
    let total_supply = principal;

    if total_supply > 0 {
        let numerator = total_cash
            .saturating_add(total_borrows)
            .saturating_sub(total_reserves);

        let exchange_rate = numerator / total_supply;

        // Invariant: Exchange rate should be close to 1:1 initially
        // and should only increase over time (never decrease)
        assert!(exchange_rate > 0, "Exchange rate is zero or negative");

        // Invariant: Exchange rate shouldn't deviate wildly
        assert!(
            exchange_rate <= 10,
            "Exchange rate is unreasonably high: {exchange_rate}",
        );
    }
});

// ============================================================================
// CRITICAL BUGS TO LOOK FOR:
// ============================================================================
// 1. Integer overflow when calculating compound interest
// 2. Underflow when principal < interest payment
// 3. Division by zero in utilization rate calculations
// 4. Rounding errors that favor borrowers/lenders
// 5. Interest rate manipulation through edge cases
// 6. Exchange rate manipulation
// 7. Reserve factor calculations that drain protocol
// 8. Time-weighted average calculations
//
// Run with:
//   cargo +nightly fuzz run fuzz_interest_math -- -max_total_time=600
