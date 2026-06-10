use crate::{
    config::validator::check_asset_existence,
    logger,
    ui::prompt::{
        error::map_dialoguer_err, parsers::parse_asset_input, prompt_account_with_validation,
        PromptContext,
    },
    CliResult, ConfigBuilder,
};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use near_jsonrpc_client::JsonRpcClient;
use templar_common::{
    asset::{AssetClass, FungibleAsset},
    utils::Network,
};

use super::types::{prompt_until_valid, AssetStandard};

/// Extracts default values from an existing asset.
pub fn asset_defaults<T: AssetClass>(
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

/// Edits an existing fungible asset configuration.
pub fn edit_fungible_asset<T: AssetClass>(
    theme: &ColorfulTheme,
    label: &str,
    current: &FungibleAsset<T>,
) -> CliResult<FungibleAsset<T>> {
    let prompt_ctx = PromptContext::new(theme);
    let (default_standard, default_contract, default_token) = asset_defaults(current);

    let asset_standard = Select::with_theme(theme)
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
                    let mut input =
                        Input::with_theme(theme).with_prompt(format!("{label} token ID (string)"));
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

/// Prompts for a new fungible asset configuration.
pub async fn prompt_fungible_asset<T: AssetClass>(
    theme: &ColorfulTheme,
    builder: ConfigBuilder,
    label: &str,
    nep141_example: &str,
    apply: impl Fn(ConfigBuilder, FungibleAsset<T>) -> CliResult<ConfigBuilder>,
    network: Network,
) -> CliResult<ConfigBuilder> {
    let prompt_ctx = PromptContext::new(theme);
    let asset_standard = Select::with_theme(theme)
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
                        Input::with_theme(theme)
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
                        let retry = Confirm::with_theme(theme)
                            .with_prompt(format!("Re-enter {label} token ID?"))
                            .default(true)
                            .interact()
                            .map_err(|err| map_dialoguer_err(&err))?;
                        if retry {
                            continue;
                        }
                        let continue_anyway = Confirm::with_theme(theme)
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
