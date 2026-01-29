use crate::{
    common::{
        prompt::{
            prompt_account_with_validation, prompt_price_id_with_validation,
            ranges::RangeDefaults,
            utils::{
                fee_defaults, parse_asset_input, prompt_decimal, prompt_decimals, EditSection,
                StrategyDefaults, StrategyKind,
            },
            PromptContext,
        },
        shared::{handle_interrupted, map_dialoguer_err},
    },
    config::{
        validator::{
            check_asset_existence, resolve_nep141_or_nep245_parts, set_progress_style,
            TokenMetadata,
        },
        ConfigTemplate,
    },
    logger,
    oracle::PriceValidator,
    rpc::{ft_metadata, list_ema_prices_no_older_than, multitoken_metadata, view_account},
    CliError, CliResult, ConfigBuilder, InterestRateCalculator,
};
use console::Term;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, MultiSelect, Select};
use indicatif::ProgressBar;
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::{
    json_types::{U128, U64},
    AccountId,
};
use std::{cell::RefCell, str::FromStr};
use templar_common::{
    asset::{AssetClass, BorrowAsset, CollateralAsset, FungibleAsset},
    fee::{Fee, TimeBasedFee, TimeBasedFeeFunction},
    interest_rate_strategy::InterestRateStrategy,
    market::{MarketConfiguration, YieldWeights},
    number::Decimal,
    oracle::pyth::{Price, PriceIdentifier},
    time_chunk::TimeChunkConfiguration,
    utils::Network,
};

const INTERACTIVE_STEPS: u64 = 7;

pub struct MarketPrompter<'a> {
    theme: &'a ColorfulTheme,
    network: Network,
    borrow_price_context: RefCell<Option<PriceHintContext>>,
    collateral_price_context: RefCell<Option<PriceHintContext>>,
}

#[derive(Clone, Copy)]
enum AssetStandard {
    Nep141,
    Nep245,
}

#[derive(Clone)]
struct PriceHintContext {
    price: Price,
    asset_decimals: i32,
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
                handle_interrupted(&err);
                logger::warn(format!("Failed to read input: {err}"));
                println!("Please try again.\n");
            }
        }
    }
}

async fn fetch_asset_metadata<T: AssetClass>(
    client: &JsonRpcClient,
    asset: &FungibleAsset<T>,
) -> CliResult<TokenMetadata> {
    if let Some((contract_id, token_id)) = asset.clone().into_nep245() {
        let (resolved_contract, resolved_token_id) =
            resolve_nep141_or_nep245_parts(contract_id, &token_id)?;
        let token_id = resolved_token_id.ok_or_else(|| {
            CliError::Validation(format!("Missing NEP-245 token id for asset '{asset}'"))
        })?;
        let metadata = multitoken_metadata(client, resolved_contract, token_id).await?;
        return Ok(TokenMetadata::Nep245(metadata));
    }

    let id: AccountId = asset.contract_id().as_ref().parse().map_err(|e| {
        CliError::Other(format!(
            "Unable to parse account_id '{}': {e}",
            asset.contract_id().as_ref()
        ))
    })?;

    let metadata = ft_metadata(client, id).await?;
    Ok(TokenMetadata::Nep141(metadata))
}

impl<'a> MarketPrompter<'a> {
    pub fn new(theme: &'a ColorfulTheme, network: Network) -> Self {
        Self {
            theme,
            network,
            borrow_price_context: RefCell::new(None),
            collateral_price_context: RefCell::new(None),
        }
    }

    /// Interactive flow: walks through all prompts from scratch.
    /// # Errors
    pub async fn run_interactive(&self) -> CliResult<MarketConfiguration> {
        logger::heading("\n🔧 Templar Market Configuration Generator\n");
        logger::heading("This tool will guide you through creating a market configuration.\n");

        let progress = ProgressBar::new(INTERACTIVE_STEPS);

        set_progress_style(&progress, "⏳ {msg}...");

        let use_template = Confirm::with_theme(self.theme)
            .with_prompt("Would you like to start with a template?")
            .default(true)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        let mut builder = ConfigBuilder::new();

        if use_template {
            let template = self.select_template()?;
            logger::heading(format!("\nUsing template: {}", template.name));
            logger::heading(format!("{}\n", template.description));
            builder = template.apply_to_builder(builder)?;
        }

        let mut step_idx = 0;

        print_step_overview(&progress, &builder, step_idx, "Basic configuration");
        builder = self.prompt_basic_config(builder, self.network).await?;
        progress.inc(1);
        step_idx += 1;

        print_step_overview(&progress, &builder, step_idx, "Oracle configuration");
        builder = self.prompt_oracle_config(builder, self.network).await?;
        progress.inc(1);
        step_idx += 1;

        print_step_overview(&progress, &builder, step_idx, "Risk parameters");
        builder = self.prompt_risk_parameters(builder)?;
        progress.inc(1);
        step_idx += 1;

        print_step_overview(&progress, &builder, step_idx, "Interest rate strategy");
        builder = self.prompt_interest_rate_strategy(builder)?;
        progress.inc(1);
        step_idx += 1;

        print_step_overview(&progress, &builder, step_idx, "Position ranges");

        if let Some((
            oracle_account_id,
            borrow_price_id,
            collateral_price_id,
            borrow_decimals,
            collateral_decimals,
            price_max_age,
        )) = builder.price_oracle_inputs()
        {
            self.refresh_price_contexts(
                oracle_account_id,
                borrow_price_id,
                collateral_price_id,
                borrow_decimals,
                collateral_decimals,
                price_max_age,
            )
            .await;
        }
        builder = self.prompt_ranges(builder)?;
        progress.inc(1);
        step_idx += 1;

        print_step_overview(&progress, &builder, step_idx, "Fees");
        builder = self.prompt_fees(builder)?;
        progress.inc(1);
        step_idx += 1;

        print_step_overview(&progress, &builder, step_idx, "Yield distribution");
        builder = self.prompt_yield_weights(builder).await?;
        progress.inc(1);

        progress.set_message("Building configuration");
        let config = builder.build()?;
        progress.finish_with_message("✓ Setup complete");

        println!("\n✓ Configuration complete! Building...");

        Ok(config)
    }

    fn edit_fungible_asset<T: AssetClass>(
        &self,
        label: &str,
        current: &FungibleAsset<T>,
    ) -> CliResult<FungibleAsset<T>> {
        let prompt_ctx = PromptContext::new(self.theme);
        let (default_standard, default_contract, default_token) = asset_defaults(current);

        let asset_standard = Select::with_theme(self.theme)
            .with_prompt(format!("{label} type"))
            .items(["NEP-141 (fungible token)", "NEP-245 (multi-token)"])
            .default(match default_standard {
                AssetStandard::Nep141 => 0,
                AssetStandard::Nep245 => 1,
            })
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        match asset_standard {
            0 => {
                let contract_id = prompt_ctx.prompt_account_id(
                    &format!("{label} contract ID"),
                    Some(default_contract),
                    label,
                )?;

                Ok(FungibleAsset::nep141(contract_id))
            }
            1 => {
                let contract_id = prompt_ctx.prompt_account_id(
                    &format!("{label} contract ID (NEP-245 multi-token)"),
                    Some(default_contract.clone()),
                    label,
                )?;

                let contract_id_str = contract_id.to_string();
                let asset = prompt_until_valid(
                    || {
                        let mut input = Input::with_theme(self.theme)
                            .with_prompt(format!("{label} token ID (string)"));
                        if let Some(default_token) = &default_token {
                            input = input.default(default_token.clone());
                        }
                        input.interact_text()
                    },
                    |token_id: String| {
                        let composed = format!("nep245:{contract_id_str}:{token_id}");
                        parse_asset_input(&composed, label)
                    },
                )?;

                Ok(asset)
            }
            _ => unreachable!(),
        }
    }

