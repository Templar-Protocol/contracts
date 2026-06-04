mod assets;
mod display;
mod price;
mod sections;
#[cfg(test)]
mod tests;
pub mod types;

use crate::{
    config::{validator::set_progress_style, ConfigTemplate},
    logger,
    ui::prompt::{error::map_dialoguer_err, types::EditSection},
    CliResult, ConfigBuilder,
};
use dialoguer::{theme::ColorfulTheme, Confirm, MultiSelect, Select};
use display::print_step_overview;
use indicatif::ProgressBar;
use price::refresh_price_contexts;
use sections::{
    edit_basic_config, edit_fees, edit_interest_rate_strategy, edit_oracle_config, edit_ranges,
    edit_risk_parameters, edit_yield_weights, prompt_basic_config, prompt_fees,
    prompt_interest_rate_strategy, prompt_oracle_config, prompt_ranges, prompt_risk_parameters,
    prompt_yield_weights,
};
use std::cell::RefCell;
use templar_common::{market::MarketConfiguration, utils::Network, Decimal};
use types::{PriceHintContext, INTERACTIVE_STEPS};

pub struct MarketPrompter<'a> {
    theme: &'a ColorfulTheme,
    network: Network,
    borrow_price_context: RefCell<Option<PriceHintContext>>,
    collateral_price_context: RefCell<Option<PriceHintContext>>,
    eth_price_usd: RefCell<Option<Decimal>>,
    near_price_usd: RefCell<Option<Decimal>>,
}

impl<'a> MarketPrompter<'a> {
    pub fn new(theme: &'a ColorfulTheme, network: Network) -> Self {
        Self {
            theme,
            network,
            borrow_price_context: RefCell::new(None),
            collateral_price_context: RefCell::new(None),
            eth_price_usd: RefCell::new(None),
            near_price_usd: RefCell::new(None),
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
        builder = prompt_basic_config(self.theme, builder, self.network).await?;
        progress.inc(1);
        step_idx += 1;

        print_step_overview(&progress, &builder, step_idx, "Oracle configuration");
        builder = prompt_oracle_config(self.theme, builder, self.network).await?;
        progress.inc(1);
        step_idx += 1;

        print_step_overview(&progress, &builder, step_idx, "Risk parameters");
        builder = prompt_risk_parameters(self.theme, builder)?;
        progress.inc(1);
        step_idx += 1;

        print_step_overview(&progress, &builder, step_idx, "Interest rate strategy");
        builder = prompt_interest_rate_strategy(self.theme, builder)?;
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
            refresh_price_contexts(
                self.network,
                oracle_account_id,
                borrow_price_id,
                collateral_price_id,
                borrow_decimals,
                collateral_decimals,
                price_max_age,
                &self.borrow_price_context,
                &self.collateral_price_context,
                &self.eth_price_usd,
                &self.near_price_usd,
            )
            .await;
        }
        builder = prompt_ranges(
            self.theme,
            builder,
            &self.borrow_price_context,
            &self.collateral_price_context,
            &self.eth_price_usd,
            &self.near_price_usd,
        )?;
        progress.inc(1);
        step_idx += 1;

        print_step_overview(&progress, &builder, step_idx, "Fees");
        builder = prompt_fees(self.theme, builder)?;
        progress.inc(1);
        step_idx += 1;

        print_step_overview(&progress, &builder, step_idx, "Yield distribution");
        builder = prompt_yield_weights(self.theme, builder, self.network).await?;
        progress.inc(1);

        progress.set_message("Building configuration");
        let config = builder.build()?;
        progress.finish_with_message("✓ Setup complete");

        println!("\n✓ Configuration complete! Building...");

        Ok(config)
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
                EditSection::BasicConfiguration => {
                    edit_basic_config(self.theme, &mut config)?;
                }
                EditSection::OracleSettings => edit_oracle_config(self.theme, &mut config)?,
                EditSection::RiskParameters => edit_risk_parameters(self.theme, &mut config)?,
                EditSection::InterestRateStrategy => {
                    edit_interest_rate_strategy(self.theme, &mut config)?;
                }
                EditSection::Ranges => {
                    edit_ranges(
                        self.theme,
                        &mut config,
                        self.network,
                        &self.borrow_price_context,
                        &self.collateral_price_context,
                        &self.eth_price_usd,
                        &self.near_price_usd,
                    )
                    .await?;
                }
                EditSection::Fees => edit_fees(self.theme, &mut config)?,
                EditSection::YieldDistribution => edit_yield_weights(self.theme, &mut config)?,
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
}
