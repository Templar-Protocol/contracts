use crate::{
    logger,
    ui::prompt::{
        error::map_dialoguer_err,
        helpers::prompt_decimal,
        types::{StrategyDefaults, StrategyKind},
        wizard::display::strategy_label,
    },
    CliError, CliResult, ConfigBuilder, InterestRateCalculator,
};
use dialoguer::{theme::ColorfulTheme, Select};
use templar_common::{
    interest_rate_strategy::InterestRateStrategy, market::MarketConfiguration, number::Decimal,
};

/// Returns the default interest rate strategies for selection.
pub fn default_interest_rate_strategies() -> CliResult<Vec<InterestRateStrategy>> {
    Ok(vec![
        InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO)
            .ok_or_else(|| CliError::InvalidInput("Invalid default linear strategy".into()))?,
        InterestRateStrategy::piecewise(Decimal::ZERO, Decimal::ZERO, Decimal::ZERO, Decimal::ZERO)
            .ok_or_else(|| CliError::InvalidInput("Invalid default piecewise strategy".into()))?,
        InterestRateStrategy::exponential2(Decimal::ZERO, Decimal::ZERO, Decimal::from(2u32))
            .ok_or_else(|| CliError::InvalidInput("Invalid default exponential strategy".into()))?,
    ])
}

/// Returns the default index for the strategy selector (piecewise).
fn default_strategy_index(strategies: &[InterestRateStrategy]) -> usize {
    strategies
        .iter()
        .position(|strategy| matches!(strategy, InterestRateStrategy::Piecewise(_)))
        .unwrap_or(0)
}

/// Prompts for a linear interest rate strategy.
fn prompt_linear_strategy(
    theme: &ColorfulTheme,
    calculator: &InterestRateCalculator,
) -> CliResult<InterestRateStrategy> {
    loop {
        let base_rate = prompt_decimal(
            theme,
            "Base rate at 0% utilization (e.g., 0.05 for 5% APY)",
            "0.05",
            "linear base rate",
        )?;
        let top_rate = prompt_decimal(
            theme,
            "Rate at 100% utilization (e.g., 0.15 for 15% APY)",
            "0.10",
            "linear top rate",
        )?;

        match calculator.calculate_linear(base_rate, top_rate) {
            Ok(strategy) => break Ok(strategy),
            Err(e) => {
                logger::warn(e);
                println!("Please re-enter the base rate and top rate.\n");
            }
        }
    }
}

/// Prompts for a piecewise interest rate strategy.
fn prompt_piecewise_strategy(
    theme: &ColorfulTheme,
    calculator: &InterestRateCalculator,
) -> CliResult<InterestRateStrategy> {
    loop {
        let starting_rate = prompt_decimal(
            theme,
            "Starting rate at 0% utilization (e.g., 0.02)",
            "0.02",
            "piecewise starting rate",
        )?;
        let optimal_usage = prompt_decimal(
            theme,
            "Optimal utilization ratio (e.g., 0.80 for 80%)",
            "0.80",
            "piecewise optimal utilization",
        )?;
        let optimal_rate = prompt_decimal(
            theme,
            "Rate at optimal utilization (e.g., 0.10)",
            "0.10",
            "piecewise optimal rate",
        )?;
        let max_rate = prompt_decimal(
            theme,
            "Maximum rate at 100% utilization (e.g., 0.50)",
            "0.50",
            "piecewise max rate",
        )?;

        match calculator.calculate_piecewise(starting_rate, optimal_rate, optimal_usage, max_rate) {
            Ok(strategy) => break Ok(strategy),
            Err(e) => {
                logger::warn(e);
                println!("Please re-enter the interest rate parameters.\n");
            }
        }
    }
}

/// Prompts for an exponential interest rate strategy.
fn prompt_exponential_strategy(
    theme: &ColorfulTheme,
    calculator: &InterestRateCalculator,
) -> CliResult<InterestRateStrategy> {
    loop {
        let base_rate = prompt_decimal(
            theme,
            "Base rate at 0% utilization (e.g., 0.02)",
            "0.02",
            "exponential base rate",
        )?;
        let top_rate = prompt_decimal(
            theme,
            "Top rate at 100% utilization (e.g., 0.50)",
            "0.50",
            "exponential top rate",
        )?;
        let eccentricity = prompt_decimal(
            theme,
            "Curve eccentricity (e.g., 2-12)",
            "2",
            "exponential eccentricity",
        )?;

        match calculator.calculate_exponential2(base_rate, top_rate, eccentricity) {
            Ok(strategy) => break Ok(strategy),
            Err(e) => {
                logger::warn(e);
                println!("Please re-enter the exponential parameters.\n");
            }
        }
    }
}

