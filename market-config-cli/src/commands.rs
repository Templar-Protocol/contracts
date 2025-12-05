use crate::prompts::{prompt_contract_id, prompt_network, prompt_path};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm};
use market_config_cli::editor::utils::prompt_decimal;
use market_config_cli::logger;
use market_config_cli::{
    CliResult, ConfigEditor, ConfigValidator, ContractReader, InteractivePrompt,
    InterestRateCalculator, MarketConfiguration,
};
use near_sdk::AccountId;
use serde_json::{self, to_string_pretty};
use std::path::PathBuf;
use templar_common::{
    interest_rate_strategy::InterestRateStrategy, number::Decimal, utils::Network,
};

/// Handle the interactive command flow.
/// # Errors
pub async fn handle_interactive(
    output: Option<PathBuf>,
    network: Option<Network>,
    theme: &ColorfulTheme,
) -> CliResult {
    let network = prompt_network(network, theme)?;
    let prompt = InteractivePrompt::new(network);
    let config = prompt.run().await?;

    let market_output_path = prompt_path(
        output,
        theme,
        "Output file path for the generated configuration",
    )?;

    let validator = ConfigValidator::new(Some(network));
    validator.validate(&config).await?;
    std::fs::write(&market_output_path, to_string_pretty(&config)?)?;
    logger::success(format!(
        "Configuration written to: {}",
        market_output_path.display()
    ));
    Ok(())
}

/// Handle the from-contract command flow.
/// # Errors
pub async fn handle_from_contract(
    contract_id: Option<AccountId>,
    output: Option<PathBuf>,
    network: Option<Network>,
    theme: &ColorfulTheme,
) -> CliResult {
    let network = prompt_network(network, theme)?;
    let contract_id = prompt_contract_id(contract_id, theme)?;
    let market_path = prompt_path(
        output,
        theme,
        "Output file path for the generated configuration",
    )?;

    let reader = ContractReader::new(&network.to_string());
    let config = reader.read_config(contract_id.clone()).await?;
    println!("✓ Configuration fetched from {contract_id}");

    let mut config = config;
    let wants_edit = Confirm::with_theme(theme)
        .with_prompt("Edit configuration before saving?")
        .default(true)
        .interact()
        .map_err(std::io::Error::other)?;

    if wants_edit {
        let editor = ConfigEditor::new(theme);
        config = editor.edit(config)?;
    }

    let validator = ConfigValidator::new(Some(network));
    validator.validate(&config).await?;

    std::fs::write(&market_path, serde_json::to_string_pretty(&config)?)?;
    logger::success(format!(
        "Configuration written to: {}",
        market_path.display()
    ));

    Ok(())
}

/// Handle the from-template command flow.
/// # Errors
pub async fn handle_from_template(
    template: Option<PathBuf>,
    output: Option<PathBuf>,
    theme: &ColorfulTheme,
) -> CliResult {
    let network = prompt_network(None, theme)?;
    let template_path = prompt_path(template, theme, "Path to wanted configuration")?;

    let template_content = std::fs::read_to_string(&template_path)?;
    let mut config: MarketConfiguration = serde_json::from_str(&template_content)?;

    println!("Template loaded: {}", template_path.display());

    let wants_edit = Confirm::with_theme(theme)
        .with_prompt("Edit configuration before saving?")
        .default(true)
        .interact()
        .map_err(std::io::Error::other)?;

    if wants_edit {
        let editor = ConfigEditor::new(theme);
        config = editor.edit(config)?;
    }

    let validator = ConfigValidator::new(Some(network));
    validator.validate(&config).await?;

    let output_path = prompt_path(
        output,
        theme,
        "Output file path for the generated configuration",
    )?;

    std::fs::write(&output_path, serde_json::to_string_pretty(&config)?)?;
    logger::success(format!(
        "Configuration written to: {}",
        output_path.display()
    ));

    Ok(())
}

/// Handle the validate command flow.
/// # Errors
pub async fn handle_validate(
    network: Option<Network>,
    config_path: Option<PathBuf>,
    theme: &ColorfulTheme,
) -> CliResult {
    let network = prompt_network(network, theme)?;
    let config_path = prompt_path(
        config_path,
        theme,
        "Path to configuration you want to validate",
    )?;

    println!("Validating configuration: {}", config_path.display());

    let market_json = std::fs::read_to_string(&config_path)?;
    let market_config: serde_json::Value = serde_json::from_str(&market_json)?;

    let validator = ConfigValidator::new(Some(network));
    validator.validate_json(&market_config).await?;

    println!("{}", style("✓ Configuration is valid!").green());
    Ok(())
}

