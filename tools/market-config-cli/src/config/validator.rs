use crate::{
    oracle::PriceValidator,
    rpc::{ft_metadata, multitoken_metadata, view_account, MultiTokenMetadata},
    CliError, CliResult,
};
use console::{style, Style};
use indicatif::{ProgressBar, ProgressStyle};
use near_contract_standards::fungible_token::metadata::FungibleTokenMetadata;
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::AccountId;
use serde_json::Value;
use templar_common::{asset::FungibleAsset, market::MarketConfiguration, utils::Network};

pub struct ConfigValidator {
    network: Option<Network>,
}

impl ConfigValidator {
    pub fn new(network: Option<Network>) -> Self {
        Self { network }
    }

    pub fn rpc_url(&self) -> String {
        match self.network {
            Some(n) => n.rpc_url().to_string(),
            None => Network::default().rpc_url().to_string(),
        }
    }

    /// Validate that an account ID exists on-chain
    /// # Errors
    pub async fn validate_account_id(&self, account_id: &AccountId) -> CliResult {
        let rpc_url = self.rpc_url();
        let rpc_client = JsonRpcClient::connect(&rpc_url);

        view_account(&rpc_client, account_id.clone())
            .await
            .map_err(|e| {
                CliError::Validation(format!("Account check failed for '{account_id}': {e}"))
            })?;

        Ok(())
    }

    /// Validate a ``MarketConfiguration`` struct
    /// # Errors
    pub async fn validate(&self, config: &MarketConfiguration) -> CliResult {
        // Use the built-in validation
        config
            .validate()
            .map_err(|e| CliError::Validation(e.to_string()))?;

        // Additional custom validations
        self.validate_accounts(config).await?;
        validate_decimals(config)?;
        // If network is provided, validate with oracle
        if let Some(network) = self.network {
            self.validate_with_oracle(config, network).await?;
        }

        println!("✅ All configuration checks passed");

        Ok(())
    }

    /// Offline-only validation that skips RPC/oracle checks.
    /// Useful for tests and environments without network access.
    /// # Errors
    pub fn validate_offline(&self, config: &MarketConfiguration) -> CliResult {
        config
            .validate()
            .map_err(|e| CliError::Validation(e.to_string()))?;
        validate_decimals(config)?;
        Ok(())
    }

    /// Validate a JSON representation of a config
    /// # Errors
    pub async fn validate_json(&self, config_json: &Value) -> CliResult {
        let config: MarketConfiguration = serde_json::from_value(config_json.clone())
            .map_err(|e| CliError::Validation(format!("Failed to parse configuration: {e}")))?;

        self.validate(&config)
            .await
            .map_err(|e| CliError::Validation(format!("Configuration validation failed: {e}")))
    }

    async fn validate_with_oracle(
        &self,
        config: &MarketConfiguration,
        network: Network,
    ) -> CliResult {
        let validator = PriceValidator::new(network);
        let oracle_account_id = config.price_oracle_configuration.account_id.clone();

        // Run sequentially to keep progress indicators readable.
        self.validate_token_decimals(config).await?;
        self.validate_oracle_feeds(&validator, config, &oracle_account_id)
            .await?;

        Ok(())
    }

    async fn validate_oracle_feeds(
        &self,
        validator: &PriceValidator,
        config: &MarketConfiguration,
        oracle_account_id: &AccountId,
    ) -> CliResult {
        let progress = ProgressBar::new(2);

        set_progress_style(&progress, "oracle feeds");

        let borrow = validator.validate_price_feed(
            oracle_account_id.clone(),
            &config.price_oracle_configuration.borrow_asset_price_id,
        );
        let collateral = validator.validate_price_feed(
            oracle_account_id.clone(),
            &config.price_oracle_configuration.collateral_asset_price_id,
        );

        progress.set_message("borrow feed");
        borrow.await?;
        progress.inc(1);

        progress.set_message("collateral feed");
        collateral.await?;
        progress.inc(1);

        progress.finish_with_message(
            Style::new()
                .green()
                .apply_to("✓ Oracle feeds validated")
                .to_string(),
        );

        Ok(())
    }

    async fn validate_accounts(&self, config: &MarketConfiguration) -> CliResult {
        let rpc_url = self.rpc_url();
        let rpc_client = JsonRpcClient::connect(&rpc_url);

        let accounts = vec![
            (
                "price oracle",
                config.price_oracle_configuration.account_id.to_string(),
            ),
            ("protocol", config.protocol_account_id.to_string()),
            (
                "borrow asset",
                config.borrow_asset.contract_id().as_ref().to_string(),
            ),
            (
                "collateral asset",
                config.collateral_asset.contract_id().as_ref().to_string(),
            ),
        ];
        let mut accounts = accounts;
        for account in config.yield_weights.r#static.keys() {
            accounts.push(("yield recipient", account.to_string()));
        }

