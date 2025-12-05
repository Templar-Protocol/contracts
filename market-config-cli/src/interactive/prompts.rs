use crate::{
    config::{validator::set_progress_style, ConfigTemplate},
    editor::utils::prompt_decimal,
    logger,
    oracle::PriceValidator,
    CliError, CliResult, ConfigBuilder, ConfigValidator, InterestRateCalculator,
};
use console::Term;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use indicatif::ProgressBar;
use near_sdk::AccountId;
use std::str::FromStr;
use templar_common::{
    fee::{Fee, TimeBasedFee},
    interest_rate_strategy::InterestRateStrategy,
    market::{MarketConfiguration, YieldWeights},
    number::Decimal,
    oracle::pyth::PriceIdentifier,
    utils::Network,
};

const INTERACTIVE_STEPS: u64 = 7;

pub struct InteractivePrompt {
    network: Network,
    theme: ColorfulTheme,
}

fn prompt_until_valid<T, R, P, V>(mut prompt_fn: P, mut validate_fn: V) -> CliResult<R>
where
    P: FnMut() -> Result<T, dialoguer::Error>,
    V: FnMut(T) -> CliResult<R>,
{
    loop {
        match prompt_fn() {
            Ok(value) => match validate_fn(value) {
                Ok(result) => break Ok(result),
                Err(err) => {
                    logger::warn(err);
                    println!("Please try again.\n");
                }
            },
            Err(err) => {
                logger::warn(format!("Failed to read input: {err}"));
                println!("Please try again.\n");
            }
        }
    }
}

impl InteractivePrompt {
    pub fn new(network: Network) -> Self {
        Self {
            network,
            theme: ColorfulTheme::default(),
        }
    }

    async fn prompt_account_with_validation<F>(
        &self,
        builder: ConfigBuilder,
        prompt: &str,
        default: Option<String>,
        label: &str,
        apply: F,
    ) -> CliResult<(ConfigBuilder, AccountId)>
    where
        F: Fn(ConfigBuilder, &AccountId) -> CliResult<ConfigBuilder>,
    {
        let mut account_id: AccountId = prompt_until_valid(
            || {
                let mut input = Input::with_theme(&self.theme).with_prompt(prompt);
                if let Some(ref default_value) = default {
                    input = input.default(default_value.clone());
                }
                input.interact_text()
            },
            |value: String| {
                value.parse::<AccountId>().map_err(|e| {
                    CliError::Validation(format!("Invalid {label} account ID '{value}': {e}"))
                })
            },
        )?;

        let validator = ConfigValidator::new(Some(self.network));

        loop {
            match validator.validate_account_id(&account_id).await {
                Ok(()) => {
                    logger::success(format!("{label} validated"));
                    let builder = apply(builder, &account_id)?;
                    break Ok((builder, account_id));
                }
                Err(e) => {
                    logger::warn(format!("Could not validate {label}: {e}"));
                    let retry = Confirm::with_theme(&self.theme)
                        .with_prompt(format!("Re-enter {label}?"))
                        .default(true)
                        .interact()
                        .map_err(|err| CliError::Io(std::io::Error::other(err)))?;
                    if retry {
                        account_id = prompt_until_valid(
                            || {
                                let mut input = Input::with_theme(&self.theme).with_prompt(prompt);
                                input = input.default(account_id.to_string());
                                input.interact_text()
                            },
                            |value: String| {
                                value.parse::<AccountId>().map_err(|err| {
                                    CliError::Validation(format!(
                                        "Invalid {label} account ID '{value}': {err}"
                                    ))
                                })
                            },
                        )?;
                        continue;
                    }
                    let continue_anyway = Confirm::with_theme(&self.theme)
                        .with_prompt(format!(
                            "Continue anyway with this {label} even though validation failed?"
                        ))
                        .default(false)
                        .interact()
                        .map_err(|err| CliError::Io(std::io::Error::other(err)))?;
                    if continue_anyway {
                        let builder = apply(builder, &account_id)?;
                        break Ok((builder, account_id));
                    }
                }
            }
        }
    }

