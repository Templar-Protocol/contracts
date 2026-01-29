use crate::CliResult;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use market_config_cli::{
    calculator::parameters::CurveParameters,
    common::prompt::utils::prompt_decimal,
    common::shared::map_dialoguer_err,
    curve::{strategy_from_name, CurveInput, ModelArg},
    CliError,
};
use near_sdk::AccountId;
use std::{path::PathBuf, str::FromStr};
use templar_common::{
    interest_rate_strategy::InterestRateStrategy, number::Decimal, utils::Network,
};

/// # Errors
pub fn prompt_network(network: Option<Network>, theme: &ColorfulTheme) -> CliResult<Network> {
    if let Some(network) = network {
        return Ok(network);
    }

    let networks = [Network::Testnet, Network::Mainnet];
    let labels: Vec<String> = networks.iter().map(ToString::to_string).collect();
    let index = Select::with_theme(theme)
        .with_prompt("Select NEAR network")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;
    Ok(networks.get(index).copied().unwrap_or(Network::Testnet))
}

/// # Errors
pub fn prompt_contract_id(
    contract_id: Option<AccountId>,
    theme: &ColorfulTheme,
) -> CliResult<AccountId> {
    if let Some(value) = contract_id {
        Ok(value)
    } else {
        let value: String = Input::with_theme(theme)
            .with_prompt("Enter contract account ID")
            .interact_text()
            .map_err(|err| map_dialoguer_err(&err))?;
        value
            .parse::<AccountId>()
            .map_err(|e| CliError::InvalidInput(e.to_string()))
    }
}

/// # Errors
pub fn prompt_path(
    value: Option<PathBuf>,
    theme: &ColorfulTheme,
    prompt: &str,
) -> CliResult<PathBuf> {
    if let Some(path) = value {
        Ok(path)
    } else {
        let path: String = Input::with_theme(theme)
            .with_prompt(prompt)
            .interact_text()
            .map_err(|err| map_dialoguer_err(&err))?;
        Ok(PathBuf::from(path))
    }
}

/// # Errors
pub fn prompt_curve_params(theme: &ColorfulTheme) -> CliResult<CurveParameters> {
    let starting_rate_input: String = Input::with_theme(theme)
        .with_prompt("Starting rate at 0% utilization (e.g., 0.02)")
        .default("0.02".to_string())
        .interact_text()
        .map_err(|err| map_dialoguer_err(&err))?;
    let starting_rate: Decimal = Decimal::from_str(&starting_rate_input)
        .map_err(|e| CliError::InvalidInput(format!("Invalid starting rate: {e}")))?;

    let optimal_rate_input: String = Input::with_theme(theme)
        .with_prompt("Rate at optimal utilization (e.g., 0.10)")
        .default("0.10".to_string())
        .interact_text()
        .map_err(|err| map_dialoguer_err(&err))?;
    let optimal_rate: Decimal = Decimal::from_str(&optimal_rate_input)
        .map_err(|e| CliError::InvalidInput(format!("Invalid optimal rate: {e}")))?;

    let optimal_usage_input: String = Input::with_theme(theme)
        .with_prompt("Optimal utilization ratio (e.g., 0.80 for 80%)")
        .default("0.80".to_string())
        .interact_text()
        .map_err(|err| map_dialoguer_err(&err))?;
    let optimal_usage: Decimal = Decimal::from_str(&optimal_usage_input)
        .map_err(|e| CliError::InvalidInput(format!("Invalid optimal utilization: {e}")))?;

    let max_rate_input: String = Input::with_theme(theme)
        .with_prompt("Maximum rate at 100% utilization (e.g., 0.50)")
        .default("0.50".to_string())
        .interact_text()
        .map_err(|err| map_dialoguer_err(&err))?;
    let max_rate: Decimal = Decimal::from_str(&max_rate_input)
        .map_err(|e| CliError::InvalidInput(format!("Invalid max rate: {e}")))?;

    Ok(CurveParameters {
        starting_rate,
        optimal_rate,
        optimal_usage,
        max_rate,
        display_points: 10,
    })
}

