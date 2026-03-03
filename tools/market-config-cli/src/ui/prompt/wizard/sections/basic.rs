use crate::{
    logger,
    ui::prompt::{
        error::map_dialoguer_err,
        prompt_account_with_validation,
        wizard::{
            assets::{edit_fungible_asset, prompt_fungible_asset},
            types::prompt_until_valid,
        },
        PromptContext,
    },
    CliError, CliResult, ConfigBuilder,
};
use dialoguer::{theme::ColorfulTheme, Input};
use templar_common::{
    asset::{BorrowAsset, CollateralAsset},
    market::MarketConfiguration,
    time_chunk::TimeChunkConfiguration,
    utils::Network,
};

/// Prompts for basic configuration during interactive mode.
pub async fn prompt_basic_config(
    theme: &ColorfulTheme,
    mut builder: ConfigBuilder,
    network: Network,
) -> CliResult<ConfigBuilder> {
    logger::heading("\n📋 Basic Configuration\n");

    let prompt_ctx = PromptContext::new(theme);

    let time_chunk_default = builder.time_chunk_duration_ms_value().unwrap_or(600_000);
    let time_chunk_ms: u64 = prompt_until_valid(
        || {
            Input::with_theme(theme)
                .with_prompt("Time chunk duration (milliseconds)")
                .default(time_chunk_default)
                .interact_text()
        },
        Ok::<_, CliError>,
    )?;
    builder = builder.time_chunk_duration_ms(time_chunk_ms);

    builder = prompt_fungible_asset::<BorrowAsset>(
        theme,
        builder,
        "Borrow asset",
        match network {
            Network::Mainnet => "usdc.near",
            Network::Testnet => "usdc.testnet",
        },
        ConfigBuilder::borrow_fungible_asset,
        network,
    )
    .await?;

    builder = prompt_fungible_asset::<CollateralAsset>(
        theme,
        builder,
        "Collateral asset",
        match network {
            Network::Mainnet => "wrap.near",
            Network::Testnet => "wrap.testnet",
        },
        ConfigBuilder::collateral_fungible_asset,
        network,
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

/// Edits basic configuration on an existing market configuration.
pub fn edit_basic_config(theme: &ColorfulTheme, config: &mut MarketConfiguration) -> CliResult<()> {
    logger::heading("\n📋 Basic Configuration");
    let prompt_ctx = PromptContext::new(theme);

    let time_chunk_ms: u64 = Input::with_theme(theme)
        .with_prompt("Time chunk duration (ms)")
        .default(config.time_chunk_configuration.duration_ms())
        .interact_text()
        .map_err(|err| map_dialoguer_err(&err))?;
    config.time_chunk_configuration = TimeChunkConfiguration::new(time_chunk_ms);

    config.borrow_asset = edit_fungible_asset(theme, "Borrow asset", &config.borrow_asset)?;

    config.collateral_asset =
        edit_fungible_asset(theme, "Collateral asset", &config.collateral_asset)?;

    config.protocol_account_id = prompt_ctx.prompt_account_id(
        "Protocol account ID",
        Some(config.protocol_account_id.to_string()),
        "protocol account",
    )?;

    Ok(())
}