    /// # Errors
    pub async fn run(&self) -> CliResult<MarketConfiguration> {
        println!("\n🔧 Templar Market Configuration Generator\n");
        println!("This tool will guide you through creating a market configuration.\n");

        let progress = ProgressBar::new(INTERACTIVE_STEPS);

        set_progress_style(&progress, "⏳ {msg}...");

        // Ask if they want to use a template
        let use_template = Confirm::with_theme(&self.theme)
            .with_prompt("Would you like to start with a template?")
            .default(true)
            .interact()
            .map_err(|e| CliError::Io(std::io::Error::other(e)))?;

        let mut builder = ConfigBuilder::new();

        if use_template {
            let template = self.select_template()?;
            println!("\nUsing template: {}", template.name);
            println!("{}\n", template.description);
            // Apply template defaults (we'll override them in prompts)
        }

        let mut step_idx = 0;

        // Basic configuration
        print_step_overview(&progress, &builder, step_idx, "Basic configuration");
        builder = self.prompt_basic_config(builder).await?;
        progress.inc(1);
        step_idx += 1;

        // Oracle configuration
        print_step_overview(&progress, &builder, step_idx, "Oracle configuration");
        builder = self.prompt_oracle_config(builder).await?;
        progress.inc(1);
        step_idx += 1;

        // Risk parameters
        print_step_overview(&progress, &builder, step_idx, "Risk parameters");
        builder = self.prompt_risk_parameters(builder)?;
        progress.inc(1);
        step_idx += 1;

        // Interest rate strategy
        print_step_overview(&progress, &builder, step_idx, "Interest rate strategy");
        builder = self.prompt_interest_rate_strategy(builder)?;
        progress.inc(1);
        step_idx += 1;

        // Ranges
        print_step_overview(&progress, &builder, step_idx, "Position ranges");
        builder = self.prompt_ranges(builder)?;
        progress.inc(1);
        step_idx += 1;

        // Fees
        print_step_overview(&progress, &builder, step_idx, "Fees");
        builder = self.prompt_fees(builder)?;
        progress.inc(1);
        step_idx += 1;

        // Yield distribution
        print_step_overview(&progress, &builder, step_idx, "Yield distribution");
        builder = self.prompt_yield_weights(builder)?;
        progress.inc(1);

        progress.set_message("Building configuration");
        let config = builder.build()?;
        progress.finish_with_message("✓ Setup complete");

        println!("\n✓ Configuration complete! Building...");

        Ok(config)
    }

    fn select_template(&self) -> CliResult<ConfigTemplate> {
        let templates = ConfigTemplate::list_all();
        let template_names: Vec<String> = templates.iter().map(|t| t.name.clone()).collect();

        let selection = Select::with_theme(&self.theme)
            .with_prompt("Select a template")
            .items(&template_names)
            .default(0)
            .interact()
            .map_err(|e| CliError::Io(std::io::Error::other(e)))?;

        Ok(templates[selection].clone())
    }

    async fn prompt_basic_config(&self, mut builder: ConfigBuilder) -> CliResult<ConfigBuilder> {
        println!("\n📋 Basic Configuration\n");

        let time_chunk_ms: u64 = prompt_until_valid(
            || {
                Input::with_theme(&self.theme)
                    .with_prompt("Time chunk duration (milliseconds)")
                    .default(600_000)
                    .interact_text()
            },
            Ok::<_, CliError>,
        )?;
        builder = builder.time_chunk_duration_ms(time_chunk_ms);

        let (builder_with_borrow, _) = self
            .prompt_account_with_validation(
                builder,
                &format!(
                    "Borrow asset contract ID (e.g., {})",
                    match self.network {
                        Network::Mainnet => "usdc.near",
                        Network::Testnet => "usdc.testnet",
                    }
                ),
                None,
                "borrow asset",
                |b, account| ConfigBuilder::borrow_asset(b, account.as_str()),
            )
            .await?;

        let (builder_with_collateral, _) = self
            .prompt_account_with_validation(
                builder_with_borrow,
                &format!(
                    "Collateral asset contract ID (e.g., {})",
                    match self.network {
                        Network::Mainnet => "wrap.near",
                        Network::Testnet => "wrap.testnet",
                    }
                ),
                None,
                "collateral asset",
                |b, account| ConfigBuilder::collateral_asset(b, account.as_str()),
            )
            .await?;

        let (builder, _) = self
            .prompt_account_with_validation(
                builder_with_collateral,
                "Protocol account ID (for fees)",
                None,
                "protocol account",
                |b, account| ConfigBuilder::protocol_account_id(b, account.as_str()),
            )
            .await?;

        Ok(builder)
    }

