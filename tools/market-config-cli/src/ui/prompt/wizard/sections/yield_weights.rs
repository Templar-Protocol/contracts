use crate::{
    logger,
    rpc::view_account,
    ui::prompt::{error::map_dialoguer_err, wizard::types::prompt_until_valid},
    CliError, CliResult, ConfigBuilder,
};
use dialoguer::{theme::ColorfulTheme, Confirm, Input};
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::AccountId;
use std::str::FromStr;
use templar_common::{
    market::{MarketConfiguration, YieldWeights},
    utils::Network,
};

/// Prompts for yield weight configuration during interactive mode.
#[allow(clippy::too_many_lines)]
pub async fn prompt_yield_weights(
    theme: &ColorfulTheme,
    mut builder: ConfigBuilder,
    network: Network,
) -> CliResult<ConfigBuilder> {
    logger::heading("\n🎯 Yield Distribution\n");

    let client = JsonRpcClient::connect(network.rpc_url());

    let share_percent =
        |weight: u16, total: u16| -> f64 { (f64::from(weight) / f64::from(total)) * 100.0 };

    let supply_weight: u16 = prompt_until_valid(
        || {
            Input::with_theme(theme)
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

    let add_static = Confirm::with_theme(theme)
        .with_prompt("Add static yield recipients (e.g., protocol revenue)?")
        .default(true)
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;

    if add_static {
        loop {
            let account_id: AccountId = loop {
                let account_id: AccountId = prompt_until_valid(
                    || {
                        Input::with_theme(theme)
                            .with_prompt("Static recipient account ID")
                            .interact_text()
                    },
                    |value: String| {
                        value
                            .parse()
                            .map_err(|e| CliError::InvalidInput(format!("Invalid account ID: {e}")))
                    },
                )?;

                match view_account(&client, account_id.clone()).await {
                    Ok(_) => break account_id,
                    Err(e) => {
                        logger::warn(format!("Account check failed: {e}"));
                        let retry = Confirm::with_theme(theme)
                            .with_prompt("Re-enter static recipient account ID?")
                            .default(true)
                            .interact()
                            .map_err(|err| map_dialoguer_err(&err))?;
                        if retry {
                            continue;
                        }
                        let continue_anyway = Confirm::with_theme(theme)
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
                    let prompt = format!("Static recipient weight (current total) {current_total}");
                    Input::with_theme(theme)
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
                            CliError::InvalidInput("Total yield weight would overflow u16".into())
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

            let add_more = Confirm::with_theme(theme)
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

/// Edits yield weight configuration on an existing market configuration.
pub fn edit_yield_weights(
    theme: &ColorfulTheme,
    config: &mut MarketConfiguration,
) -> CliResult<()> {
    logger::heading("\n🎯 Yield Distribution");

    let supply_weight: u16 = Input::with_theme(theme)
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

    let keep_static = Confirm::with_theme(theme)
        .with_prompt("Keep existing static recipients?")
        .default(!config.yield_weights.r#static.is_empty())
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;

    if keep_static {
        weights.r#static.clone_from(&config.yield_weights.r#static);
    } else {
        while Confirm::with_theme(theme)
            .with_prompt("Add a static recipient?")
            .default(weights.r#static.is_empty())
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?
        {
            let account: String = Input::with_theme(theme)
                .with_prompt("Static recipient account ID")
                .interact_text()
                .map_err(|err| map_dialoguer_err(&err))?;
            let weight: u16 = Input::with_theme(theme)
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