/// Handle the calculate-curve command flow.
/// # Errors
#[allow(clippy::too_many_arguments)]
pub fn handle_calculate_curve(
    starting_rate: Option<Decimal>,
    optimal_rate: Option<Decimal>,
    optimal_usage: Option<Decimal>,
    max_rate: Option<Decimal>,
    display_points: usize,
    model: Option<&InterestRateStrategy>,
    eccentricity: Option<Decimal>,
    theme: &ColorfulTheme,
) -> CliResult {
    let calculator = InterestRateCalculator::new();
    let strategy = match model {
        Some(InterestRateStrategy::Linear(_)) => {
            let (start, slope) = prompt_or_default_linear(starting_rate, optimal_rate, theme)?;
            calculator.calculate_linear(start, slope)?
        }
        Some(InterestRateStrategy::Exponential2(_)) => {
            let (start, top, ecc) =
                prompt_or_default_exponential(starting_rate, optimal_rate, eccentricity, theme)?;
            calculator.calculate_exponential2(start, top, ecc)?
        }
        _ => {
            let params = prompt_or_default_piecewise(
                starting_rate,
                optimal_rate,
                optimal_usage,
                max_rate,
                theme,
            )?;
            calculator.calculate_piecewise(params.0, params.1, params.2, params.3)?
        }
    };

    println!("Interest Rate Strategy:");
    println!("{}", serde_json::to_string_pretty(&strategy)?);
    calculator.display_curve(&strategy, display_points);
    Ok(())
}

fn prompt_or_default_linear(
    starting_rate: Option<Decimal>,
    slope: Option<Decimal>,
    theme: &ColorfulTheme,
) -> CliResult<(Decimal, Decimal)> {
    let start = match starting_rate {
        Some(v) => v,
        None => prompt_decimal(
            theme,
            "Starting rate at 0% utilization (e.g., 0.02)",
            "0.02",
            "linear starting rate",
        )?,
    };
    let slope = match slope {
        Some(v) => v,
        None => prompt_decimal(
            theme,
            "Slope (rate increase per utilization, e.g., 0.10)",
            "0.10",
            "linear slope",
        )?,
    };
    Ok((start, slope))
}

fn prompt_or_default_exponential(
    starting_rate: Option<Decimal>,
    top_rate: Option<Decimal>,
    eccentricity: Option<Decimal>,
    theme: &ColorfulTheme,
) -> CliResult<(Decimal, Decimal, Decimal)> {
    let start = match starting_rate {
        Some(v) => v,
        None => prompt_decimal(
            theme,
            "Starting rate at 0% utilization (e.g., 0.02)",
            "0.02",
            "exponential starting rate",
        )?,
    };
    let top = match top_rate {
        Some(v) => v,
        None => prompt_decimal(
            theme,
            "Top rate at 100% utilization (e.g., 0.50)",
            "0.50",
            "exponential top rate",
        )?,
    };
    let ecc = match eccentricity {
        Some(v) => v,
        None => prompt_decimal(
            theme,
            "Curve eccentricity (e.g., 2-12)",
            "2.0",
            "exponential eccentricity",
        )?,
    };
    Ok((start, top, ecc))
}

fn prompt_or_default_piecewise(
    starting_rate: Option<Decimal>,
    optimal_rate: Option<Decimal>,
    optimal_usage: Option<Decimal>,
    max_rate: Option<Decimal>,
    theme: &ColorfulTheme,
) -> CliResult<(Decimal, Decimal, Decimal, Decimal)> {
    let start = match starting_rate {
        Some(v) => v,
        None => prompt_decimal(
            theme,
            "Starting rate at 0% utilization (e.g., 0.02)",
            "0.02",
            "piecewise starting rate",
        )?,
    };
    let opt_rate = match optimal_rate {
        Some(v) => v,
        None => prompt_decimal(
            theme,
            "Rate at optimal utilization (e.g., 0.10)",
            "0.10",
            "piecewise optimal rate",
        )?,
    };
    let opt_usage = match optimal_usage {
        Some(v) => v,
        None => prompt_decimal(
            theme,
            "Optimal utilization ratio (e.g., 0.80 for 80%)",
            "0.80",
            "piecewise optimal usage",
        )?,
    };
    let max = match max_rate {
        Some(v) => v,
        None => prompt_decimal(
            theme,
            "Maximum rate at 100% utilization (e.g., 0.50)",
            "0.50",
            "piecewise max rate",
        )?,
    };
    Ok((start, opt_rate, opt_usage, max))
}
