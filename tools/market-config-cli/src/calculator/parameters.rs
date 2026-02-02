use crate::{CliError, CliResult};
use std::str::FromStr;
use templar_common::{
    interest_rate_strategy::InterestRateStrategy, market::APY_LIMIT, number::Decimal,
};

pub struct InterestRateCalculator;

pub struct CurveParameters {
    pub starting_rate: Decimal,
    pub optimal_rate: Decimal,
    pub optimal_usage: Decimal,
    pub max_rate: Decimal,
    pub display_points: usize,
}

impl InterestRateCalculator {
    pub fn new() -> Self {
        Self
    }

    /// Calculate a piecewise linear interest rate curve from the given parameters
    ///
    /// # Parameters
    /// - `starting_rate`: Base interest rate (APY as decimal, e.g., "0.05" for 5%)
    /// - `optimal_rate`: Interest rate at optimal usage (APY as decimal)
    /// - `optimal_usage`: Optimal utilization ratio (0.0-1.0, e.g., "0.8" for 80%)
    /// - `max_rate`: Maximum interest rate at 100% usage (APY as decimal)
    ///
    /// # Returns
    /// An `InterestRateStrategy` configured with the calculated parameters
    /// # Errors
    pub fn calculate_piecewise(
        &self,
        starting_rate: Decimal,
        optimal_rate: Decimal,
        optimal_usage: Decimal,
        max_rate: Decimal,
    ) -> CliResult<InterestRateStrategy> {
        // Validate inputs
        validate_inputs(starting_rate, optimal_rate, optimal_usage, max_rate)?;

        // Convert target rates at the breakpoints into slopes for the piecewise segments.
        let denom_one = optimal_usage;
        let denom_two = Decimal::ONE - optimal_usage;

        if denom_one.is_zero() || denom_two.is_zero() {
            return Err(CliError::InvalidInput(
                "Optimal usage must be between 0 and 1 (exclusive)".into(),
            ));
        }

        let slope_one = (optimal_rate - starting_rate) / denom_one;
        let slope_two = (max_rate - optimal_rate) / denom_two;

        if slope_one > slope_two {
            return Err(CliError::InvalidInput(
                "Cannot build piecewise curve: slope before optimal exceeds slope after optimal"
                    .into(),
            ));
        }

        InterestRateStrategy::piecewise(starting_rate, optimal_usage, slope_one, slope_two)
            .ok_or_else(|| CliError::InvalidInput("Failed to create interest rate strategy".into()))
    }

    /// Calculate a linear interest rate curve
    ///
    /// # Parameters
    /// - `base_rate`: Base interest rate at 0% usage
    /// - `top_rate`: Interest rate at 100% usage
    /// # Errors
    pub fn calculate_linear(
        &self,
        base_rate: Decimal,
        top_rate: Decimal,
    ) -> CliResult<InterestRateStrategy> {
        if top_rate < base_rate {
            return Err(CliError::InvalidInput(
                "Top rate must be greater than or equal to base rate".into(),
            ));
        }

        InterestRateStrategy::linear(base_rate, top_rate)
            .ok_or_else(|| CliError::InvalidInput("Failed to create linear strategy".into()))
    }

    /// Calculate an exponential (2^k) interest rate curve
    /// # Errors
    pub fn calculate_exponential2(
        &self,
        base_rate: Decimal,
        top_rate: Decimal,
        eccentricity: Decimal,
    ) -> CliResult<InterestRateStrategy> {
        if eccentricity.is_zero() {
            return Err(CliError::InvalidInput(
                "Eccentricity must be greater than zero".into(),
            ));
        }

        InterestRateStrategy::exponential2(base_rate, top_rate, eccentricity).ok_or_else(|| {
            CliError::InvalidInput("Failed to create exponential interest rate strategy".into())
        })
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Affects only display, not calculations"
    )]
    /// Helper to display interest rate curves for visualization
    pub fn display_curve(&self, strategy: &InterestRateStrategy, points: usize) -> Vec<(f64, f64)> {
        let samples = points.max(2); // ensure endpoints
        let step = 1.0 / (samples as f64 - 1.0);

        let mut result = Vec::with_capacity(samples);
        println!("\nUtilization\tRate (APY)");
        for i in 0..samples {
            let utilization = (i as f64 * step).min(1.0);
            let util_decimal = Decimal::from_str(&utilization.to_string()).unwrap_or(Decimal::ZERO);
            let rate = strategy.at(util_decimal);
            let rate_f64 = rate.to_string().parse::<f64>().unwrap_or(0.0);
            println!("{utilization:>6.2}\t\t{rate_f64:>8.4}");
            result.push((utilization, rate_f64));
        }

        // Render a simple ASCII curve with utilization on X and rate on Y
        let max_rate = result.iter().map(|(_, r)| *r).fold(0.0_f64, f64::max);

        if max_rate > 0.0 {
            let height = 10_usize;
            let width = 50_usize;
            let mut grid = vec![vec![' '; width]; height];

            for (util, rate) in result.iter().copied() {
                let x = ((util * (width as f64 - 1.0)).round() as usize).min(width - 1);
                let y_scaled = (rate / max_rate) * (height as f64 - 1.0);
                let y = height - 1 - y_scaled.round().clamp(0.0, height as f64 - 1.0) as usize;
                grid[y][x] = '*';
            }

            let label = match strategy {
                InterestRateStrategy::Linear(_) => "Linear Curve",
                InterestRateStrategy::Piecewise(_) => "Piecewise Curve",
                InterestRateStrategy::Exponential2(_) => "Exponential Curve",
            };

            println!("\n {label} (utilization on X, rate on Y):");
            for (row_idx, row) in grid.iter().enumerate() {
                let label_rate = max_rate * (1.0 - row_idx as f64 / (height as f64 - 1.0));
                let line: String = row.iter().collect();
                println!("{label_rate:>8.4} |{line}");
            }
            println!("        +{}", "-".repeat(width));
            println!("         0%{}100%", " ".repeat(width.saturating_sub(5)));
        } else {
            println!("\n(All rates are zero; skipping curve plot)");
        }

        result
    }
}