/// # Errors
pub fn resolve_curve_params(
    input: &CurveInput,
    theme: &ColorfulTheme,
) -> CliResult<(CurveParameters, InterestRateStrategy, Option<Decimal>)> {
    let has_rate_values = input.starting_rate.is_some()
        || input.optimal_rate.is_some()
        || input.optimal_usage.is_some()
        || input.max_rate.is_some()
        || input.eccentricity.is_some();

    if !has_rate_values {
        let model = input.model.unwrap_or(prompt_model_arg(theme)?);
        return prompt_model_params(theme, model, input.display_points);
    }

    let model = select_model(input.model)?;
    build_curve_params(model, input)
}

fn select_model(model: Option<ModelArg>) -> CliResult<InterestRateStrategy> {
    let model_name = model.ok_or_else(|| {
        CliError::InvalidInput(
            "When supplying curve parameters via flags, --model is required (piecewise|linear|exponential)"
                .into(),
        )
    })?;
    strategy_from_name(model_name.as_str())
}

fn build_curve_params(
    model: InterestRateStrategy,
    input: &CurveInput,
) -> CliResult<(CurveParameters, InterestRateStrategy, Option<Decimal>)> {
    match model {
        InterestRateStrategy::Linear(_) => build_linear_params(input, model),
        InterestRateStrategy::Exponential2(_) => build_exponential_params(input, model),
        InterestRateStrategy::Piecewise(_) => build_piecewise_params(input, model),
    }
}

fn build_linear_params(
    input: &CurveInput,
    model: InterestRateStrategy,
) -> CliResult<(CurveParameters, InterestRateStrategy, Option<Decimal>)> {
    let starting_rate = input.starting_rate.ok_or_else(|| {
        CliError::InvalidInput(
            "When providing curve flags for linear model, --starting-rate is required".into(),
        )
    })?;
    let top_rate = input.optimal_rate.ok_or_else(|| {
        CliError::InvalidInput(
            "When providing curve flags for linear model, --optimal-rate is required".into(),
        )
    })?;

    let params = CurveParameters {
        starting_rate,
        optimal_rate: top_rate,
        optimal_usage: input.optimal_usage.unwrap_or(Decimal::ZERO),
        max_rate: input.max_rate.unwrap_or(Decimal::ZERO),
        display_points: input.display_points,
    };
    Ok((params, model, input.eccentricity))
}

fn build_exponential_params(
    input: &CurveInput,
    model: InterestRateStrategy,
) -> CliResult<(CurveParameters, InterestRateStrategy, Option<Decimal>)> {
    let starting_rate = input.starting_rate.ok_or_else(|| {
        CliError::InvalidInput(
            "When providing curve flags for exponential model, --starting-rate is required".into(),
        )
    })?;
    let top_rate = input.optimal_rate.ok_or_else(|| {
        CliError::InvalidInput(
            "When providing curve flags for exponential model, --optimal-rate is required".into(),
        )
    })?;
    let eccentricity = input.eccentricity.ok_or_else(|| {
        CliError::InvalidInput(
            "When providing curve flags for exponential model, --eccentricity is required".into(),
        )
    })?;

    let params = CurveParameters {
        starting_rate,
        optimal_rate: top_rate,
        optimal_usage: input.optimal_usage.unwrap_or(Decimal::ZERO),
        max_rate: input.max_rate.unwrap_or(Decimal::ZERO),
        display_points: input.display_points,
    };
    Ok((params, model, Some(eccentricity)))
}

