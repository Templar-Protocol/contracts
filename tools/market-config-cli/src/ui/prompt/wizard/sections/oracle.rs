use crate::{
    config::validator::{fetch_metadata, TokenMetadata},
    logger,
    oracle::PriceValidator,
    ui::prompt::{
        error::map_dialoguer_err, helpers::prompt_decimals, prompt_account_with_validation,
        prompt_price_id_with_validation, wizard::types::prompt_until_valid, PromptContext,
    },
    CliError, CliResult, ConfigBuilder,
};
use dialoguer::{theme::ColorfulTheme, Confirm, Input};
use near_jsonrpc_client::JsonRpcClient;
use templar_common::{market::MarketConfiguration, utils::Network};

/// Prompts for oracle configuration during interactive mode.
#[allow(clippy::too_many_lines)]
pub async fn prompt_oracle_config(
    theme: &ColorfulTheme,
    builder: ConfigBuilder,
    network: Network,
) -> CliResult<ConfigBuilder> {
    logger::heading("\n🔮 Oracle Configuration\n");

    let prompt_ctx = PromptContext::new(theme);

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
        .ok_or_else(|| CliError::Validation("Borrow asset missing before oracle config".into()))?
        .clone();
    let collateral_asset = builder
        .collateral_asset_ref()
        .ok_or_else(|| {
            CliError::Validation("Collateral asset missing before oracle config".into())
        })?
        .clone();
    let borrow_metadata = match fetch_metadata(&client, &borrow_asset).await {
        Ok(TokenMetadata::Nep141(ref m)) => Some(TokenMetadata::Nep141(m.clone())),
        Ok(TokenMetadata::Nep245(ref m)) => Some(TokenMetadata::Nep245(m.clone())),
        Err(err) => {
            logger::warn(format!(
                "Unable to fetch borrow asset metadata: {err}. Skipping Hermes symbol check."
            ));
            None
        }
    };
    let collateral_metadata = match fetch_metadata(&client, &collateral_asset).await {
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
            theme,
            "Borrow asset decimals",
            borrow_expected_decimals.unwrap_or(6),
            "Borrow asset decimals",
        )?;
        if let Some(expected) = borrow_expected_decimals {
            if value != expected {
                logger::warn(format!(
                    "Borrow asset decimals mismatch: on-chain {expected}, entered {value}"
                ));
                let retry = Confirm::with_theme(theme)
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
            theme,
            "Collateral asset decimals",
            collateral_expected_decimals.unwrap_or(24),
            "Collateral asset decimals",
        )?;
        if let Some(expected) = collateral_expected_decimals {
            if value != expected {
                logger::warn(format!(
                    "Collateral asset decimals mismatch: on-chain {expected}, entered {value}"
                ));
                let retry = Confirm::with_theme(theme)
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
            Input::with_theme(theme)
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

/// Edits oracle configuration on an existing market configuration.
pub fn edit_oracle_config(
    theme: &ColorfulTheme,
    config: &mut MarketConfiguration,
) -> CliResult<()> {
    logger::heading("\n🔮 Oracle Settings");
    let prompt_ctx = PromptContext::new(theme);

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
        theme,
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
        theme,
        "Collateral asset decimals",
        config.price_oracle_configuration.collateral_asset_decimals,
        "Collateral asset decimals",
    )?;
    config.price_oracle_configuration.collateral_asset_decimals = collateral_decimals;

    let price_max_age: u32 = Input::with_theme(theme)
        .with_prompt("Maximum price age (seconds)")
        .default(config.price_oracle_configuration.price_maximum_age_s)
        .interact_text()
        .map_err(|err| map_dialoguer_err(&err))?;
    config.price_oracle_configuration.price_maximum_age_s = price_max_age;

    Ok(())
}
