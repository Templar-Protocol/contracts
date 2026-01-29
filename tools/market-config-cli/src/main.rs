mod commands;
mod prompts;

use clap::{Args, Parser, Subcommand};
use commands::{
    handle_calculate_curve, handle_from_contract, handle_from_template, handle_interactive,
    handle_validate,
};
use dialoguer::theme::ColorfulTheme;
use market_config_cli::curve::{CurveInput, ModelArg};
use market_config_cli::{logger, CliResult};
use near_sdk::AccountId;
use prompts::resolve_curve_params;
use std::path::PathBuf;
use templar_common::{number::Decimal, utils::Network};

const LONG_ABOUT: &str = "\
Market Configuration CLI\n\n\
Interactive prompting is split into clear sections:\n\
- Basic config: time chunk duration, borrow/collateral asset IDs, protocol account.\n\
- Oracle config: oracle account ID, Pyth price IDs, on-chain decimals, max price age.\n\
- Risk parameters: maintenance/liquidation MCRs, usage ratios, liquidation spread.\n\
- Interest rates: choose model (linear/piecewise/exponential) and fill its parameters.\n\
- Ranges: supply/borrow/withdrawal minimums/maximums.\n\
- Fees: origination and time-based withdrawal fee configuration.\n\
- Yield weights: split of protocol yield across recipients.";
#[derive(Parser)]
#[command(
    version,
    about = "Market Configuration CLI",
    long_about = LONG_ABOUT
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new market configuration interactively
    #[command(alias = "i")]
    Interactive(InteractiveArgs),
    /// Generate configuration from an existing deployed contract
    #[command(alias = "fc")]
    FromContract(FromContractArgs),
    /// Generate configuration from a template file
    #[command(alias = "ft")]
    FromTemplate(FromTemplateArgs),
    /// Validate an existing configuration file
    #[command(alias = "v")]
    Validate(ValidateArgs),
    /// Calculate interest rate curve parameters
    #[command(alias = "calc")]
    CalculateCurve(Box<CalculateCurveArgs>),
}

#[derive(Args)]
struct InteractiveArgs {
    /// Output file path for the generated configuration
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// NEAR network (mainnet, testnet)
    #[arg(short, long)]
    network: Option<Network>,
}

#[derive(Args)]
struct FromContractArgs {
    /// Contract account ID to copy configuration from
    #[arg(short, long)]
    contract_id: Option<AccountId>,

    /// Output file path for the generated configuration
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// NEAR network (mainnet, testnet)
    #[arg(short, long)]
    network: Option<Network>,
}

#[derive(Args)]
struct FromTemplateArgs {
    /// Template file path
    #[arg(short, long)]
    template: Option<PathBuf>,

    /// Output file path for the generated configuration
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Args)]
struct ValidateArgs {
    /// Configuration file path to validate
    #[arg(short, long)]
    config_path: Option<PathBuf>,

    /// NEAR network (mainnet, testnet)
    #[arg(short, long)]
    network: Option<Network>,
}

#[derive(Args)]
struct CalculateCurveArgs {
    /// Starting interest rate (APY as decimal, e.g., 0.05 for 5%)
    #[arg(short, long)]
    starting_rate: Option<Decimal>,

    /// Optimal interest rate at optimal usage
    #[arg(short = 'r', long)]
    optimal_rate: Option<Decimal>,

    /// Optimal usage ratio (0.0-1.0)
    #[arg(short = 'u', long)]
    optimal_usage: Option<Decimal>,

    /// Maximum interest rate at 100% usage
    #[arg(short, long)]
    max_rate: Option<Decimal>,

    /// Number of points to render in the ASCII curve/table output
    #[arg(short, long, default_value_t = 10)]
    display_points: usize,

    /// Interest rate model to calculate (piecewise, linear, or exponential)
    #[arg(long)]
    model: Option<ModelArg>,

    /// Exponential curve eccentricity (required for model=exponential)
    #[arg(long)]
    eccentricity: Option<Decimal>,
}

#[tokio::main]
async fn main() -> CliResult {
    let result = run_cli().await;
    match result {
        Ok(()) => Ok(()),
        Err(market_config_cli::CliError::Interrupted) => {
            std::process::exit(130);
        }
        Err(market_config_cli::CliError::Silent(_)) => {
            std::process::exit(1);
        }
        Err(err) => {
            logger::warn(err);
            std::process::exit(1);
        }
    }
}

async fn run_cli() -> CliResult {
    let cli = Cli::parse();
    let theme = ColorfulTheme::default();

    ctrlc::set_handler(move || {
        let term = console::Term::stdout();
        let _ = term.show_cursor();
    })
    .map_err(|e| {
        market_config_cli::CliError::Other(format!("Error setting Ctrl-C handler: {e}"))
    })?;

    match cli.command {
        Commands::Interactive(InteractiveArgs { output, network }) => {
            handle_interactive(output, network, &theme).await?;
        }

        Commands::FromContract(FromContractArgs {
            contract_id,
            output,
            network,
        }) => {
            handle_from_contract(contract_id, output, network, &theme).await?;
        }

        Commands::FromTemplate(FromTemplateArgs { template, output }) => {
            handle_from_template(template, output, &theme).await?;
        }

        Commands::Validate(ValidateArgs {
            config_path,
            network,
        }) => {
            handle_validate(network, config_path, &theme).await?;
        }

        Commands::CalculateCurve(calc_args) => {
            let CalculateCurveArgs {
                starting_rate,
                optimal_rate,
                optimal_usage,
                max_rate,
                display_points,
                model,
                eccentricity,
            } = *calc_args;
            let input = CurveInput {
                starting_rate,
                optimal_rate,
                optimal_usage,
                max_rate,
                display_points,
                model,
                eccentricity,
            };
            let (params, model, eccentricity) = resolve_curve_params(&input, &theme)?;

            handle_calculate_curve(&params, &model, eccentricity)?;
        }
    }
    Ok(())
}