    /// Edit flow: pick sections to edit on an existing configuration.
    /// # Errors
    pub async fn edit_config(
        &self,
        mut config: MarketConfiguration,
    ) -> CliResult<MarketConfiguration> {
        logger::heading("\n🧩 Select sections to edit (leave empty to keep as-is):");
        let sections = EditSection::ALL;

        let selections = MultiSelect::with_theme(self.theme)
            .with_prompt("Press space to pick sections, enter to continue")
            .items(sections)
            .defaults(&vec![false; sections.len()])
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        if selections.is_empty() {
            println!("No edits selected. Keeping configuration as-is.");
            return Ok(config);
        }

        for selection in selections {
            match sections[selection] {
                EditSection::BasicConfiguration => self.edit_basic_config(&mut config)?,
                EditSection::OracleSettings => self.edit_oracle_config(&mut config)?,
                EditSection::RiskParameters => self.edit_risk_parameters(&mut config)?,
                EditSection::InterestRateStrategy => {
                    self.edit_interest_rate_strategy(&mut config)?;
                }
                EditSection::Ranges => self.edit_ranges(&mut config).await?,
                EditSection::Fees => self.edit_fees(&mut config)?,
                EditSection::YieldDistribution => self.edit_yield_weights(&mut config)?,
            }
        }

        Ok(config)
    }

    fn select_template(&self) -> CliResult<ConfigTemplate> {
        let templates = ConfigTemplate::list_all();
        let template_names: Vec<String> = templates.iter().map(|t| t.name.clone()).collect();

        let selection = Select::with_theme(self.theme)
            .with_prompt("Select a template")
            .items(&template_names)
            .default(0)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        Ok(templates[selection].clone())
    }

    async fn prompt_basic_config(
        &self,
        mut builder: ConfigBuilder,
        network: Network,
    ) -> CliResult<ConfigBuilder> {
        logger::heading("\n📋 Basic Configuration\n");

        let prompt_ctx = PromptContext::new(self.theme);

        let time_chunk_default = builder.time_chunk_duration_ms_value().unwrap_or(600_000);
        let time_chunk_ms: u64 = prompt_until_valid(
            || {
                Input::with_theme(self.theme)
                    .with_prompt("Time chunk duration (milliseconds)")
                    .default(time_chunk_default)
                    .interact_text()
            },
            Ok::<_, CliError>,
        )?;
        builder = builder.time_chunk_duration_ms(time_chunk_ms);

        builder = self
            .prompt_fungible_asset::<BorrowAsset>(
                builder,
                "Borrow asset",
                match network {
                    Network::Mainnet => "usdc.near",
                    Network::Testnet => "usdc.testnet",
                },
                ConfigBuilder::borrow_fungible_asset,
                self.network,
            )
            .await?;

        builder = self
            .prompt_fungible_asset::<CollateralAsset>(
                builder,
                "Collateral asset",
                match network {
                    Network::Mainnet => "wrap.near",
                    Network::Testnet => "wrap.testnet",
                },
                ConfigBuilder::collateral_fungible_asset,
                self.network,
            )
            .await?;

        let (builder, _) = prompt_account_with_validation(
            &prompt_ctx,
            Some(network),
            builder,
            "Protocol account ID (for fees)",
            None,
            "protocol account",
            |b, account| ConfigBuilder::protocol_account_id(b, account.as_str()),
        )
        .await?;

        Ok(builder)
    }

    #[allow(clippy::too_many_lines)]
    async fn prompt_oracle_config(
        &self,
        builder: ConfigBuilder,
        network: Network,
    ) -> CliResult<ConfigBuilder> {
        logger::heading("\n🔮 Oracle Configuration\n");

        let prompt_ctx = PromptContext::new(self.theme);

        let default_oracle = match network {
            Network::Mainnet => "pyth-oracle.near".to_string(),
            Network::Testnet => "pyth-oracle.testnet".to_string(),
        };
        let (mut builder, oracle_id) = prompt_account_with_validation(
            &prompt_ctx,
            Some(network),
            builder,
            "Oracle contract ID",
            Some(default_oracle),
            "oracle account",
            |b, account| ConfigBuilder::oracle_account_id(b, account.as_str()),
        )
        .await?;

        let validator = PriceValidator::new(network);
        let oracle_account_id = oracle_id.clone();

        let rpc_url = network.rpc_url().to_string();
        let client = JsonRpcClient::connect(&rpc_url);
        let borrow_asset = builder
            .borrow_asset_ref()
            .ok_or_else(|| {
                CliError::Validation("Borrow asset missing before oracle config".into())
            })?
            .clone();
        let collateral_asset = builder
            .collateral_asset_ref()
            .ok_or_else(|| {
                CliError::Validation("Collateral asset missing before oracle config".into())
            })?
            .clone();
        let borrow_metadata = match fetch_asset_metadata(&client, &borrow_asset).await {
            Ok(TokenMetadata::Nep141(ref m)) => Some(TokenMetadata::Nep141(m.clone())),
            Ok(TokenMetadata::Nep245(ref m)) => Some(TokenMetadata::Nep245(m.clone())),
            Err(err) => {
                logger::warn(format!(
                    "Unable to fetch borrow asset metadata: {err}. Skipping Hermes symbol check."
                ));
                None
            }
        };
        let collateral_metadata = match fetch_asset_metadata(&client, &collateral_asset).await {
            Ok(TokenMetadata::Nep141(ref m)) => Some(TokenMetadata::Nep141(m.clone())),
            Ok(TokenMetadata::Nep245(ref m)) => Some(TokenMetadata::Nep245(m.clone())),
            Err(err) => {
                logger::warn(format!(
                    "Unable to fetch collateral asset metadata: {err}. Skipping Hermes symbol check."
                ));
                None
            }
        };
        let borrow_symbol = borrow_metadata.as_ref().map(|meta| match meta {
            TokenMetadata::Nep141(m) => m.symbol.clone(),
            TokenMetadata::Nep245(m) => m.symbol.clone(),
        });
        let collateral_symbol = collateral_metadata.as_ref().map(|meta| match meta {
            TokenMetadata::Nep141(m) => m.symbol.clone(),
            TokenMetadata::Nep245(m) => m.symbol.clone(),
        });

        let borrow_price_id = prompt_price_id_with_validation(
            &prompt_ctx,
            &validator,
            oracle_account_id.clone(),
            borrow_symbol.as_deref(),
            "Borrow asset Pyth price ID (64 hex chars)",
            None,
            "Borrow price feed",
        )
        .await?;
        builder = builder.borrow_price_id(borrow_price_id.0);

        let borrow_expected_decimals = borrow_metadata.as_ref().map(|meta| match meta {
            TokenMetadata::Nep141(m) => i32::from(m.decimals),
            TokenMetadata::Nep245(m) => i32::from(m.decimals),
        });
        let borrow_decimals = loop {
            let value = prompt_decimals(
                self.theme,
                "Borrow asset decimals",
                borrow_expected_decimals.unwrap_or(6),
                "Borrow asset decimals",
            )?;
            if let Some(expected) = borrow_expected_decimals {
                if value != expected {
                    logger::warn(format!(
                        "Borrow asset decimals mismatch: on-chain {expected}, entered {value}"
                    ));
                    let retry = Confirm::with_theme(self.theme)
                        .with_prompt("Re-enter borrow asset decimals?")
                        .default(true)
                        .interact()
                        .map_err(|err| map_dialoguer_err(&err))?;
                    if retry {
                        continue;
                    }
                }
            }
            break value;
        };
        builder = builder.borrow_decimals(borrow_decimals);

        let collateral_price_id = prompt_price_id_with_validation(
            &prompt_ctx,
            &validator,
            oracle_account_id.clone(),
            collateral_symbol.as_deref(),
            "Collateral asset Pyth price ID (64 hex chars)",
            None,
            "Collateral price feed",
        )
        .await?;
        builder = builder.collateral_price_id(collateral_price_id.0);

        let collateral_expected_decimals = collateral_metadata.as_ref().map(|meta| match meta {
            TokenMetadata::Nep141(m) => i32::from(m.decimals),
            TokenMetadata::Nep245(m) => i32::from(m.decimals),
        });
        let collateral_decimals = loop {
            let value = prompt_decimals(
                self.theme,
                "Collateral asset decimals",
                collateral_expected_decimals.unwrap_or(24),
                "Collateral asset decimals",
            )?;
            if let Some(expected) = collateral_expected_decimals {
                if value != expected {
                    logger::warn(format!(
                        "Collateral asset decimals mismatch: on-chain {expected}, entered {value}"
                    ));
                    let retry = Confirm::with_theme(self.theme)
                        .with_prompt("Re-enter collateral asset decimals?")
                        .default(true)
                        .interact()
                        .map_err(|err| map_dialoguer_err(&err))?;
                    if retry {
                        continue;
                    }
                }
            }
            break value;
        };
        builder = builder.collateral_decimals(collateral_decimals);

        let price_max_age_default = builder.price_max_age_s_value().unwrap_or(60);
        let price_max_age: u32 = prompt_until_valid(
            || {
                Input::with_theme(self.theme)
                    .with_prompt("Maximum price age (seconds)")
                    .default(price_max_age_default)
                    .interact_text()
            },
            Ok::<_, CliError>,
        )?;
        builder = builder.price_max_age_s(price_max_age);

        logger::success("Price feeds set");
        Ok(builder)
    }