/// Prompts for interest rate strategy during interactive mode.
pub fn prompt_interest_rate_strategy(
    theme: &ColorfulTheme,
    mut builder: ConfigBuilder,
) -> CliResult<ConfigBuilder> {
    logger::heading("\n📈 Interest Rate Strategy\n");

    let strategy_types = default_interest_rate_strategies()?;
    let strategy_labels: Vec<String> = strategy_types
        .iter()
        .map(strategy_label)
        .map(str::to_string)
        .collect();
    let strategy_type = Select::with_theme(theme)
        .with_prompt("Select interest rate model")
        .items(&strategy_labels)
        .default(default_strategy_index(&strategy_types))
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;

    let calculator = InterestRateCalculator::new();

    let strategy = match strategy_types
        .get(strategy_type)
        .ok_or_else(|| CliError::InvalidInput("Invalid interest rate selection".into()))?
    {
        InterestRateStrategy::Linear(_) => prompt_linear_strategy(theme, &calculator)?,
        InterestRateStrategy::Piecewise(_) => prompt_piecewise_strategy(theme, &calculator)?,
        InterestRateStrategy::Exponential2(_) => prompt_exponential_strategy(theme, &calculator)?,
    };

    builder = builder.borrow_interest_rate_strategy(strategy);

    Ok(builder)
}

/// Edits interest rate strategy on an existing market configuration.
pub fn edit_interest_rate_strategy(
    theme: &ColorfulTheme,
    config: &mut MarketConfiguration,
) -> CliResult<()> {
    logger::heading("\n📈 Interest Rate Strategy");
    let defaults = StrategyDefaults::from_strategy(&config.borrow_interest_rate_strategy)?;

    let strategy_types = StrategyKind::ALL.to_vec();
    let strategy_choice = Select::with_theme(theme)
        .with_prompt("Select interest rate model")
        .items(&strategy_types)
        .default(defaults.kind.as_index())
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;

    config.borrow_interest_rate_strategy = match strategy_choice {
        0 => {
            let base = prompt_decimal(
                theme,
                "Base rate at 0% utilization",
                &defaults.get("base", "0.0"),
                "linear base rate",
            )?;
            let top = prompt_decimal(
                theme,
                "Top rate at 100% utilization",
                &defaults.get("top", "0.0"),
                "linear top rate",
            )?;
            InterestRateStrategy::linear(base, top).ok_or_else(|| {
                CliError::InvalidInput("Invalid linear interest rate parameters".into())
            })?
        }
        1 => {
            let base = prompt_decimal(
                theme,
                "Starting rate at 0% utilization",
                &defaults.get("base", "0.0"),
                "piecewise starting rate",
            )?;
            let optimal = prompt_decimal(
                theme,
                "Optimal utilization ratio (0-1)",
                &defaults.get("optimal", "0.8"),
                "piecewise optimal utilization",
            )?;
            let rate_1 = prompt_decimal(
                theme,
                "Rate at optimal utilization",
                &defaults.get("rate_1", "0.0"),
                "piecewise optimal rate",
            )?;
            let rate_2 = prompt_decimal(
                theme,
                "Maximum rate at 100% utilization",
                &defaults.get("rate_2", "0.0"),
                "piecewise max rate",
            )?;
            InterestRateStrategy::piecewise(base, optimal, rate_1, rate_2).ok_or_else(|| {
                CliError::InvalidInput("Invalid piecewise interest rate parameters".into())
            })?
        }
        2 => {
            let base = prompt_decimal(
                theme,
                "Base rate at 0% utilization",
                &defaults.get("base", "0.0"),
                "exponential base rate",
            )?;
            let top = prompt_decimal(
                theme,
                "Top rate at 100% utilization",
                &defaults.get("top", "0.0"),
                "exponential top rate",
            )?;
            let eccentricity = prompt_decimal(
                theme,
                "Curve eccentricity (e.g., 2-12)",
                &defaults.get("eccentricity", "2.0"),
                "exponential eccentricity",
            )?;
            InterestRateStrategy::exponential2(base, top, eccentricity).ok_or_else(|| {
                CliError::InvalidInput("Invalid exponential interest rate parameters".into())
            })?
        }
        _ => config.borrow_interest_rate_strategy.clone(),
    };

    Ok(())
}
