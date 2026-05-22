pub mod error;
pub mod helpers;
pub mod parsers;
pub mod ranges;
pub mod types;
pub mod wizard;

pub use wizard::MarketPrompter;

use crate::{logger, oracle::PriceValidator, CliError, CliResult, ConfigBuilder, ConfigValidator};
use dialoguer::{theme::ColorfulTheme, Confirm, Input};
use error::map_dialoguer_err;
use helpers::{prompt_decimal, prompt_decimals};
use near_sdk::AccountId;
use parsers::{parse_asset_input, parse_price_id};
use templar_common::{
    asset::{AssetClass, FungibleAsset},
    oracle::pyth::PriceIdentifier,
};

pub struct PromptContext<'a> {
    pub theme: &'a ColorfulTheme,
}

impl<'a> PromptContext<'a> {
    pub fn new(theme: &'a ColorfulTheme) -> Self {
        Self { theme }
    }

    /// # Errors
    pub fn prompt_decimal_input(
        &self,
        prompt: &str,
        default: &str,
        field: &str,
    ) -> CliResult<templar_common::Decimal> {
        prompt_decimal(self.theme, prompt, default, field)
    }

    /// # Errors
    pub fn prompt_decimals(&self, prompt: &str, default: i32, field: &str) -> CliResult<i32> {
        prompt_decimals(self.theme, prompt, default, field)
    }

    /// # Errors
    pub fn prompt_account_id(
        &self,
        prompt: &str,
        default: Option<String>,
        label: &str,
    ) -> CliResult<AccountId> {
        let mut current_default = default;
        loop {
            let mut input = Input::with_theme(self.theme).with_prompt(prompt);
            if let Some(default) = current_default.clone() {
                input = input.default(default);
            }
            let value: String = input
                .interact_text()
                .map_err(|err| map_dialoguer_err(&err))?;
            match value.parse::<AccountId>() {
                Ok(account_id) => return Ok(account_id),
                Err(e) => {
                    logger::warn(format!("Invalid {label} account ID '{value}': {e}"));
                    current_default = Some(value);
                }
            }
        }
    }

    /// # Errors
    pub fn prompt_asset<T: AssetClass>(
        &self,
        prompt: &str,
        default: String,
        label: &str,
    ) -> CliResult<FungibleAsset<T>> {
        let value: String = Input::with_theme(self.theme)
            .with_prompt(prompt)
            .default(default)
            .interact_text()
            .map_err(|err| map_dialoguer_err(&err))?;
        parse_asset_input(&value, label)
    }

    /// # Errors
    pub fn prompt_price_id(
        &self,
        prompt: &str,
        default: Option<String>,
    ) -> CliResult<PriceIdentifier> {
        let mut input = Input::with_theme(self.theme).with_prompt(prompt);
        if let Some(default) = default {
            input = input.default(default);
        }
        let value: String = input
            .interact_text()
            .map_err(|err| map_dialoguer_err(&err))?;
        parse_price_id(&value)
    }
}

/// # Errors
pub async fn prompt_account_with_validation<F>(
    ctx: &PromptContext<'_>,
    network: Option<templar_common::utils::Network>,
    builder: ConfigBuilder,
    prompt: &str,
    default: Option<String>,
    label: &str,
    apply: F,
) -> CliResult<(ConfigBuilder, AccountId)>
where
    F: Fn(ConfigBuilder, &AccountId) -> CliResult<ConfigBuilder>,
{
    let mut account_id = ctx.prompt_account_id(prompt, default, label)?;
    let validator = ConfigValidator::new(network);

    loop {
        match validator.validate_account_id(&account_id).await {
            Ok(()) => {
                logger::success(format!("{label} validated"));
                let builder = apply(builder, &account_id)?;
                break Ok((builder, account_id));
            }
            Err(e) => {
                logger::warn(format!("Could not validate {label}: {e}"));
                let retry = Confirm::with_theme(ctx.theme)
                    .with_prompt(format!("Re-enter {label}?"))
                    .default(true)
                    .interact()
                    .map_err(|err| map_dialoguer_err(&err))?;
                if retry {
                    account_id =
                        ctx.prompt_account_id(prompt, Some(account_id.to_string()), label)?;
                    continue;
                }
                let continue_anyway = Confirm::with_theme(ctx.theme)
                    .with_prompt(format!(
                        "Continue anyway with this {label} even though validation failed?"
                    ))
                    .default(false)
                    .interact()
                    .map_err(|err| map_dialoguer_err(&err))?;
                if continue_anyway {
                    let builder = apply(builder, &account_id)?;
                    break Ok((builder, account_id));
                }
                return Err(CliError::Validation(format!(
                    "Validation failed for {label}: {e}"
                )));
            }
        }
    }
}

/// # Errors
pub async fn prompt_price_id_with_validation(
    ctx: &PromptContext<'_>,
    validator: &PriceValidator,
    oracle_account_id: AccountId,
    token_symbol: Option<&str>,
    prompt: &str,
    default: Option<String>,
    label: &str,
) -> CliResult<PriceIdentifier> {
    let price_id = loop {
        let price_id = match ctx.prompt_price_id(prompt, default.clone()) {
            Ok(value) => value,
            Err(err) => {
                if matches!(err, CliError::Interrupted) {
                    return Err(err);
                }
                logger::warn(format!("Invalid {label}: {err}"));
                println!("Please try again.\n");
                continue;
            }
        };
        match validator
            .validate_price_feed(oracle_account_id.clone(), &price_id)
            .await
        {
            Ok(()) => {
                if let Some(symbol) = token_symbol {
                    if let Err(e) = validator
                        .validate_price_feed_matches_symbol(symbol, &price_id)
                        .await
                    {
                        logger::warn(format!("Could not validate {label}: {e}"));
                        let continue_anyway = Confirm::with_theme(ctx.theme)
                            .with_prompt(format!("{label} validation failed. Continue anyway?"))
                            .default(false)
                            .interact()
                            .map_err(|err| map_dialoguer_err(&err))?;
                        if continue_anyway {
                            break price_id;
                        }
                    } else {
                        logger::success(format!("{label} validated"));
                        break price_id;
                    }
                } else {
                    logger::info(format!(
                        "{label} validated (symbol metadata unavailable; skipping Hermes check)"
                    ));
                    break price_id;
                }
            }
            Err(e) => {
                logger::warn(format!("Could not validate {label}: {e}"));
                let continue_anyway = Confirm::with_theme(ctx.theme)
                    .with_prompt(format!("{label} validation failed. Continue anyway?"))
                    .default(false)
                    .interact()
                    .map_err(|err| map_dialoguer_err(&err))?;
                if continue_anyway {
                    break price_id;
                }
                // If not continuing, loop again for another attempt
            }
        }
    };

    Ok(price_id)
}