        let progress = ProgressBar::new(accounts.len() as u64);

        set_progress_style(&progress, "accounts");

        for (label, account) in accounts {
            progress.set_message(label);

            let account_id: AccountId = account.parse().map_err(|e| {
                CliError::Validation(format!("Invalid {label} account ID '{account}': {e}",))
            })?;

            view_account(&rpc_client, account_id.clone())
                .await
                .map_err(|e| {
                    CliError::Validation(format!("Account check failed for '{account_id}': {e}"))
                })?;

            progress.inc(1);
        }

        progress.finish_with_message(
            Style::new()
                .green()
                .apply_to("✓ Accounts validated")
                .to_string(),
        );

        Ok(())
    }

    async fn validate_token_decimals(&self, config: &MarketConfiguration) -> CliResult {
        let (borrow_decimals, collateral_decimals) = self.check_token_metadata(config).await?;

        if i32::from(borrow_decimals) != config.price_oracle_configuration.borrow_asset_decimals {
            return Err(CliError::Validation(format!(
                "Borrow asset decimals mismatch: config {} vs on-chain {}",
                config.price_oracle_configuration.borrow_asset_decimals, borrow_decimals
            )));
        }

        if i32::from(collateral_decimals)
            != config.price_oracle_configuration.collateral_asset_decimals
        {
            return Err(CliError::Validation(format!(
                "Collateral asset decimals mismatch: config {} vs on-chain {}",
                config.price_oracle_configuration.collateral_asset_decimals, collateral_decimals
            )));
        }

        Ok(())
    }

    /// Validate that token metadata view calls succeed and return decimals.
    /// Returns the decimals fetched for borrow and collateral assets (in that order).
    async fn check_token_metadata(&self, config: &MarketConfiguration) -> CliResult<(u8, u8)> {
        let rpc_url = self.rpc_url();
        let client = JsonRpcClient::connect(&rpc_url);

        let progress = ProgressBar::new(2);
        set_progress_style(&progress, "token metadata");
        progress.tick();

        progress.set_message("borrow asset");
        let borrow_metadata = fetch_metadata(&client, &config.borrow_asset)
            .await
            .map_err(|e| {
                CliError::Validation(format!("Failed to fetch borrow asset metadata: {e}"))
            })?;
        progress.inc(1);

        progress.set_message("collateral asset");
        let collateral_metadata = fetch_metadata(&client, &config.collateral_asset)
            .await
            .map_err(|e| {
                CliError::Validation(format!("Failed to fetch collateral asset metadata: {e}"))
            })?;
        progress.inc(1);

        let borrow_decimals = match borrow_metadata {
            TokenMetadata::Nep141(ref m) => m.decimals,
            TokenMetadata::Nep245(ref m) => m.decimals,
        };
        let collateral_decimals = match collateral_metadata {
            TokenMetadata::Nep141(ref m) => m.decimals,
            TokenMetadata::Nep245(ref m) => m.decimals,
        };

        progress.finish_with_message(
            Style::new()
                .green()
                .apply_to("✓ Token metadata validated")
                .to_string(),
        );

        Ok((borrow_decimals, collateral_decimals))
    }
}

pub enum TokenMetadata {
    Nep141(FungibleTokenMetadata),
    Nep245(MultiTokenMetadata),
}