    #[allow(clippy::too_many_lines)]
    async fn prompt_oracle_config(&self, builder: ConfigBuilder) -> CliResult<ConfigBuilder> {
        println!("\n🔮 Oracle Configuration\n");

        let default_oracle = match self.network {
            Network::Mainnet => "pyth-oracle.near".to_string(),
            Network::Testnet => "pyth-oracle.testnet".to_string(),
        };
        let (mut builder, oracle_id) = self
            .prompt_account_with_validation(
                builder,
                "Oracle contract ID",
                Some(default_oracle),
                "oracle account",
                |b, account| ConfigBuilder::oracle_account_id(b, account.as_str()),
            )
            .await?;

        let validator = PriceValidator::new(self.network);
        let oracle_account_id = oracle_id.clone();

        // Borrow asset price feed
        let borrow_price_id = loop {
            let borrow_price_id: PriceIdentifier = prompt_until_valid(
                || {
                    Input::with_theme(&self.theme)
                        .with_prompt("Borrow asset Pyth price ID (64 hex chars)")
                        .interact_text()
                },
                |value: String| parse_price_id(&value),
            )?;
            match validator
                .validate_price_feed(oracle_account_id.clone(), &borrow_price_id)
                .await
            {
                Ok(()) => {
                    logger::success("Borrow asset price feed validated");
                    break borrow_price_id;
                }
                Err(e) => {
                    logger::warn(format!("Could not validate borrow price feed: {e}"));
                    let retry = Confirm::with_theme(&self.theme)
                        .with_prompt("Re-enter this price ID?")
                        .default(true)
                        .interact()
                        .map_err(|err| CliError::Io(std::io::Error::other(err)))?;
                    if retry {
                        continue;
                    }
                    let continue_anyway = Confirm::with_theme(&self.theme)
                        .with_prompt("Continue anyway with this ID?")
                        .default(false)
                        .interact()
                        .map_err(|err| CliError::Io(std::io::Error::other(err)))?;
                    if continue_anyway {
                        break borrow_price_id;
                    }
                }
            }
        };
        builder = builder.borrow_price_id(borrow_price_id.0);

        let borrow_decimals: i32 = prompt_until_valid(
            || {
                Input::with_theme(&self.theme)
                    .with_prompt("Borrow asset decimals")
                    .default(6)
                    .interact_text()
            },
            Ok::<_, CliError>,
        )?;
        builder = builder.borrow_decimals(borrow_decimals);

        // Collateral asset price feed
        let collateral_price_id = loop {
            let collateral_price_id: PriceIdentifier = prompt_until_valid(
                || {
                    Input::with_theme(&self.theme)
                        .with_prompt("Collateral asset Pyth price ID (64 hex chars)")
                        .interact_text()
                },
                |value: String| parse_price_id(&value),
            )?;

            match validator
                .validate_price_feed(oracle_account_id.clone(), &collateral_price_id)
                .await
            {
                Ok(()) => {
                    logger::success("Collateral asset price feed validated");
                    break collateral_price_id;
                }
                Err(e) => {
                    logger::warn(format!("Could not validate collateral price feed: {e}"));
                    let retry = Confirm::with_theme(&self.theme)
                        .with_prompt("Re-enter this price ID?")
                        .default(true)
                        .interact()
                        .map_err(|err| CliError::Io(std::io::Error::other(err)))?;
                    if retry {
                        continue;
                    }
                    let continue_anyway = Confirm::with_theme(&self.theme)
                        .with_prompt("Continue anyway with this ID?")
                        .default(false)
                        .interact()
                        .map_err(|err| CliError::Io(std::io::Error::other(err)))?;
                    if continue_anyway {
                        break collateral_price_id;
                    }
                }
            }
        };
        builder = builder.collateral_price_id(collateral_price_id.0);

        let collateral_decimals: i32 = prompt_until_valid(
            || {
                Input::with_theme(&self.theme)
                    .with_prompt("Collateral asset decimals")
                    .default(24)
                    .interact_text()
            },
            Ok::<_, CliError>,
        )?;
        builder = builder.collateral_decimals(collateral_decimals);

        let price_max_age: u32 = prompt_until_valid(
            || {
                Input::with_theme(&self.theme)
                    .with_prompt("Maximum price age (seconds)")
                    .default(60)
                    .interact_text()
            },
            Ok::<_, CliError>,
        )?;
        builder = builder.price_max_age_s(price_max_age);

        logger::success("Price feeds set");
        Ok(builder)
    }