fn build_piecewise_params(
    input: &CurveInput,
    model: InterestRateStrategy,
) -> CliResult<(CurveParameters, InterestRateStrategy, Option<Decimal>)> {
    let starting_rate = input.starting_rate.ok_or_else(|| {
        CliError::InvalidInput(
            "When providing curve flags for piecewise model, --starting-rate is required".into(),
        )
    })?;
    let optimal_rate = input.optimal_rate.ok_or_else(|| {
        CliError::InvalidInput(
            "When providing curve flags for piecewise model, --optimal-rate is required".into(),
        )
    })?;
    let optimal_usage = input.optimal_usage.ok_or_else(|| {
        CliError::InvalidInput(
            "When providing curve flags for piecewise model, --optimal-usage is required".into(),
        )
    })?;
    let max_rate = input.max_rate.ok_or_else(|| {
        CliError::InvalidInput(
            "When providing curve flags for piecewise model, --max-rate is required".into(),
        )
    })?;

    let params = CurveParameters {
        starting_rate,
        optimal_rate,
        optimal_usage,
        max_rate,
        display_points: input.display_points,
    };
    Ok((params, model, input.eccentricity))
}

fn prompt_model_arg(theme: &ColorfulTheme) -> CliResult<ModelArg> {
    let options = ["piecewise", "linear", "exponential"];
    let idx = Select::with_theme(theme)
        .with_prompt("Select interest rate model")
        .items(options)
        .default(0)
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;
    Ok(match options[idx] {
        "linear" => ModelArg::Linear,
        "exponential" => ModelArg::Exponential,
        _ => ModelArg::Piecewise,
    })
}

fn prompt_model_params(
    theme: &ColorfulTheme,
    model: ModelArg,
    display_points_default: usize,
) -> CliResult<(CurveParameters, InterestRateStrategy, Option<Decimal>)> {
    match model {
        ModelArg::Linear => {
            let start = prompt_decimal(
                theme,
                "Starting rate at 0% utilization (e.g., 0.02)",
                "0.02",
                "linear starting rate",
            )?;
            let top_rate = prompt_decimal(
                theme,
                "Rate at 100% utilization (e.g., 0.15)",
                "0.10",
                "linear top rate",
            )?;
            let display_points = prompt_display_points(theme, display_points_default)?;
            let params = CurveParameters {
                starting_rate: start,
                optimal_rate: top_rate,
                optimal_usage: Decimal::ZERO,
                max_rate: Decimal::ZERO,
                display_points,
            };
            let model = strategy_from_name("linear")?;
            Ok((params, model, None))
        }
        ModelArg::Exponential => {
            let start = prompt_decimal(
                theme,
                "Starting rate at 0% utilization (e.g., 0.02)",
                "0.02",
                "exponential starting rate",
            )?;
            let top = prompt_decimal(
                theme,
                "Top rate at 100% utilization (e.g., 0.50)",
                "0.50",
                "exponential top rate",
            )?;
            let ecc = prompt_decimal(
                theme,
                "Curve eccentricity (e.g., 2-12)",
                "2.0",
                "exponential eccentricity",
            )?;
            let display_points = prompt_display_points(theme, display_points_default)?;
            let params = CurveParameters {
                starting_rate: start,
                optimal_rate: top,
                optimal_usage: Decimal::ZERO,
                max_rate: Decimal::ZERO,
                display_points,
            };
            let model = strategy_from_name("exponential")?;
            Ok((params, model, Some(ecc)))
        }
        ModelArg::Piecewise => {
            let mut params = prompt_curve_params(theme)?;
            params.display_points = prompt_display_points(theme, display_points_default)?;
            let model = strategy_from_name("piecewise")?;
            Ok((params, model, None))
        }
    }
}

fn prompt_display_points(theme: &ColorfulTheme, default: usize) -> CliResult<usize> {
    let input: String = Input::with_theme(theme)
        .with_prompt("Number of display points for the curve")
        .default(default.to_string())
        .interact_text()
        .map_err(|err| map_dialoguer_err(&err))?;

    input
        .parse::<usize>()
        .map_err(|e| CliError::InvalidInput(format!("Invalid display points: {e}")))
}
