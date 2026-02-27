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

fn mask_account_id(account_id: &AccountId) -> String {
    let chars: Vec<char> = account_id.as_str().chars().collect();
    let total = chars.len();
    if total <= 3 {
        return account_id.to_string();
    }

    let prefix_len = 3;
    let last_dot_pos = chars.iter().rposition(|c| *c == '.');
    let Some(dot_pos) = last_dot_pos else {
        let masked_len = total.saturating_sub(prefix_len);
        return chars[..prefix_len].iter().collect::<String>() + &"*".repeat(masked_len);
    };

    if dot_pos < prefix_len {
        return account_id.to_string();
    }

    let suffix = chars[dot_pos..].iter().collect::<String>();
    let masked_len = total.saturating_sub(prefix_len + suffix.chars().count());

    chars[..prefix_len].iter().collect::<String>() + &"*".repeat(masked_len) + &suffix
}

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
            let masked_account_id = mask_account_id(&account_id);

            println!(
                "➡️  Updated total weight = {total_weight}. Suppliers ≈ {supply_share:.2}%, {masked_account_id} ≈ {static_share:.2}% (from {supply_share_before:.2}% for suppliers).",
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
            let masked_account = mask_account_id(account);
            println!("- {masked_account}: {weight}");
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

#[cfg(test)]
mod tests {
    use super::mask_account_id;
    use near_sdk::AccountId;
    use rstest::rstest;
    use std::str::FromStr;

    #[test]
    fn mask_account_id_keeps_prefix_and_suffix() {
        let account = AccountId::from_str("revenue.tmplr.near").expect("valid account");
        assert_eq!(mask_account_id(&account), "rev**********.near");
    }

    #[test]
    fn mask_account_id_handles_no_dot() {
        let account = AccountId::from_str("protocol").expect("valid account");
        assert_eq!(mask_account_id(&account), "pro*****");
    }

    #[rstest]
    #[case("a.near", "a.near")] // dot_pos (1) < prefix_len (3), returns original
    #[case("ab.near", "ab.near")] // dot_pos (2) < prefix_len (3), returns original
    #[case("abc", "abc")] // total <= 3, returns original
    fn mask_account_id_short_accounts(#[case] input: &str, #[case] expected: &str) {
        let account = AccountId::from_str(input).expect("valid account");
        assert_eq!(mask_account_id(&account), expected);
    }

    #[rstest]
    #[case("alice.near", "ali**.near")] // 10 chars: 3 prefix + 2 masked + 5 suffix
    #[case("bob.testnet", "bob.testnet")] // 11 chars: masked_len = 11 - 3 - 8 = 0, unchanged
    #[case("verylongaccountname.near", "ver****************.near")] // 24 chars: 16 masked
    #[case("treasury.protocol.near", "tre**************.near")] // 22 chars: 14 masked
    #[case("sub.domain.testnet", "sub*******.testnet")] // 18 chars: 3 + 7 + 8, uses last dot
    fn mask_account_id_standard_accounts(#[case] input: &str, #[case] expected: &str) {
        let account = AccountId::from_str(input).expect("valid account");
        assert_eq!(mask_account_id(&account), expected);
    }

    #[test]
    fn mask_account_id_exact_prefix_length_account() {
        // Account where dot position equals prefix length
        let account = AccountId::from_str("abcd.near").expect("valid account");
        let masked = mask_account_id(&account);
        assert_eq!(masked, "abc*.near"); // 9 chars: 3 + 1 + 5
    }

    #[test]
    fn mask_account_id_longer_no_dot_account() {
        let account = AccountId::from_str("verylongname").expect("valid account");
        assert_eq!(mask_account_id(&account), "ver*********"); // 12 chars: 3 + 9
    }

    #[test]
    fn mask_account_id_preserves_full_suffix() {
        // Ensure the full suffix after the last dot is preserved
        let account = AccountId::from_str("user.testnet").expect("valid account");
        let masked = mask_account_id(&account);
        assert!(masked.ends_with(".testnet"));
    }

    #[test]
    fn mask_account_id_with_numbers() {
        let account = AccountId::from_str("user123.near").expect("valid account");
        assert_eq!(mask_account_id(&account), "use****.near"); // 12 chars: 3 + 4 + 5
    }

    #[test]
    fn mask_account_id_with_hyphens() {
        let account = AccountId::from_str("my-account.near").expect("valid account");
        assert_eq!(mask_account_id(&account), "my-*******.near"); // 15 chars: 3 + 7 + 5
    }

    #[test]
    fn mask_account_id_with_underscores() {
        let account = AccountId::from_str("my_account.near").expect("valid account");
        assert_eq!(mask_account_id(&account), "my_*******.near"); // 15 chars: 3 + 7 + 5
    }
}