async fn fetch_metadata<T: templar_common::asset::AssetClass>(
    client: &JsonRpcClient,
    asset: &FungibleAsset<T>,
) -> CliResult<TokenMetadata> {
    if let Some((contract_id, token_id)) = asset.clone().into_nep245() {
        let (resolved_contract, resolved_token_id) =
            resolve_nep141_or_nep245_parts(contract_id, &token_id)?;
        let token_id = resolved_token_id.ok_or_else(|| {
            CliError::Validation(format!("Missing NEP-245 token id for asset '{asset}'"))
        })?;
        // return multitoken_metadata(client, resolved_contract, token_id).await;

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

/// # Errors
pub async fn check_asset_existence<T: templar_common::asset::AssetClass>(
    client: &JsonRpcClient,
    asset: &FungibleAsset<T>,
) -> CliResult {
    if let Some((contract_id, token_id)) = asset.clone().into_nep245() {
        let (resolved_contract, resolved_token_id) =
            resolve_nep141_or_nep245_parts(contract_id, &token_id)?;
        // NEP-245 multi-token
        view_account(client, resolved_contract.clone())
            .await
            .map_err(|e| {
                CliError::Validation(format!(
                    "Account check failed for '{resolved_contract}': {e}"
                ))
            })?;

        if let Some(resolved_token_id) = resolved_token_id {
            let metadata =
                multitoken_metadata(client, resolved_contract, resolved_token_id.clone())
                    .await
                    .map_err(|e| {
                        CliError::Validation(format!("Failed to fetch NEP-245 metadata: {e}"))
                    })?;
            let metadata_id = metadata
                .id
                .as_deref()
                .ok_or_else(|| CliError::Validation("NEP-245 metadata missing id field".into()))?;
            if metadata_id != resolved_token_id {
                return Err(CliError::Validation(format!(
                    "NEP-245 token id mismatch: expected '{resolved_token_id}', got '{metadata_id}'"
                )));
            }
        }
    }

    // NEP-141
    let account_id: AccountId = asset.contract_id().as_ref().parse().map_err(|e| {
        CliError::Other(format!(
            "Unable to parse account_id '{}': {e}",
            asset.contract_id().as_ref()
        ))
    })?;
    view_account(client, account_id.clone())
        .await
        .map_err(|e| {
            CliError::Validation(format!("Account check failed for '{account_id}': {e}"))
        })?;
    Ok(())
}

pub(crate) fn resolve_nep141_or_nep245_parts(
    contract_id: AccountId,
    token_id: &str,
) -> CliResult<(AccountId, Option<String>)> {
    if let Some(stripped) = token_id.strip_prefix("nep141:") {
        let mut parts = stripped.splitn(2, ':');
        let Some(ft_contract) = parts.next() else {
            return Err(CliError::Validation(format!(
                "Invalid NEP-245 token id '{token_id}'"
            )));
        };

        let ft_contract = ft_contract.parse::<AccountId>().map_err(|e| {
            CliError::Validation(format!(
                "Invalid NEP-245 contract id in token '{token_id}': {e}"
            ))
        })?;

        return Ok((ft_contract, None));
    }

    if let Some(stripped) = token_id.strip_prefix("nep245:") {
        let mut parts = stripped.splitn(2, ':');
        let Some(mt_contract) = parts.next() else {
            return Err(CliError::Validation(format!(
                "Invalid NEP-245 token id '{token_id}'"
            )));
        };
        let Some(mt_token_id) = parts.next() else {
            return Err(CliError::Validation(format!(
                "Invalid NEP-245 token id '{token_id}'"
            )));
        };

        let mt_contract = mt_contract.parse::<AccountId>().map_err(|e| {
            CliError::Validation(format!(
                "Invalid NEP-245 contract id in token '{token_id}': {e}"
            ))
        })?;
        return Ok((mt_contract, Some(mt_token_id.to_string())));
    }

    Ok((contract_id, Some(token_id.to_string())))
}

/// Validate that decimals are within reasonable bounds (typically 0-24 for NEAR)
fn validate_decimals(config: &MarketConfiguration) -> CliResult {
    let progress = ProgressBar::new(2);
    set_progress_style(&progress, "decimals");

    progress.set_message("borrow decimals");
    if config.price_oracle_configuration.borrow_asset_decimals > 24 {
        return Err(CliError::Validation(
            "Borrow asset decimals must be <= 24".into(),
        ));
    }
    progress.inc(1);

    progress.set_message("collateral decimals");
    if config.price_oracle_configuration.collateral_asset_decimals > 24 {
        return Err(CliError::Validation(
            "Collateral asset decimals must be <= 24".into(),
        ));
    }
    progress.inc(1);

    progress.finish_with_message(
        Style::new()
            .green()
            .apply_to("✓ Decimals validated")
            .to_string(),
    );

    Ok(())
}

#[allow(clippy::unwrap_used, reason = "Styling should not fail")]
pub fn set_progress_style(progress: &ProgressBar, section: &str) {
    let template = format!(
        "🔎 {} {section} [{{bar:20.cyan/blue}}] {{pos}}/{{len}} {{msg}}",
        style("Validating").yellow()
    );
    progress.set_style(
        ProgressStyle::with_template(&template)
            .unwrap()
            .progress_chars("=>-"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::AccountId;
    use std::str::FromStr;
    use templar_common::{
        asset::FungibleAsset,
        fee::{Fee, TimeBasedFee},
        interest_rate_strategy::InterestRateStrategy,
        market::{PriceOracleConfiguration, YieldWeights},
        number::Decimal,
        oracle::pyth::PriceIdentifier,
        time_chunk::TimeChunkConfiguration,
    };

    fn create_valid_config() -> MarketConfiguration {
        MarketConfiguration {
            time_chunk_configuration: TimeChunkConfiguration::new(600_000),
            borrow_asset: FungibleAsset::nep141(AccountId::from_str("usdc.near").unwrap()),
            collateral_asset: FungibleAsset::nep141(AccountId::from_str("wnear.near").unwrap()),
            price_oracle_configuration: PriceOracleConfiguration {
                account_id: AccountId::from_str("pyth-oracle.near").unwrap(),
                collateral_asset_price_id: PriceIdentifier([0xaa; 32]),
                collateral_asset_decimals: 24,
                borrow_asset_price_id: PriceIdentifier([0xbb; 32]),
                borrow_asset_decimals: 6,
                price_maximum_age_s: 60,
            },
            borrow_mcr_maintenance: Decimal::from(125u32) / 100u32,
            borrow_mcr_liquidation: Decimal::from(120u32) / 100u32,
            borrow_asset_maximum_usage_ratio: Decimal::from(99u32) / 100u32,
            borrow_origination_fee: Fee::zero(),
            borrow_interest_rate_strategy: InterestRateStrategy::linear(
                Decimal::from(5u32) / 100u32,
                Decimal::from(10u32) / 100u32,
            )
            .unwrap(),
            borrow_maximum_duration_ms: None,
            borrow_range: (1_000_000, None).try_into().unwrap(),
            supply_range: (1_000_000, None).try_into().unwrap(),
            supply_withdrawal_range: (1_000_000, None).try_into().unwrap(),
            supply_withdrawal_fee: TimeBasedFee::zero(),
            yield_weights: YieldWeights::new_with_supply_weight(10),
            protocol_account_id: AccountId::from_str("protocol.near").unwrap(),
            liquidation_maximum_spread: Decimal::from(5u32) / 100u32,
        }
    }

    #[tokio::test]
    async fn test_valid_config() {
        let validator = ConfigValidator::new(None);
        let config = create_valid_config();
        assert!(validator.validate_offline(&config).is_ok());
    }

    #[tokio::test]
    async fn test_invalid_decimals() {
        let validator = ConfigValidator::new(None);
        let mut config = create_valid_config();
        config.price_oracle_configuration.borrow_asset_decimals = 30;

        let result = validator.validate_offline(&config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("decimals must be <= 24"));
    }

    #[tokio::test]
    async fn withdrawal_minimum_above_supply_rejected() {
        let validator = ConfigValidator::new(None);
        let mut config = create_valid_config();
        config.supply_range = (1_000_000_000_000_000_000_000_000_000u128, None)
            .try_into()
            .unwrap();
        config.supply_withdrawal_range = (1_000_000_000_000_000_000_000_000_001u128, None)
            .try_into()
            .unwrap();

        let result = validator.validate_offline(&config);
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("supply_withdrawal_range.minimum"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn resolve_nep141_or_nep245_parts_supports_nep141_prefix() {
        let contract_id = AccountId::from_str("intents.near").unwrap();
        let token_id = "nep141:usdc.near:anything".to_string();

        let (resolved_contract, resolved_token_id) =
            resolve_nep141_or_nep245_parts(contract_id, &token_id).unwrap();

        assert_eq!(resolved_contract, AccountId::from_str("usdc.near").unwrap());
        assert!(resolved_token_id.is_none());
    }

    #[test]
    fn resolve_nep141_or_nep245_parts_delegates_nep245() {
        let contract_id = AccountId::from_str("intents.near").unwrap();
        let token_id = "nep245:intents.near:foo".to_string();

        let (resolved_contract, resolved_token_id) =
            resolve_nep141_or_nep245_parts(contract_id.clone(), &token_id).unwrap();

        assert_eq!(resolved_contract, contract_id);
        assert_eq!(resolved_token_id, Some("foo".to_string()));
    }

    #[test]
    fn resolve_nep141_or_nep245_parts_parses_nep245_prefix() {
        let contract_id = AccountId::from_str("intents.near").unwrap();
        let token_id = "nep245:intents.near:bar".to_string();

        let (resolved_contract, resolved_token_id) =
            resolve_nep141_or_nep245_parts(contract_id.clone(), &token_id).unwrap();

        assert_eq!(resolved_contract, contract_id);
        assert_eq!(resolved_token_id, Some("bar".to_string()));
    }

    #[test]
    fn resolve_nep141_or_nep245_parts_passthrough_when_no_prefix() {
        let contract_id = AccountId::from_str("intents.near").unwrap();
        let token_id = "plain-token".to_string();

        let (resolved_contract, resolved_token_id) =
            resolve_nep141_or_nep245_parts(contract_id.clone(), &token_id).unwrap();

        assert_eq!(resolved_contract, contract_id);
        assert_eq!(resolved_token_id, Some(token_id));
    }
}