    async fn refresh_price_contexts(
        &self,
        oracle_account_id: AccountId,
        borrow_price_id: PriceIdentifier,
        collateral_price_id: PriceIdentifier,
        borrow_decimals: i32,
        collateral_decimals: i32,
        price_max_age: u32,
    ) {
        let client = JsonRpcClient::connect(self.network.rpc_url());
        let prices = match list_ema_prices_no_older_than(
            &client,
            oracle_account_id,
            vec![borrow_price_id, collateral_price_id],
            u64::from(price_max_age),
        )
        .await
        {
            Ok(prices) => prices,
            Err(err) => {
                logger::warn(format!("Unable to fetch current prices: {err}"));
                *self.borrow_price_context.borrow_mut() = None;
                *self.collateral_price_context.borrow_mut() = None;
                return;
            }
        };

        let borrow_price = prices
            .get(&borrow_price_id)
            .and_then(|value| value.as_ref())
            .cloned();

        if let Some(price) = borrow_price {
            *self.borrow_price_context.borrow_mut() = Some(PriceHintContext {
                price,
                asset_decimals: borrow_decimals,
            });
        } else {
            logger::warn("Borrow price feed returned no price data");
            *self.borrow_price_context.borrow_mut() = None;
        }

        let collateral_price = prices
            .get(&collateral_price_id)
            .and_then(|value| value.as_ref())
            .cloned();

        if let Some(price) = collateral_price {
            *self.collateral_price_context.borrow_mut() = Some(PriceHintContext {
                price,
                asset_decimals: collateral_decimals,
            });
        } else {
            *self.collateral_price_context.borrow_mut() = None;
        }
    }