    fn prompt_risk_parameters(&self, mut builder: ConfigBuilder) -> CliResult<ConfigBuilder> {
        println!("\n⚖️  Risk Parameters\n");

        let mcr_maintenance = prompt_until_valid(
            || {
                Input::with_theme(&self.theme)
                    .with_prompt("Maintenance collateralization ratio (e.g., 1.25 for 125%)")
                    .default("1.25".to_string())
                    .interact_text()
            },
            |value: String| {
                Decimal::from_str(&value)
                    .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))
            },
        )?;
        builder = builder.borrow_mcr_maintenance(mcr_maintenance);

        let mcr_liquidation = prompt_until_valid(
            || {
                Input::with_theme(&self.theme)
                    .with_prompt("Liquidation collateralization ratio (e.g., 1.20 for 120%)")
                    .default("1.20".to_string())
                    .interact_text()
            },
            |value: String| {
                Decimal::from_str(&value)
                    .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))
            },
        )?;
        builder = builder.borrow_mcr_liquidation(mcr_liquidation);

        let max_usage = prompt_until_valid(
            || {
                Input::with_theme(&self.theme)
                    .with_prompt("Maximum usage ratio (e.g., 0.90 for 90%)")
                    .default("0.90".to_string())
                    .interact_text()
            },
            |value: String| {
                Decimal::from_str(&value)
                    .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))
            },
        )?;
        builder = builder.borrow_max_usage_ratio(max_usage);

        let liquidation_spread = prompt_until_valid(
            || {
                Input::with_theme(&self.theme)
                    .with_prompt("Maximum liquidator spread (e.g., 0.05 for 5%)")
                    .default("0.05".to_string())
                    .interact_text()
            },
            |value: String| {
                Decimal::from_str(&value)
                    .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))
            },
        )?;
        builder = builder.liquidation_max_spread(liquidation_spread);

        let has_max_duration = Confirm::with_theme(&self.theme)
            .with_prompt("Set maximum borrow duration?")
            .default(true)
            .interact()
            .map_err(|e| CliError::Io(std::io::Error::other(e)))?;

        if has_max_duration {
            let max_duration_ms: u64 = prompt_until_valid(
                || {
                    Input::with_theme(&self.theme)
                        .with_prompt("Maximum borrow duration (milliseconds)")
                        .interact_text()
                },
                Ok::<_, CliError>,
            )?;
            builder = builder.borrow_max_duration_ms(Some(max_duration_ms));
        } else {
            builder = builder.borrow_max_duration_ms(None);
        }

        Ok(builder)
    }

    fn prompt_interest_rate_strategy(
        &self,
        mut builder: ConfigBuilder,
    ) -> CliResult<ConfigBuilder> {
        println!("\n📈 Interest Rate Strategy\n");

        let strategy_types = default_interest_rate_strategies()?;
        let strategy_labels: Vec<String> = strategy_types
            .iter()
            .map(strategy_label)
            .map(str::to_string)
            .collect();
        let strategy_type = Select::with_theme(&self.theme)
            .with_prompt("Select interest rate model")
            .items(&strategy_labels)
            .default(default_strategy_index(&strategy_types))
            .interact()
            .map_err(|e| CliError::Io(std::io::Error::other(e)))?;

        let calculator = InterestRateCalculator::new();

        let strategy = match strategy_types
            .get(strategy_type)
            .ok_or_else(|| CliError::InvalidInput("Invalid interest rate selection".into()))?
        {
            InterestRateStrategy::Linear(_) => self.prompt_linear_strategy(&calculator)?,
            InterestRateStrategy::Piecewise(_) => self.prompt_piecewise_strategy(&calculator)?,
            InterestRateStrategy::Exponential2(_) => {
                self.prompt_exponential_strategy(&calculator)?
            }
        };

        builder = builder.borrow_interest_rate_strategy(strategy);

        Ok(builder)
    }

    fn prompt_linear_strategy(
        &self,
        calculator: &InterestRateCalculator,
    ) -> CliResult<InterestRateStrategy> {
        loop {
            let base_rate = prompt_decimal(
                &self.theme,
                "Base rate at 0% utilization (e.g., 0.05 for 5% APY)",
                "0.05",
                "linear base rate",
            )?;
            let slope = prompt_decimal(
                &self.theme,
                "Slope (rate increase per utilization, e.g., 0.10)",
                "0.10",
                "linear slope",
            )?;

            match calculator.calculate_linear(base_rate, slope) {
                Ok(strategy) => break Ok(strategy),
                Err(e) => {
                    logger::warn(e);
                    println!("Please re-enter the base rate and slope.\n");
                }
            }
        }
    }

    fn prompt_piecewise_strategy(
        &self,
        calculator: &InterestRateCalculator,
    ) -> CliResult<InterestRateStrategy> {
        loop {
            let starting_rate = prompt_decimal(
                &self.theme,
                "Starting rate at 0% utilization (e.g., 0.02)",
                "0.02",
                "piecewise starting rate",
            )?;
            let optimal_usage = prompt_decimal(
                &self.theme,
                "Optimal utilization ratio (e.g., 0.80 for 80%)",
                "0.80",
                "piecewise optimal utilization",
            )?;
            let optimal_rate = prompt_decimal(
                &self.theme,
                "Rate at optimal utilization (e.g., 0.10)",
                "0.10",
                "piecewise optimal rate",
            )?;
            let max_rate = prompt_decimal(
                &self.theme,
                "Maximum rate at 100% utilization (e.g., 0.50)",
                "0.50",
                "piecewise max rate",
            )?;

            match calculator.calculate_piecewise(
                starting_rate,
                optimal_rate,
                optimal_usage,
                max_rate,
            ) {
                Ok(strategy) => break Ok(strategy),
                Err(e) => {
                    logger::warn(e);
                    println!("Please re-enter the interest rate parameters.\n");
                }
            }
        }
    }

    fn prompt_exponential_strategy(
        &self,
        calculator: &InterestRateCalculator,
    ) -> CliResult<InterestRateStrategy> {
        loop {
            let base_rate = prompt_decimal(
                &self.theme,
                "Base rate at 0% utilization (e.g., 0.02)",
                "0.02",
                "exponential base rate",
            )?;
            let top_rate = prompt_decimal(
                &self.theme,
                "Top rate at 100% utilization (e.g., 0.50)",
                "0.50",
                "exponential top rate",
            )?;
            let eccentricity = prompt_decimal(
                &self.theme,
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

    #[allow(clippy::too_many_lines)]
    fn prompt_ranges(&self, mut builder: ConfigBuilder) -> CliResult<ConfigBuilder> {
        println!("\n📏 Position Ranges\n");

        let (supply_min, supply_max) = loop {
            let borrow_min: u128 = prompt_until_valid(
                || {
                    Input::with_theme(&self.theme)
                        .with_prompt("Minimum borrow amount")
                        .default(1_000_000)
                        .interact_text()
                },
                Ok::<_, CliError>,
            )?;
            let has_borrow_max = Confirm::with_theme(&self.theme)
                .with_prompt("Set maximum borrow amount?")
                .default(true)
                .interact()
                .map_err(|e| CliError::Io(std::io::Error::other(e)))?;
            let borrow_max = if has_borrow_max {
                Some(prompt_until_valid(
                    || {
                        Input::with_theme(&self.theme)
                            .with_prompt("Maximum borrow amount")
                            .interact_text()
                    },
                    Ok::<_, CliError>,
                )?)
            } else {
                None
            };
            match builder.clone().borrow_range(borrow_min, borrow_max) {
                Ok(updated) => {
                    builder = updated;
                    logger::success("Borrow range set");
                }
                Err(e) => {
                    logger::warn(e);
                    println!("Please re-enter the borrow range values.\n");
                    continue;
                }
            }

            let supply_values = loop {
                let supply_min: u128 = prompt_until_valid(
                    || {
                        Input::with_theme(&self.theme)
                            .with_prompt("Minimum supply amount")
                            .default(1_000_000)
                            .interact_text()
                    },
                    Ok::<_, CliError>,
                )?;
                let has_supply_max = Confirm::with_theme(&self.theme)
                    .with_prompt("Set maximum supply amount?")
                    .default(true)
                    .interact()
                    .map_err(|e| CliError::Io(std::io::Error::other(e)))?;
                let supply_max = if has_supply_max {
                    Some(prompt_until_valid(
                        || {
                            Input::with_theme(&self.theme)
                                .with_prompt("Maximum supply amount")
                                .interact_text()
                        },
                        Ok::<_, CliError>,
                    )?)
                } else {
                    None
                };
                match builder.clone().supply_range(supply_min, supply_max) {
                    Ok(updated_builder) => {
                        builder = updated_builder;
                        break (supply_min, supply_max);
                    }
                    Err(e) => {
                        logger::warn(e);
                        println!("Please re-enter the supply range values.\n");
                    }
                }
            };

            break supply_values;
        };
        loop {
            let withdrawal_min: u128 = prompt_until_valid(
                || {
                    Input::with_theme(&self.theme)
                        .with_prompt("Minimum withdrawal amount")
                        .default(supply_min)
                        .interact_text()
                },
                Ok::<_, CliError>,
            )?;
            match builder
                .clone()
                .supply_withdrawal_range(withdrawal_min, supply_max)
            {
                Ok(updated) => {
                    builder = updated;
                    break;
                }
                Err(e) => {
                    logger::warn(e);
                    println!("Please re-enter the withdrawal range.\n");
                }
            }
        }
        Ok(builder)
    }

    fn prompt_fees(&self, mut builder: ConfigBuilder) -> CliResult<ConfigBuilder> {
        println!("\n💰 Fees\n");

        // Origination fee
        let has_origination_fee = Confirm::with_theme(&self.theme)
            .with_prompt("Set borrow origination fee?")
            .default(true)
            .interact()
            .map_err(|e| CliError::Io(std::io::Error::other(e)))?;

        if has_origination_fee {
            let fee_types = vec!["Flat amount", "Percentage"];
            let fee_type = Select::with_theme(&self.theme)
                .with_prompt("Fee type")
                .items(&fee_types)
                .default(1)
                .interact()
                .map_err(|e| CliError::Io(std::io::Error::other(e)))?;

            if fee_type == 0 {
                let amount: u128 = prompt_until_valid(
                    || {
                        Input::with_theme(&self.theme)
                            .with_prompt("Flat fee amount")
                            .interact_text()
                    },
                    Ok::<_, CliError>,
                )?;
                builder = builder.borrow_origination_fee(Fee::Flat(amount.into()));
            } else {
                let pct = prompt_until_valid(
                    || {
                        Input::with_theme(&self.theme)
                            .with_prompt("Fee percentage (e.g., 0.001 for 0.1%)")
                            .interact_text()
                    },
                    |value: String| {
                        Decimal::from_str(&value)
                            .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))
                    },
                )?;
                builder = builder.borrow_origination_fee(Fee::Proportional(pct));
            }
        } else {
            builder = builder.borrow_origination_fee(Fee::zero());
        }

        // Withdrawal fee (simplified - just using zero for now)
        builder = builder.supply_withdrawal_fee(TimeBasedFee::zero());

        Ok(builder)
    }

    fn prompt_yield_weights(&self, mut builder: ConfigBuilder) -> CliResult<ConfigBuilder> {
        println!("\n🎯 Yield Distribution\n");

        let supply_weight: u16 = prompt_until_valid(
            || {
                Input::with_theme(&self.theme)
                    .with_prompt("Supplier yield weight (e.g., 9 for 90% to suppliers)")
                    .default(9)
                    .interact_text()
            },
            Ok::<_, CliError>,
        )?;

        let mut weights = YieldWeights::new_with_supply_weight(supply_weight);

        let add_static = Confirm::with_theme(&self.theme)
            .with_prompt("Add static yield recipients (e.g., protocol revenue)?")
            .default(true)
            .interact()
            .map_err(|e| CliError::Io(std::io::Error::other(e)))?;

        if add_static {
            let account_id = prompt_until_valid(
                || {
                    Input::with_theme(&self.theme)
                        .with_prompt("Static recipient account ID")
                        .interact_text()
                },
                |value: String| {
                    value
                        .parse()
                        .map_err(|e| CliError::InvalidInput(format!("Invalid account ID: {e}")))
                },
            )?;

            let weight: u16 = prompt_until_valid(
                || {
                    Input::with_theme(&self.theme)
                        .with_prompt("Static recipient weight (e.g., 1 for 10%)")
                        .default(1)
                        .interact_text()
                },
                Ok::<_, CliError>,
            )?;

            weights = weights.with_static(account_id, weight);
        }

        builder = builder.yield_weights(weights);

        Ok(builder)
    }
}

