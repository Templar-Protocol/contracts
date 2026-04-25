use crate::cli_prompts::{prompt_contract_id, prompt_network, prompt_path};
use dialoguer::{theme::ColorfulTheme, Confirm};
use market_config_cli::calculator::parameters::CurveParameters;
use market_config_cli::logger;
use market_config_cli::ui::prompt::error::map_dialoguer_err;
use market_config_cli::{
    CliResult, ConfigEditor, ConfigValidator, ContractReader, InteractivePrompt,
    InterestRateCalculator, MarketConfiguration,
};
use near_sdk::AccountId;
use serde_json::{self, to_string_pretty};
use std::path::PathBuf;
use templar_common::{interest_rate_strategy::InterestRateStrategy, utils::Network, Decimal};

/// Handle the interactive command flow.
/// # Errors
pub async fn handle_interactive(
    output: Option<PathBuf>,
    network: Option<Network>,
    theme: &ColorfulTheme,
) -> CliResult {
    let network = prompt_network(network, theme)?;
    let prompt = InteractivePrompt::new(theme, network);
    let config = prompt.run_interactive().await?;

    let market_output_path = prompt_path(
        output,
        theme,
        "Output file path for the generated configuration",
    )?;

    let validator = ConfigValidator::new(Some(network));
    let mut validation_ok = true;
    if let Err(err) = validator.validate(&config).await {
        validation_ok = false;
        logger::warn(format!("Validation failed: {err}"));
        let continue_anyway = Confirm::with_theme(theme)
            .with_prompt("Save configuration anyway?")
            .default(false)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;
        if !continue_anyway {
            return Err(err);
        }
    }
    std::fs::write(&market_output_path, to_string_pretty(&config)?)?;
    if validation_ok {
        logger::success(format!(
            "Configuration written to: {}",
            market_output_path.display()
        ));
    } else {
        logger::alert(format!(
            "Configuration written without passing validation: {}",
            market_output_path.display()
        ));
    }
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

    let reader = ContractReader::new(network)?;
    let config = reader.read_config(contract_id.clone()).await?;
    logger::success(format!("Configuration fetched from {contract_id}"));

    let mut config = config;
    let wants_edit = Confirm::with_theme(theme)
        .with_prompt("Edit configuration before saving?")
        .default(true)
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;

    if wants_edit {
        let editor = ConfigEditor::new(theme, network);
        config = editor.edit_config(config).await?;
    }

    let validator = ConfigValidator::new(Some(network));
    let mut validation_ok = true;
    if let Err(err) = validator.validate(&config).await {
        validation_ok = false;
        logger::warn(format!("Validation failed: {err}"));
        let continue_anyway = Confirm::with_theme(theme)
            .with_prompt("Save configuration anyway?")
            .default(false)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;
        if !continue_anyway {
            return Err(err);
        }
    }

    std::fs::write(&market_path, serde_json::to_string_pretty(&config)?)?;
    if validation_ok {
        logger::success(format!(
            "Configuration written to: {}",
            market_path.display()
        ));
    } else {
        logger::alert(format!(
            "Configuration written without passing validation: {}",
            market_path.display()
        ));
    }

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

    let template_content = match std::fs::read_to_string(&template_path) {
        Ok(content) => content,
        Err(err) => {
            logger::warn(format!(
                "Template not found or unreadable: {} ({err})",
                template_path.display()
            ));
            return Err(market_config_cli::CliError::Silent(format!(
                "Unable to read template: {}",
                template_path.display()
            )));
        }
    };
    let mut config: MarketConfiguration = serde_json::from_str(&template_content).map_err(|e| {
        market_config_cli::CliError::Validation(format!(
            "Failed to parse configuration template: {e}"
        ))
    })?;

    logger::info(format!("Template loaded: {}", template_path.display()));

    let wants_edit = Confirm::with_theme(theme)
        .with_prompt("Edit configuration before saving?")
        .default(true)
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;

    if wants_edit {
        let editor = ConfigEditor::new(theme, network);
        config = editor.edit_config(config).await?;
    }

    let validator = ConfigValidator::new(Some(network));
    let mut validation_ok = true;
    if let Err(err) = validator.validate(&config).await {
        validation_ok = false;
        logger::warn(format!("Validation failed: {err}"));
        let continue_anyway = Confirm::with_theme(theme)
            .with_prompt("Save configuration anyway?")
            .default(false)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;
        if !continue_anyway {
            return Err(err);
        }
    }

    let output_path = prompt_path(
        output,
        theme,
        "Output file path for the generated configuration",
    )?;

    std::fs::write(&output_path, serde_json::to_string_pretty(&config)?)?;
    if validation_ok {
        logger::success(format!(
            "Configuration written to: {}",
            output_path.display()
        ));
    } else {
        logger::alert(format!(
            "Configuration written without passing validation: {}",
            output_path.display()
        ));
    }

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

    logger::info(format!(
        "Validating configuration: {}",
        config_path.display()
    ));

    let market_json = std::fs::read_to_string(&config_path)?;
    let market_config: serde_json::Value = serde_json::from_str(&market_json).map_err(|e| {
        market_config_cli::CliError::Validation(format!("Failed to parse configuration: {e}"))
    })?;

    let validator = ConfigValidator::new(Some(network));
    validator.validate_json(&market_config).await?;

    logger::success("Configuration is valid!");
    Ok(())
}

/// Handle the calculate-curve command flow.
/// # Errors
pub fn handle_calculate_curve(
    params: &CurveParameters,
    model: &InterestRateStrategy,
    eccentricity: Option<Decimal>,
) -> CliResult {
    let calculator = InterestRateCalculator::new();
    let strategy = match model {
        InterestRateStrategy::Linear(_) => {
            calculator.calculate_linear(params.starting_rate, params.optimal_rate)?
        }
        InterestRateStrategy::Exponential2(_) => {
            let ecc = eccentricity.ok_or_else(|| {
                market_config_cli::CliError::InvalidInput(
                    "Exponential model requires eccentricity".into(),
                )
            })?;
            calculator.calculate_exponential2(params.starting_rate, params.optimal_rate, ecc)?
        }
        InterestRateStrategy::Piecewise(_) => calculator.calculate_piecewise(
            params.starting_rate,
            params.optimal_rate,
            params.optimal_usage,
            params.max_rate,
        )?,
    };

    println!("Interest Rate Strategy:");
    println!("{}", serde_json::to_string_pretty(&strategy)?);
    calculator.display_curve(&strategy, params.display_points);
    Ok(())
}