    #[allow(clippy::too_many_lines)]
    fn prompt_risk_parameters(&self, mut builder: ConfigBuilder) -> CliResult<ConfigBuilder> {
        logger::heading("\n⚖️  Risk Parameters\n");

        let mcr_maintenance_default = builder
            .borrow_mcr_maintenance_value()
            .map_or_else(|| "1.25".to_string(), |value| value.to_string());
        let mcr_maintenance = prompt_until_valid(
            || {
                Input::with_theme(self.theme)
                    .with_prompt("Maintenance collateralization ratio (e.g., 1.25 for 125%)")
                    .default(mcr_maintenance_default.clone())
                    .interact_text()
            },
            |value: String| {
                let mcr = Decimal::from_str(&value)
                    .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
                if mcr <= Decimal::ONE {
                    return Err(CliError::InvalidInput(
                        "Maintenance MCR must be greater than 1.0".into(),
                    ));
                }
                Ok(mcr)
            },
        )?;
        builder = builder.borrow_mcr_maintenance(mcr_maintenance);

        let mcr_liquidation_default = builder
            .borrow_mcr_liquidation_value()
            .map_or_else(|| "1.20".to_string(), |value| value.to_string());
        let mcr_liquidation = prompt_until_valid(
            || {
                Input::with_theme(self.theme)
                    .with_prompt("Liquidation collateralization ratio (e.g., 1.20 for 120%)")
                    .default(mcr_liquidation_default.clone())
                    .interact_text()
            },
            |value: String| {
                let mcr = Decimal::from_str(&value)
                    .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
                if mcr <= Decimal::ONE {
                    return Err(CliError::InvalidInput(
                        "Liquidation MCR must be greater than 1.0".into(),
                    ));
                }
                if mcr > mcr_maintenance {
                    return Err(CliError::InvalidInput(
                        "Liquidation MCR must be less than or equal to maintenance MCR".into(),
                    ));
                }
                Ok(mcr)
            },
        )?;
        builder = builder.borrow_mcr_liquidation(mcr_liquidation);

        let max_usage_default = builder
            .borrow_max_usage_ratio_value()
            .map_or_else(|| "0.90".to_string(), |value| value.to_string());
        let max_usage = prompt_until_valid(
            || {
                Input::with_theme(self.theme)
                    .with_prompt("Maximum usage ratio (e.g., 0.90 for 90%)")
                    .default(max_usage_default.clone())
                    .interact_text()
            },
            |value: String| {
                let ratio = Decimal::from_str(&value)
                    .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
                if ratio.is_zero() || ratio > Decimal::ONE {
                    return Err(CliError::InvalidInput(
                        "Maximum usage ratio must be > 0 and <= 1.0".into(),
                    ));
                }
                Ok(ratio)
            },
        )?;
        builder = builder.borrow_max_usage_ratio(max_usage);

        let liquidation_spread_default = builder
            .liquidation_max_spread_value()
            .map_or_else(|| "0.05".to_string(), |value| value.to_string());
        let liquidation_spread = prompt_until_valid(
            || {
                Input::with_theme(self.theme)
                    .with_prompt("Maximum liquidator spread (e.g., 0.05 for 5%)")
                    .default(liquidation_spread_default.clone())
                    .interact_text()
            },
            |value: String| {
                let spread = Decimal::from_str(&value)
                    .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
                if spread < Decimal::ZERO || spread >= Decimal::ONE {
                    return Err(CliError::InvalidInput(
                        "Liquidation spread must be >= 0 and < 1.0".into(),
                    ));
                }
                Ok(spread)
            },
        )?;
        builder = builder.liquidation_max_spread(liquidation_spread);

        let has_max_duration = Confirm::with_theme(self.theme)
            .with_prompt("Set maximum borrow duration?")
            .default(true)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        if has_max_duration {
            let max_duration_ms: u64 = prompt_until_valid(
                || {
                    Input::with_theme(self.theme)
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
        logger::heading("\n📈 Interest Rate Strategy\n");

        let strategy_types = default_interest_rate_strategies()?;
        let strategy_labels: Vec<String> = strategy_types
            .iter()
            .map(strategy_label)
            .map(str::to_string)
            .collect();
        let strategy_type = Select::with_theme(self.theme)
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
                self.theme,
                "Base rate at 0% utilization (e.g., 0.05 for 5% APY)",
                "0.05",
                "linear base rate",
            )?;
            let top_rate = prompt_decimal(
                self.theme,
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

    fn prompt_piecewise_strategy(
        &self,
        calculator: &InterestRateCalculator,
    ) -> CliResult<InterestRateStrategy> {
        loop {
            let starting_rate = prompt_decimal(
                self.theme,
                "Starting rate at 0% utilization (e.g., 0.02)",
                "0.02",
                "piecewise starting rate",
            )?;
            let optimal_usage = prompt_decimal(
                self.theme,
                "Optimal utilization ratio (e.g., 0.80 for 80%)",
                "0.80",
                "piecewise optimal utilization",
            )?;
            let optimal_rate = prompt_decimal(
                self.theme,
                "Rate at optimal utilization (e.g., 0.10)",
                "0.10",
                "piecewise optimal rate",
            )?;
            let max_rate = prompt_decimal(
                self.theme,
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
                self.theme,
                "Base rate at 0% utilization (e.g., 0.02)",
                "0.02",
                "exponential base rate",
            )?;
            let top_rate = prompt_decimal(
                self.theme,
                "Top rate at 100% utilization (e.g., 0.50)",
                "0.50",
                "exponential top rate",
            )?;
            let eccentricity = prompt_decimal(
                self.theme,
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

    fn prompt_ranges(&self, builder: ConfigBuilder) -> CliResult<ConfigBuilder> {
        let defaults = RangeDefaults {
            borrow_min: 1_000_000,
            borrow_max: None,
            supply_min: 1_000_000,
            supply_max: None,
            withdrawal_min: 1_000_000,
            withdrawal_max: None,
        };

        let mut hint = |label: &str, amount: u128| {
            self.print_price_hint(label, amount);
        };
        let selection = crate::common::prompt::ranges::prompt_ranges_with_validation(
            self.theme,
            &defaults,
            self.price_header_line(),
            &mut hint,
            |selection| {
                let mut tmp = builder.clone();
                tmp = crate::common::prompt::ranges::apply_ranges_to_builder(tmp, selection)?;
                let _ = tmp;
                Ok(())
            },
        )?;

        crate::common::prompt::ranges::apply_ranges_to_builder(builder, &selection)
    }

    fn print_price_hint(&self, label: &str, amount: u128) {
        let ctx_ref = self.borrow_price_context.borrow();
        let Some(ctx) = ctx_ref.as_ref() else {
            return;
        };
        let Some((price_usd, total_usd)) =
            price_hint_amount(&ctx.price, ctx.asset_decimals, amount)
        else {
            return;
        };
        println!(
            "At current ${price_usd}, {label} is worth ~${total_usd}",
            price_usd = format_price(price_usd),
            total_usd = format_price(total_usd),
        );
    }

    fn price_header_line(&self) -> Option<String> {
        let mut lines = Vec::new();

        if let Some(ctx) = self.borrow_price_context.borrow().as_ref() {
            if let Some(price_usd) = price_usd(&ctx.price) {
                lines.push(format!(
                    "Borrow/Supply price: ~${}. 1 token = 10^{} base units.",
                    format_price(price_usd),
                    ctx.asset_decimals
                ));
            }
        }

        if let Some(ctx) = self.collateral_price_context.borrow().as_ref() {
            if let Some(price_usd) = price_usd(&ctx.price) {
                lines.push(format!(
                    "Collateral price: ~${}. 1 token = 10^{} base units.",
                    format_price(price_usd),
                    ctx.asset_decimals
                ));
            }
        }

        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    fn prompt_fees(&self, mut builder: ConfigBuilder) -> CliResult<ConfigBuilder> {
        logger::heading("\n💰 Fees\n");

        let has_origination_fee = Confirm::with_theme(self.theme)
            .with_prompt("Set borrow origination fee?")
            .default(true)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        if has_origination_fee {
            let fee_types = vec!["Flat amount", "Percentage"];
            let fee_type = Select::with_theme(self.theme)
                .with_prompt("Fee type")
                .items(&fee_types)
                .default(1)
                .interact()
                .map_err(|err| map_dialoguer_err(&err))?;

            if fee_type == 0 {
                let amount: u128 = prompt_until_valid(
                    || {
                        Input::with_theme(self.theme)
                            .with_prompt("Flat fee amount")
                            .interact_text()
                    },
                    Ok::<_, CliError>,
                )?;
                builder = builder.borrow_origination_fee(Fee::Flat(amount.into()));
            } else {
                let pct = prompt_until_valid(
                    || {
                        Input::with_theme(self.theme)
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

        builder = builder.supply_withdrawal_fee(TimeBasedFee::zero());

        Ok(builder)
    }

    #[allow(clippy::too_many_lines)]
    async fn prompt_yield_weights(&self, mut builder: ConfigBuilder) -> CliResult<ConfigBuilder> {
        logger::heading("\n🎯 Yield Distribution\n");

        let client = JsonRpcClient::connect(self.network.rpc_url());

        let share_percent =
            |weight: u16, total: u16| -> f64 { (f64::from(weight) / f64::from(total)) * 100.0 };

        let supply_weight: u16 = prompt_until_valid(
            || {
                Input::with_theme(self.theme)
                    .with_prompt("Supplier yield weight (relative weight)")
                    .default(9)
                    .interact_text()
            },
            |weight: u16| {
                if weight == 0 {
                    return Err(CliError::InvalidInput(
                        "Supplier weight must be greater than zero".into(),
                    ));
                }
                Ok(weight)
            },
        )?;

        let mut weights = YieldWeights::new_with_supply_weight(supply_weight);
        let mut total_weight = u16::from(weights.total_weight());
        let mut supply_share = share_percent(supply_weight, total_weight);
        println!("➡️  Current weights: total = {total_weight}, suppliers ≈ {supply_share:.2}%",);

        let add_static = Confirm::with_theme(self.theme)
            .with_prompt("Add static yield recipients (e.g., protocol revenue)?")
            .default(true)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        if add_static {
            loop {
                let account_id: AccountId = loop {
                    let account_id: AccountId = prompt_until_valid(
                        || {
                            Input::with_theme(self.theme)
                                .with_prompt("Static recipient account ID")
                                .interact_text()
                        },
                        |value: String| {
                            value.parse().map_err(|e| {
                                CliError::InvalidInput(format!("Invalid account ID: {e}"))
                            })
                        },
                    )?;

                    match view_account(&client, account_id.clone()).await {
                        Ok(_) => break account_id,
                        Err(e) => {
                            logger::warn(format!("Account check failed: {e}"));
                            let retry = Confirm::with_theme(self.theme)
                                .with_prompt("Re-enter static recipient account ID?")
                                .default(true)
                                .interact()
                                .map_err(|err| map_dialoguer_err(&err))?;
                            if retry {
                                continue;
                            }
                            let continue_anyway = Confirm::with_theme(self.theme)
                                .with_prompt("Continue anyway with this account ID?")
                                .default(false)
                                .interact()
                                .map_err(|err| map_dialoguer_err(&err))?;
                            if continue_anyway {
                                break account_id;
                            }
                        }
                    }
                };

                total_weight = u16::from(weights.total_weight());
                let previous_weight = weights.r#static.get(&account_id).copied().unwrap_or(0);
                let current_total = total_weight;
                let supply_share_before = share_percent(supply_weight, current_total);

                let weight: u16 = prompt_until_valid(
                    || {
                        let prompt =
                            format!("Static recipient weight (current total) {current_total}");
                        Input::with_theme(self.theme)
                            .with_prompt(prompt)
                            .default(previous_weight.max(1))
                            .interact_text()
                    },
                    |weight: u16| {
                        if weight == 0 {
                            return Err(CliError::InvalidInput(
                                "Static recipient weight must be greater than zero".into(),
                            ));
                        }
                        let prospective_total = u32::from(current_total)
                            .checked_sub(u32::from(previous_weight))
                            .and_then(|t| t.checked_add(u32::from(weight)))
                            .ok_or_else(|| {
                                CliError::InvalidInput(
                                    "Total yield weight would overflow u16".into(),
                                )
                            })?;
                        if prospective_total == 0 {
                            return Err(CliError::InvalidInput(
                                "Total yield weight must stay greater than zero".into(),
                            ));
                        }
                        Ok(weight)
                    },
                )?;

                let prospective_total = u32::from(current_total)
                    .checked_sub(u32::from(previous_weight))
                    .and_then(|t| t.checked_add(u32::from(weight)))
                    .ok_or_else(|| {
                        CliError::InvalidInput("Total yield weight would overflow u16".into())
                    })?;

                total_weight = u16::try_from(prospective_total).map_err(|_| {
                    CliError::InvalidInput("Total yield weight must fit within u16".into())
                })?;
                weights = weights.with_static(account_id.clone(), weight);
                supply_share = share_percent(supply_weight, total_weight);
                let static_share = share_percent(weight, total_weight);

                println!(
                    "➡️  Updated total weight = {total_weight}. Suppliers ≈ {supply_share:.2}%, {account_id} ≈ {static_share:.2}% (from {supply_share_before:.2}% for suppliers).",
                );

                let add_more = Confirm::with_theme(self.theme)
                    .with_prompt("Add another static recipient?")
                    .default(false)
                    .interact()
                    .map_err(|err| map_dialoguer_err(&err))?;
                if !add_more {
                    break;
                }
            }
        }
        builder = builder.yield_weights(weights);
        Ok(builder)
    }

    async fn prompt_fungible_asset<T: AssetClass>(
        &self,
        builder: ConfigBuilder,
        label: &str,
        nep141_example: &str,
        apply: impl Fn(ConfigBuilder, FungibleAsset<T>) -> CliResult<ConfigBuilder>,
        network: Network,
    ) -> CliResult<ConfigBuilder> {
        let prompt_ctx = PromptContext::new(self.theme);
        let asset_standard = Select::with_theme(self.theme)
            .with_prompt(format!("{label} type"))
            .items(["NEP-141 (fungible token)", "NEP-245 (multi-token)"])
            .default(0)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        let asset_standard = match asset_standard {
            0 => AssetStandard::Nep141,
            1 => AssetStandard::Nep245,
            _ => unreachable!(),
        };

        match asset_standard {
            AssetStandard::Nep141 => {
                let (builder, _) = prompt_account_with_validation(
                    &prompt_ctx,
                    Some(network),
                    builder,
                    &format!("{label} contract ID (e.g., {nep141_example})"),
                    None,
                    label,
                    |b, account| apply(b, FungibleAsset::nep141(account.clone())),
                )
                .await?;

                Ok(builder)
            }
            AssetStandard::Nep245 => {
                let (builder, contract_id) = prompt_account_with_validation(
                    &prompt_ctx,
                    Some(network),
                    builder,
                    &format!("{label} contract ID (NEP-245 multi-token)"),
                    None,
                    label,
                    |b, _| Ok(b),
                )
                .await?;

                let contract_id_str = contract_id.to_string();
                let rpc_url = network.rpc_url().to_string();
                let client = JsonRpcClient::connect(&rpc_url);
                let asset = loop {
                    let asset = prompt_until_valid(
                        || {
                            Input::with_theme(self.theme)
                                .with_prompt(format!("{label} token ID (string)"))
                                .interact_text()
                        },
                        |token_id: String| {
                            let composed = format!("nep245:{contract_id_str}:{token_id}");
                            parse_asset_input(&composed, label)
                        },
                    )?;

                    match check_asset_existence(&client, &asset).await {
                        Ok(()) => {
                            logger::success(format!("{label} token validated"));
                            break asset;
                        }
                        Err(err) => {
                            logger::warn(format!("Could not validate {label} token: {err}"));
                            let retry = Confirm::with_theme(self.theme)
                                .with_prompt(format!("Re-enter {label} token ID?"))
                                .default(true)
                                .interact()
                                .map_err(|err| map_dialoguer_err(&err))?;
                            if retry {
                                continue;
                            }
                            let continue_anyway = Confirm::with_theme(self.theme)
                                .with_prompt(format!(
                                    "Continue anyway with this {label} even though validation failed?"
                                ))
                                .default(false)
                                .interact()
                                .map_err(|err| map_dialoguer_err(&err))?;
                            if continue_anyway {
                                break asset;
                            }
                        }
                    }
                };

                apply(builder, asset)
            }
        }
    }

    fn edit_basic_config(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        logger::heading("\n📋 Basic Configuration");
        let prompt_ctx = PromptContext::new(self.theme);

        let time_chunk_ms: u64 = Input::with_theme(self.theme)
            .with_prompt("Time chunk duration (ms)")
            .default(config.time_chunk_configuration.duration_ms())
            .interact_text()
            .map_err(|err| map_dialoguer_err(&err))?;
        config.time_chunk_configuration = TimeChunkConfiguration::new(time_chunk_ms);

        config.borrow_asset = self.edit_fungible_asset("Borrow asset", &config.borrow_asset)?;

        config.collateral_asset =
            self.edit_fungible_asset("Collateral asset", &config.collateral_asset)?;

        config.protocol_account_id = prompt_ctx.prompt_account_id(
            "Protocol account ID",
            Some(config.protocol_account_id.to_string()),
            "protocol account",
        )?;

        Ok(())
    }

    fn edit_oracle_config(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        logger::heading("\n🔮 Oracle Settings");
        let prompt_ctx = PromptContext::new(self.theme);

        config.price_oracle_configuration.account_id = prompt_ctx.prompt_account_id(
            "Oracle account ID",
            Some(config.price_oracle_configuration.account_id.to_string()),
            "oracle account",
        )?;

        config.price_oracle_configuration.borrow_asset_price_id = prompt_ctx.prompt_price_id(
            "Borrow asset Pyth price ID (64 hex chars)",
            Some(
                config
                    .price_oracle_configuration
                    .borrow_asset_price_id
                    .to_string(),
            ),
        )?;

        let borrow_decimals: i32 = prompt_decimals(
            self.theme,
            "Borrow asset decimals",
            config.price_oracle_configuration.borrow_asset_decimals,
            "Borrow asset decimals",
        )?;
        config.price_oracle_configuration.borrow_asset_decimals = borrow_decimals;

        config.price_oracle_configuration.collateral_asset_price_id = prompt_ctx.prompt_price_id(
            "Collateral asset Pyth price ID (64 hex chars)",
            Some(
                config
                    .price_oracle_configuration
                    .collateral_asset_price_id
                    .to_string(),
            ),
        )?;

        let collateral_decimals: i32 = prompt_decimals(
            self.theme,
            "Collateral asset decimals",
            config.price_oracle_configuration.collateral_asset_decimals,
            "Collateral asset decimals",
        )?;
        config.price_oracle_configuration.collateral_asset_decimals = collateral_decimals;

        let price_max_age: u32 = Input::with_theme(self.theme)
            .with_prompt("Maximum price age (seconds)")
            .default(config.price_oracle_configuration.price_maximum_age_s)
            .interact_text()
            .map_err(|err| map_dialoguer_err(&err))?;
        config.price_oracle_configuration.price_maximum_age_s = price_max_age;

        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn edit_risk_parameters(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        logger::heading("\n⚖️  Risk Parameters");

        let mcr_maintenance_default = config.borrow_mcr_maintenance.to_string();
        let mcr_maintenance = prompt_until_valid(
            || {
                Input::with_theme(self.theme)
                    .with_prompt("Maintenance collateralization ratio (e.g., 1.25 for 125%)")
                    .default(mcr_maintenance_default.clone())
                    .interact_text()
            },
            |value: String| {
                let mcr = Decimal::from_str(&value)
                    .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
                if mcr <= Decimal::ONE {
                    return Err(CliError::InvalidInput(
                        "Maintenance MCR must be greater than 1.0".into(),
                    ));
                }
                Ok(mcr)
            },
        )?;
        config.borrow_mcr_maintenance = mcr_maintenance;

        let mcr_liquidation_default = config.borrow_mcr_liquidation.to_string();
        let mcr_liquidation = prompt_until_valid(
            || {
                Input::with_theme(self.theme)
                    .with_prompt("Liquidation collateralization ratio (e.g., 1.20 for 120%)")
                    .default(mcr_liquidation_default.clone())
                    .interact_text()
            },
            |value: String| {
                let mcr = Decimal::from_str(&value)
                    .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
                if mcr <= Decimal::ONE {
                    return Err(CliError::InvalidInput(
                        "Liquidation MCR must be greater than 1.0".into(),
                    ));
                }
                if mcr > mcr_maintenance {
                    return Err(CliError::InvalidInput(
                        "Liquidation MCR must be less than or equal to maintenance MCR".into(),
                    ));
                }
                Ok(mcr)
            },
        )?;
        config.borrow_mcr_liquidation = mcr_liquidation;

        let max_usage_default = config.borrow_asset_maximum_usage_ratio.to_string();
        let max_usage = prompt_until_valid(
            || {
                Input::with_theme(self.theme)
                    .with_prompt("Maximum usage ratio (e.g., 0.90 for 90%)")
                    .default(max_usage_default.clone())
                    .interact_text()
            },
            |value: String| {
                let ratio = Decimal::from_str(&value)
                    .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
                if ratio.is_zero() || ratio > Decimal::ONE {
                    return Err(CliError::InvalidInput(
                        "Maximum usage ratio must be > 0 and <= 1.0".into(),
                    ));
                }
                Ok(ratio)
            },
        )?;
        config.borrow_asset_maximum_usage_ratio = max_usage;

        let liquidation_spread_default = config.liquidation_maximum_spread.to_string();
        let liquidation_spread = prompt_until_valid(
            || {
                Input::with_theme(self.theme)
                    .with_prompt("Maximum liquidator spread (e.g., 0.05 for 5%)")
                    .default(liquidation_spread_default.clone())
                    .interact_text()
            },
            |value: String| {
                let spread = Decimal::from_str(&value)
                    .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
                if spread < Decimal::ZERO || spread >= Decimal::ONE {
                    return Err(CliError::InvalidInput(
                        "Liquidation spread must be >= 0 and < 1.0".into(),
                    ));
                }
                Ok(spread)
            },
        )?;
        config.liquidation_maximum_spread = liquidation_spread;

        let has_max_duration = Confirm::with_theme(self.theme)
            .with_prompt("Set maximum borrow duration?")
            .default(config.borrow_maximum_duration_ms.is_some())
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        config.borrow_maximum_duration_ms = if has_max_duration {
            let default_duration = config.borrow_maximum_duration_ms.map_or(0, |d| d.0);
            let max_duration_ms: u64 = Input::with_theme(self.theme)
                .with_prompt("Maximum borrow duration (milliseconds)")
                .default(default_duration)
                .interact_text()
                .map_err(|err| map_dialoguer_err(&err))?;
            Some(U64(max_duration_ms))
        } else {
            None
        };

        Ok(())
    }

    fn edit_interest_rate_strategy(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        logger::heading("\n📈 Interest Rate Strategy");
        let defaults = StrategyDefaults::from_strategy(&config.borrow_interest_rate_strategy)?;

        let strategy_types = StrategyKind::ALL.to_vec();
        let strategy_choice = Select::with_theme(self.theme)
            .with_prompt("Select interest rate model")
            .items(&strategy_types)
            .default(defaults.kind.as_index())
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        config.borrow_interest_rate_strategy = match strategy_choice {
            0 => {
                let base = prompt_decimal(
                    self.theme,
                    "Base rate at 0% utilization",
                    &defaults.get("base", "0.0"),
                    "linear base rate",
                )?;
                let top = prompt_decimal(
                    self.theme,
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
                    self.theme,
                    "Starting rate at 0% utilization",
                    &defaults.get("base", "0.0"),
                    "piecewise starting rate",
                )?;
                let optimal = prompt_decimal(
                    self.theme,
                    "Optimal utilization ratio (0-1)",
                    &defaults.get("optimal", "0.8"),
                    "piecewise optimal utilization",
                )?;
                let rate_1 = prompt_decimal(
                    self.theme,
                    "Rate at optimal utilization",
                    &defaults.get("rate_1", "0.0"),
                    "piecewise optimal rate",
                )?;
                let rate_2 = prompt_decimal(
                    self.theme,
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
                    self.theme,
                    "Base rate at 0% utilization",
                    &defaults.get("base", "0.0"),
                    "exponential base rate",
                )?;
                let top = prompt_decimal(
                    self.theme,
                    "Top rate at 100% utilization",
                    &defaults.get("top", "0.0"),
                    "exponential top rate",
                )?;
                let eccentricity = prompt_decimal(
                    self.theme,
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

    async fn edit_ranges(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        self.refresh_price_contexts(
            config.price_oracle_configuration.account_id.clone(),
            config.price_oracle_configuration.borrow_asset_price_id,
            config.price_oracle_configuration.collateral_asset_price_id,
            config.price_oracle_configuration.borrow_asset_decimals,
            config.price_oracle_configuration.collateral_asset_decimals,
            config.price_oracle_configuration.price_maximum_age_s,
        )
        .await;

        let defaults = crate::common::prompt::ranges::RangeDefaults {
            borrow_min: U128::from(config.borrow_range.minimum).0,
            borrow_max: config.borrow_range.maximum.map(|v| U128::from(v).0),
            supply_min: U128::from(config.supply_range.minimum).0,
            supply_max: config.supply_range.maximum.map(|v| U128::from(v).0),
            withdrawal_min: U128::from(config.supply_withdrawal_range.minimum).0,
            withdrawal_max: config
                .supply_withdrawal_range
                .maximum
                .map(|v| U128::from(v).0),
        };

        let mut hint = |label: &str, amount: u128| {
            self.print_price_hint(label, amount);
        };
        let selection = crate::common::prompt::ranges::prompt_ranges_with_validation(
            self.theme,
            &defaults,
            self.price_header_line(),
            &mut hint,
            |sel| {
                let _: templar_common::market::ValidAmountRange<
                    templar_common::asset::BorrowAsset,
                > = (sel.borrow_min, sel.borrow_max)
                    .try_into()
                    .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;
                let _: templar_common::market::ValidAmountRange<
                    templar_common::asset::BorrowAsset,
                > = (sel.supply_min, sel.supply_max)
                    .try_into()
                    .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;
                let _: templar_common::market::ValidAmountRange<
                    templar_common::asset::BorrowAsset,
                > = (sel.withdrawal_min, sel.withdrawal_max)
                    .try_into()
                    .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;
                Ok(())
            },
        )?;

        config.borrow_range = (selection.borrow_min, selection.borrow_max)
            .try_into()
            .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;
        config.supply_range = (selection.supply_min, selection.supply_max)
            .try_into()
            .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;
        config.supply_withdrawal_range = (selection.withdrawal_min, selection.withdrawal_max)
            .try_into()
            .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;

        Ok(())
    }

    fn edit_fees(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        logger::heading("\n💰 Fees");

        let (origination_default_idx, origination_default_value) =
            fee_defaults(&config.borrow_origination_fee);

        let origination_fee_type = Select::with_theme(self.theme)
            .with_prompt("Borrow origination fee type")
            .items(["Flat amount", "Percentage"])
            .default(origination_default_idx)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        config.borrow_origination_fee = if origination_fee_type == 0 {
            let amount: u128 = Input::with_theme(self.theme)
                .with_prompt("Flat fee amount")
                .default(origination_default_value.parse().unwrap_or(0))
                .interact_text()
                .map_err(|err| map_dialoguer_err(&err))?;
            Fee::Flat(amount.into())
        } else {
            let percentage = prompt_decimal(
                self.theme,
                "Fee percentage (e.g., 0.001 for 0.1%)",
                &origination_default_value,
                "origination fee percentage",
            )?;
            Fee::Proportional(percentage)
        };

        let (withdrawal_default_idx, withdrawal_default_value) =
            fee_defaults(&config.supply_withdrawal_fee.fee);

        let withdrawal_fee_type = Select::with_theme(self.theme)
            .with_prompt("Supply withdrawal fee type")
            .items(["Flat amount", "Percentage"])
            .default(withdrawal_default_idx)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        let withdrawal_fee = if withdrawal_fee_type == 0 {
            let amount: u128 = Input::with_theme(self.theme)
                .with_prompt("Withdrawal flat fee amount")
                .default(withdrawal_default_value.parse().unwrap_or(0))
                .interact_text()
                .map_err(|err| map_dialoguer_err(&err))?;
            Fee::Flat(amount.into())
        } else {
            let percentage = prompt_decimal(
                self.theme,
                "Withdrawal fee percentage",
                &withdrawal_default_value,
                "withdrawal fee percentage",
            )?;
            Fee::Proportional(percentage)
        };

        let duration_default = config.supply_withdrawal_fee.duration.0;
        let duration_ms: u64 = Input::with_theme(self.theme)
            .with_prompt("Withdrawal fee duration (ms)")
            .default(duration_default)
            .interact_text()
            .map_err(|err| map_dialoguer_err(&err))?;

        let behavior_idx = match config.supply_withdrawal_fee.behavior {
            TimeBasedFeeFunction::Fixed => 0,
            TimeBasedFeeFunction::Linear => 1,
        };

        let behavior_choice = Select::with_theme(self.theme)
            .with_prompt("Withdrawal fee behavior")
            .items(["Fixed (drops to zero after duration)", "Linear decay"])
            .default(behavior_idx)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        let behavior = if behavior_choice == 0 {
            TimeBasedFeeFunction::Fixed
        } else {
            TimeBasedFeeFunction::Linear
        };

        config.supply_withdrawal_fee = TimeBasedFee {
            fee: withdrawal_fee,
            duration: U64(duration_ms),
            behavior,
        };

        Ok(())
    }

    fn edit_yield_weights(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        logger::heading("\n🎯 Yield Distribution");

        let supply_weight: u16 = Input::with_theme(self.theme)
            .with_prompt("Supplier yield weight")
            .default(config.yield_weights.supply.get())
            .interact_text()
            .map_err(|err| map_dialoguer_err(&err))?;

        if supply_weight == 0 {
            return Err(CliError::InvalidInput(
                "Supplier yield weight must be greater than zero".into(),
            ));
        }

        let mut weights = YieldWeights::new_with_supply_weight(supply_weight);

        if !config.yield_weights.r#static.is_empty() {
            println!("Current static recipients:");
            for (account, weight) in &config.yield_weights.r#static {
                println!("- {account}: {weight}");
            }
        }

        let keep_static = Confirm::with_theme(self.theme)
            .with_prompt("Keep existing static recipients?")
            .default(!config.yield_weights.r#static.is_empty())
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        if keep_static {
            weights.r#static.clone_from(&config.yield_weights.r#static);
        } else {
            while Confirm::with_theme(self.theme)
                .with_prompt("Add a static recipient?")
                .default(weights.r#static.is_empty())
                .interact()
                .map_err(|err| map_dialoguer_err(&err))?
            {
                let account: String = Input::with_theme(self.theme)
                    .with_prompt("Static recipient account ID")
                    .interact_text()
                    .map_err(|err| map_dialoguer_err(&err))?;
                let weight: u16 = Input::with_theme(self.theme)
                    .with_prompt("Static recipient weight")
                    .default(1)
                    .interact_text()
                    .map_err(|err| map_dialoguer_err(&err))?;

                let account_id = AccountId::from_str(&account).map_err(|e| {
                    CliError::InvalidInput(format!("Invalid static recipient account: {e}"))
                })?;
                weights = weights.with_static(account_id, weight);
            }
        }

        config.yield_weights = weights;

        Ok(())
    }
}

fn strategy_label(strategy: &InterestRateStrategy) -> &'static str {
    match strategy {
        InterestRateStrategy::Linear(_) => "Linear",
        InterestRateStrategy::Piecewise(_) => "Piecewise",
        InterestRateStrategy::Exponential2(_) => "Exponential2",
    }
}

fn asset_defaults<T: AssetClass>(
    asset: &FungibleAsset<T>,
) -> (AssetStandard, String, Option<String>) {
    let asset_str = asset.to_string();
    let parts: Vec<&str> = asset_str.splitn(3, ':').collect();
    match parts.as_slice() {
        ["nep141", contract_id] => (AssetStandard::Nep141, (*contract_id).to_string(), None),
        ["nep245", contract_id, token_id] => (
            AssetStandard::Nep245,
            (*contract_id).to_string(),
            Some((*token_id).to_string()),
        ),
        _ => (AssetStandard::Nep141, asset.to_string(), None),
    }
}

fn default_strategy_index(strategies: &[InterestRateStrategy]) -> usize {
    strategies
        .iter()
        .position(|strategy| matches!(strategy, InterestRateStrategy::Piecewise(_)))
        .unwrap_or(0)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "Precision loss is acceptable for price hints"
)]
fn price_hint_amount(price: &Price, asset_decimals: i32, amount: u128) -> Option<(f64, f64)> {
    let price_usd = price_usd(price)?;

    let units = (amount as f64) / 10f64.powi(asset_decimals);
    if !units.is_finite() {
        return None;
    }

    Some((price_usd, units * price_usd))
}

#[allow(
    clippy::cast_precision_loss,
    reason = "Precision loss is acceptable for price hints"
)]
fn price_usd(price: &Price) -> Option<f64> {
    let price_raw = price.price.0;
    if price_raw <= 0 {
        return None;
    }
    let price_usd = (price_raw as f64) * 10f64.powi(price.expo);
    if !price_usd.is_finite() || price_usd <= 0.0 {
        return None;
    }
    Some(price_usd)
}

fn format_price(value: f64) -> String {
    if value.abs() >= 1.0 {
        format!("{value:.2}")
    } else {
        format!("{value:.6}")
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::prompt::ranges::RangeSelection;
    use rstest::rstest;
    use std::str::FromStr;
    use templar_common::market::YieldWeights;

    fn base_builder() -> ConfigBuilder {
        ConfigBuilder::new()
            .time_chunk_duration_ms(600_000)
            .borrow_asset("usdc.near")
            .unwrap()
            .collateral_asset("wnear.near")
            .unwrap()
            .oracle_account_id("pyth-oracle.near")
            .unwrap()
            .borrow_price_id([0xbb; 32])
            .borrow_decimals(6)
            .collateral_price_id([0xaa; 32])
            .collateral_decimals(24)
            .price_max_age_s(60)
            .borrow_mcr_maintenance(Decimal::from_str("1.25").unwrap())
            .borrow_mcr_liquidation(Decimal::from_str("1.20").unwrap())
            .borrow_max_usage_ratio(Decimal::from_str("0.90").unwrap())
            .borrow_origination_fee(Fee::zero())
            .borrow_interest_rate_strategy(
                InterestRateStrategy::linear(
                    Decimal::from_str("0.01").unwrap(),
                    Decimal::from_str("0.10").unwrap(),
                )
                .unwrap(),
            )
            .borrow_max_duration_ms(None)
            .supply_withdrawal_fee(templar_common::fee::TimeBasedFee::zero())
            .yield_weights(YieldWeights::new_with_supply_weight(9))
            .protocol_account_id("protocol.near")
            .unwrap()
            .liquidation_max_spread(Decimal::from_str("0.05").unwrap())
    }

    #[rstest]
    #[case(
        FungibleAsset::<BorrowAsset>::nep141("usdc.near".parse().unwrap()),
        AssetStandard::Nep141,
        "usdc.near",
        None
    )]
    #[case(
        FungibleAsset::<BorrowAsset>::nep245("mt.near".parse().unwrap(), "btc-token".to_string()),
        AssetStandard::Nep245,
        "mt.near",
        Some("btc-token")
    )]
    fn asset_defaults_handles_assets(
        #[case] asset: FungibleAsset<BorrowAsset>,
        #[case] expected_standard: AssetStandard,
        #[case] expected_contract: &str,
        #[case] expected_token: Option<&str>,
    ) {
        let (standard, contract, token) = asset_defaults(&asset);
        assert!(
            matches!(standard, s if matches!(expected_standard, AssetStandard::Nep141) && matches!(s, AssetStandard::Nep141)
            || matches!(expected_standard, AssetStandard::Nep245) && matches!(s, AssetStandard::Nep245))
        );
        assert_eq!(contract, expected_contract);
        assert_eq!(token.as_deref(), expected_token);
    }

    #[rstest]
    #[case(Some(10), Some(20), Some(30))]
    #[case(None, None, None)]
    #[case(Some(50), None, Some(40))]
    fn apply_ranges_to_builder_respects_withdrawal_max(
        #[case] borrow_max: Option<u128>,
        #[case] supply_max: Option<u128>,
        #[case] withdrawal_max: Option<u128>,
    ) {
        let selection = RangeSelection {
            borrow_min: 1,
            borrow_max,
            supply_min: 2,
            supply_max,
            withdrawal_min: 3,
            withdrawal_max,
        };

        let builder =
            crate::common::prompt::ranges::apply_ranges_to_builder(base_builder(), &selection)
                .expect("range application should succeed");
        let config = builder.build().expect("config should build");

        assert_eq!(
            config.borrow_range.maximum.map(u128::from),
            selection.borrow_max
        );
        assert_eq!(
            config.supply_range.maximum.map(u128::from),
            selection.supply_max
        );
        assert_eq!(
            config.supply_withdrawal_range.maximum.map(u128::from),
            selection.withdrawal_max
        );
    }
}