fn validate_inputs(
    starting: Decimal,
    optimal: Decimal,
    optimal_util: Decimal,
    maximum: Decimal,
) -> CliResult {
    // Check that rates are non-negative
    if starting < Decimal::ZERO
        || optimal < Decimal::ZERO
        || optimal_util < Decimal::ZERO
        || maximum < Decimal::ZERO
    {
        return Err(CliError::InvalidInput("Rates must be non-negative".into()));
    }

    // Check that optimal usage is between 0 and 1
    if optimal_util >= Decimal::ONE || optimal_util.is_zero() {
        return Err(CliError::InvalidInput(
            "Optimal usage must be between 0 and 1 (exclusive)".into(),
        ));
    }

    // Check that rates are in logical order
    if starting > optimal {
        return Err(CliError::InvalidInput(
            "Starting rate should not exceed optimal rate".into(),
        ));
    }

    if optimal > maximum {
        return Err(CliError::InvalidInput(
            "Optimal rate should not exceed maximum rate".into(),
        ));
    }

    // Check against APY limit
    if maximum > APY_LIMIT {
        return Err(CliError::InvalidInput(format!(
            "Maximum rate exceeds APY limit of {APY_LIMIT}",
        )));
    }

    Ok(())
}

impl Default for InterestRateCalculator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close_enough(a: Decimal, b: Decimal, epsilon: Decimal) -> bool {
        if a >= b {
            (a - b) <= epsilon
        } else {
            (b - a) <= epsilon
        }
    }

    #[test]
    fn test_calculate_piecewise() {
        let calculator = InterestRateCalculator::new();
        let strategy = calculator
            .calculate_piecewise(
                Decimal::from_str("0.02").unwrap(),
                Decimal::from_str("0.10").unwrap(),
                Decimal::from_str("0.80").unwrap(),
                Decimal::from_str("0.50").unwrap(),
            )
            .unwrap();

        // Test at 0% utilization
        let rate_zero = strategy.at(Decimal::ZERO);
        assert!(rate_zero >= Decimal::from_str("0.02").unwrap());

        // Test at 80% utilization (optimal)
        let rate_optimal = strategy.at(Decimal::from_str("0.80").unwrap());
        assert!(rate_optimal >= Decimal::from_str("0.08").unwrap());

        // Test at 100% utilization
        let rate_max = strategy.at(Decimal::ONE);
        assert!(rate_max >= Decimal::from_str("0.20").unwrap());
    }

    #[test]
    fn test_piecewise_rates_match_points() {
        let calculator = InterestRateCalculator::new();
        let strategy = calculator
            .calculate_piecewise(
                Decimal::from_str("0.02").unwrap(),
                Decimal::from_str("0.23").unwrap(),
                Decimal::from_str("0.90").unwrap(),
                Decimal::from_str("0.34").unwrap(),
            )
            .unwrap();

        let epsilon = Decimal::from_str("0.000000000000000001").unwrap();
        let expected_zero = Decimal::from_str("0.02").unwrap();
        let expected_opt = Decimal::from_str("0.23").unwrap();
        let expected_max = Decimal::from_str("0.34").unwrap();

        let at_zero = strategy.at(Decimal::ZERO);
        let at_opt = strategy.at(Decimal::from_str("0.90").unwrap());
        let at_max = strategy.at(Decimal::ONE);

        assert!(close_enough(at_zero, expected_zero, epsilon));
        assert!(close_enough(at_opt, expected_opt, epsilon));
        assert!(close_enough(at_max, expected_max, epsilon));
    }

    #[test]
    fn test_calculate_linear() {
        let calculator = InterestRateCalculator::new();
        let strategy = calculator
            .calculate_linear(
                Decimal::from_str("0.05").unwrap(),
                Decimal::from_str("0.10").unwrap(),
            )
            .unwrap();

        // Test at 0% utilization
        let rate_zero = strategy.at(Decimal::ZERO);
        assert_eq!(rate_zero, Decimal::from_str("0.05").unwrap());

        // Test at 100% utilization
        let rate_max = strategy.at(Decimal::ONE);
        assert_eq!(rate_max, Decimal::from_str("0.10").unwrap());
    }

    #[test]
    fn exponential_rejects_zero_eccentricity() {
        let calculator = InterestRateCalculator::new();
        let err = calculator
            .calculate_exponential2(
                Decimal::from_str("0.02").unwrap(),
                Decimal::from_str("0.10").unwrap(),
                Decimal::ZERO,
            )
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("Eccentricity must be greater than zero"));
    }
}