fn strategy_label(strategy: &InterestRateStrategy) -> &'static str {
    match strategy {
        InterestRateStrategy::Linear(_) => "Linear",
        InterestRateStrategy::Piecewise(_) => "Piecewise",
        InterestRateStrategy::Exponential2(_) => "Exponential2",
    }
}

fn default_strategy_index(strategies: &[InterestRateStrategy]) -> usize {
    strategies
        .iter()
        .position(|strategy| matches!(strategy, InterestRateStrategy::Piecewise(_)))
        .unwrap_or(0)
}

/// Parse a price ID from a hex string
/// # Errors
pub fn parse_price_id(hex_string: &str) -> CliResult<PriceIdentifier> {
    let hex_string = hex_string.trim_start_matches("0x");

    if hex_string.len() != 64 {
        return Err(CliError::InvalidInput(
            "Price ID must be 64 hex characters (32 bytes)".into(),
        ));
    }

    let bytes = hex::decode(hex_string)
        .map_err(|e| CliError::InvalidInput(format!("Invalid hex string: {e}")))?;

    let mut array = [0u8; 32];
    array.copy_from_slice(&bytes);

    Ok(PriceIdentifier(array))
}

fn print_step_overview(
    progress: &ProgressBar,
    builder: &ConfigBuilder,
    step_index: u64,
    step_label: &str,
) {
    let term = Term::stdout();
    let _ = term.clear_screen();

    let total = progress.length().unwrap_or(INTERACTIVE_STEPS);
    let position = step_index + 1;

    let _ = term.write_line("Current config");
    for line in builder.overview_lines() {
        let _ = term.write_line(&format!("  • {line}"));
    }
    let _ = term.write_line("");

    progress.set_position(step_index);
    progress.set_message(step_label.to_string());
    progress.tick();

    let _ = term.write_line(&format!("[{position}/{total}] {step_label}"));
    let _ = term.write_line("");
}

fn default_interest_rate_strategies() -> CliResult<Vec<InterestRateStrategy>> {
    Ok(vec![
        InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO)
            .ok_or_else(|| CliError::InvalidInput("Invalid default linear strategy".into()))?,
        InterestRateStrategy::piecewise(Decimal::ZERO, Decimal::ZERO, Decimal::ZERO, Decimal::ZERO)
            .ok_or_else(|| CliError::InvalidInput("Invalid default piecewise strategy".into()))?,
        InterestRateStrategy::exponential2(Decimal::ZERO, Decimal::ZERO, Decimal::from(2u32))
            .ok_or_else(|| CliError::InvalidInput("Invalid default exponential strategy".into()))?,
    ])
}
