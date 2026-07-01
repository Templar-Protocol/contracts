//! Accumulator bot: periodically applies interest to borrow positions and
//! accumulates static yield across every market it discovers from a set of
//! registries.
//!
//! All NEAR reads and writes go through the in-process gateway library
//! ([`templar_gateway_client`]); this crate carries no bespoke RPC/transaction
//! plumbing. Reads use [`SigningClient::read`]; writes use
//! [`SigningClient::execute`], which signs and submits through the gateway's
//! operation driver (nonce sequencing, idempotency, and replay come for free).

use std::collections::HashMap;
use std::num::NonZeroU32;

use clap::Parser;
use futures::StreamExt;
use near_account_id::AccountId;
use near_api::SecretKey;
use templar_common::{borrow::BorrowPosition, market::MarketConfiguration};
use templar_gateway_client::{collect_paginated, Network, SigningClient};
use templar_gateway_methods_spec::{account, contract, market, registry};
use templar_gateway_types::{common::Pagination, Market};
use tracing::{debug, error, info, instrument};

/// Borrow positions keyed by account.
pub type BorrowPositions = HashMap<AccountId, BorrowPosition>;

/// Page size for listing borrow positions on a market.
#[allow(
    clippy::unwrap_used,
    reason = "compile-time const; a zero literal would fail to compile"
)]
const BORROW_POSITIONS_PAGE_SIZE: NonZeroU32 = NonZeroU32::new(100).unwrap();
/// Page size for listing deployments on a registry.
#[allow(
    clippy::unwrap_used,
    reason = "compile-time const; a zero literal would fail to compile"
)]
const DEPLOYMENTS_PAGE_SIZE: NonZeroU32 = NonZeroU32::new(500).unwrap();

#[derive(Debug, Clone, Parser)]
pub struct Args {
    /// Registries to run accumulator for.
    #[arg(short, long, env = "REGISTRIES_ACCOUNT_IDS", value_delimiter = ' ')]
    pub registries: Vec<AccountId>,
    /// Signer key to use for signing transactions.
    #[arg(short = 'k', long, env = "SIGNER_KEY")]
    pub signer_key: SecretKey,
    /// Signer account.
    #[arg(short, long, env = "SIGNER_ACCOUNT_ID")]
    pub signer_account: AccountId,
    /// Network to run accumulator on.
    #[arg(short, long, env = "NETWORK", default_value_t = Network::Testnet)]
    pub network: Network,
    /// Custom RPC URL (overrides default network RPC).
    #[arg(long, env = "RPC_URL")]
    pub rpc_url: Option<String>,
    /// API key for the RPC endpoint, sent as an `Authorization` header. May also
    /// be supplied as an `apiKey` query parameter on `--rpc-url`.
    #[arg(long, env = "RPC_API_KEY")]
    pub rpc_api_key: Option<String>,
    /// Interval between accumulations in seconds.
    #[arg(short, long, default_value_t = 600, env = "INTERVAL")]
    pub interval: u64,
    /// Interval between static accumulations in seconds.
    #[arg(long, default_value_t = 86_400, env = "STATIC_INTERVAL")]
    pub static_interval: u64,
    /// Registry refresh interval in seconds.
    #[arg(
        short = 'R',
        long,
        default_value_t = 3600,
        env = "REGISTRY_REFRESH_INTERVAL"
    )]
    pub registry_refresh_interval: u64,
    /// Concurrency for accumulation tasks.
    #[arg(short, long, default_value_t = 4, env = "CONCURRENCY")]
    pub concurrency: usize,
}

impl std::fmt::Display for Args {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "registries: {:?}\nsigner_account: {}\nnetwork: {}\ninterval: {}\nstatic_interval: {}\nregistry_refresh_interval: {}\nconcurrency: {}",
            self.registries,
            self.signer_account,
            self.network,
            self.interval,
            self.static_interval,
            self.registry_refresh_interval,
            self.concurrency
        )
    }
}

/// Accumulator for a single market, driving reads and writes through the
/// shared gateway client.
pub struct Accumulator {
    client: SigningClient,
    pub market: AccountId,
}

impl Accumulator {
    #[must_use]
    pub fn new(client: SigningClient, market: AccountId) -> Self {
        Self { client, market }
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_borrows(&self) -> anyhow::Result<BorrowPositions> {
        let client = self.client.clone();
        let market = self.market.clone();
        let entries = collect_paginated(BORROW_POSITIONS_PAGE_SIZE, move |offset, count| {
            let client = client.clone();
            let market = market.clone();
            async move {
                let result = client
                    .read(market::ListBorrowPositions {
                        market_id: market,
                        args: Pagination {
                            offset: Some(offset),
                            limit: Some(count),
                        },
                    })
                    .await?;
                anyhow::Ok(result.positions.into_iter().collect::<Vec<_>>())
            }
        })
        .await?;

        Ok(entries.into_iter().collect())
    }

    #[instrument(skip(self), level = "debug")]
    async fn apply_interest(&self, account_id: AccountId) -> anyhow::Result<()> {
        let result = self
            .client
            .execute(market::ApplyInterest {
                market_id: self.market.clone(),
                account_id: Some(account_id),
                snapshot_limit: None,
            })
            .await?;
        info!(operation_id = %result.operation.id.0, "Applied interest");
        Ok(())
    }

    #[instrument(skip(self), level = "info")]
    pub async fn run_borrow_accumulations(&self, concurrency: usize) -> anyhow::Result<()> {
        let borrows = match self.get_borrows().await {
            Ok(borrows) => borrows,
            Err(err) => {
                error!("Failed to fetch borrows for {}: {err}", self.market);
                return Ok(());
            }
        };

        if borrows.is_empty() {
            return Ok(());
        }

        futures::stream::iter(borrows)
            .map(|(account_id, _)| async move {
                if let Err(err) = self.apply_interest(account_id.clone()).await {
                    error!(
                        "Borrow accumulation failed for market {} account {}: {err}",
                        self.market, account_id
                    );
                }
            })
            .buffer_unordered(concurrency)
            .for_each(|()| async {})
            .await;

        Ok(())
    }

    /// Whether this market's deployed version performs static yield
    /// accumulation (>= 1.1.0). An undeterminable or unparseable version is
    /// treated as "no" (skip), mirroring the conservative legacy behaviour and
    /// `harvest-static-yield`.
    #[instrument(skip(self), level = "debug")]
    async fn supports_static_yield(&self) -> bool {
        let version = match self
            .client
            .read(contract::GetVersion {
                contract_id: self.market.clone(),
            })
            .await
        {
            Ok(version) => version,
            Err(err) => {
                debug!("Could not determine version for {}: {err}", self.market);
                return false;
            }
        };

        version
            .parsed
            .map(|version| version.cast::<Market>())
            .is_some_and(|version| version.requires_static_yield_accumulation())
    }

    #[instrument(skip(self), level = "debug")]
    async fn accumulate_static_yield(&self, account_id: AccountId) -> anyhow::Result<()> {
        let result = self
            .client
            .execute(market::AccumulateStaticYield {
                market_id: self.market.clone(),
                account_id: Some(account_id),
                snapshot_limit: None,
            })
            .await?;
        info!(operation_id = %result.operation.id.0, "Accumulated static yield");
        Ok(())
    }

    #[instrument(skip(self), level = "info")]
    pub async fn run_static_accumulations(&self, concurrency: usize) -> anyhow::Result<()> {
        if !self.supports_static_yield().await {
            debug!(
                "{} market does not support static yield accumulation",
                self.market
            );
            return Ok(());
        }

        let static_accounts = match self.get_static_accounts().await {
            Ok(accounts) => accounts,
            Err(err) => {
                error!("Failed to fetch static accounts for {}: {err}", self.market);
                return Ok(());
            }
        };

        if static_accounts.is_empty() {
            return Ok(());
        }

        futures::stream::iter(static_accounts)
            .map(|account_id| async move {
                if let Err(err) = self.accumulate_static_yield(account_id.clone()).await {
                    error!(
                        "Static accumulation failed for market {} account {}: {err}",
                        self.market, account_id
                    );
                }
            })
            .buffer_unordered(concurrency)
            .for_each(|()| async {})
            .await;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_static_accounts(&self) -> anyhow::Result<Vec<AccountId>> {
        let configuration = self
            .client
            .read(market::GetConfiguration {
                market_id: self.market.clone(),
            })
            .await?;

        Ok(static_accounts(&configuration))
    }
}

/// Accounts configured to receive static yield in a market configuration.
fn static_accounts(configuration: &MarketConfiguration) -> Vec<AccountId> {
    configuration
        .yield_weights
        .r#static
        .keys()
        .cloned()
        .collect()
}

/// Fetch every deployment across `registries` (paginated, concurrently),
/// keeping only accounts that still exist on chain.
pub async fn list_all_deployments(
    client: &SigningClient,
    registries: Vec<AccountId>,
    concurrency: usize,
) -> anyhow::Result<Vec<AccountId>> {
    let registry_count = registries.len();
    let per_registry = futures::stream::iter(registries)
        .map(|registry| {
            let client = client.clone();
            async move { list_deployments(&client, registry).await }
        })
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;

    let mut all_markets = Vec::new();
    let mut failures = 0_usize;
    for result in per_registry {
        match result {
            Ok(markets) => all_markets.extend(markets),
            Err(err) => {
                failures += 1;
                error!("Failed to list deployments: {err}");
            }
        }
    }

    // Distinguish "every registry failed" (transient outage — surface it so the
    // caller can keep its known-good state) from "registries returned nothing".
    if registry_count > 0 && failures == registry_count {
        anyhow::bail!("failed to list deployments from all {registry_count} configured registries");
    }

    let existing = futures::stream::iter(all_markets)
        .filter(|market_id| {
            let client = client.clone();
            let market_id = market_id.clone();
            async move { account_exists(&client, &market_id).await }
        })
        .collect::<Vec<AccountId>>()
        .await;

    Ok(existing)
}

/// List all deployments from a single registry, paginating until a short page.
async fn list_deployments(
    client: &SigningClient,
    registry: AccountId,
) -> anyhow::Result<Vec<AccountId>> {
    collect_paginated(DEPLOYMENTS_PAGE_SIZE, move |offset, count| {
        let client = client.clone();
        let registry = registry.clone();
        async move {
            let result = client
                .read(registry::ListDeployments {
                    registry_id: registry,
                    args: Pagination {
                        offset: Some(offset),
                        limit: Some(count),
                    },
                })
                .await?;
            anyhow::Ok(result.account_ids)
        }
    })
    .await
}

/// Whether an account currently exists on chain. Read errors are treated as
/// non-existent (best-effort filter), matching the legacy behaviour.
async fn account_exists(client: &SigningClient, account_id: &AccountId) -> bool {
    client
        .read(account::Get {
            account_id: account_id.clone(),
        })
        .await
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use templar_common::{
        asset::FungibleAsset,
        dec,
        fee::{Fee, TimeBasedFee},
        interest_rate_strategy::InterestRateStrategy,
        market::{PriceOracleConfiguration, YieldWeights},
        oracle::pyth::PriceIdentifier,
        time_chunk::TimeChunkConfiguration,
        Decimal,
    };

    fn sample_configuration(yield_weights: YieldWeights) -> MarketConfiguration {
        MarketConfiguration {
            time_chunk_configuration: TimeChunkConfiguration::new(1),
            borrow_asset: FungibleAsset::nep141("borrow.testnet".parse().unwrap()),
            collateral_asset: FungibleAsset::nep141("collateral.testnet".parse().unwrap()),
            price_oracle_configuration: PriceOracleConfiguration {
                account_id: "oracle.testnet".parse().unwrap(),
                collateral_asset_price_id: PriceIdentifier([1; 32]),
                collateral_asset_decimals: 24,
                borrow_asset_price_id: PriceIdentifier([2; 32]),
                borrow_asset_decimals: 24,
                price_maximum_age_s: 60,
            },
            borrow_mcr_maintenance: dec!("1.25"),
            borrow_mcr_liquidation: dec!("1.2"),
            borrow_asset_maximum_usage_ratio: dec!("0.9"),
            borrow_origination_fee: Fee::Proportional(dec!("0.01")),
            borrow_interest_rate_strategy: InterestRateStrategy::piecewise(
                Decimal::ZERO,
                dec!("0.8"),
                dec!("0.02"),
                dec!("0.5"),
            )
            .unwrap(),
            borrow_maximum_duration_ms: None,
            borrow_range: (1, None).try_into().unwrap(),
            supply_range: (1, None).try_into().unwrap(),
            supply_withdrawal_range: (1, None).try_into().unwrap(),
            supply_withdrawal_fee: TimeBasedFee::zero(),
            yield_weights,
            protocol_account_id: "protocol.testnet".parse().unwrap(),
            liquidation_maximum_spread: dec!("0.05"),
        }
    }

    #[test]
    fn static_accounts_extracts_configured_accounts() {
        let configuration = sample_configuration(
            YieldWeights::new_with_supply_weight(100)
                .with_static("static.one.testnet".parse().unwrap(), 50)
                .with_static("static.two.testnet".parse().unwrap(), 25),
        );

        let mut accounts = static_accounts(&configuration);
        accounts.sort();

        assert_eq!(
            accounts,
            vec![
                "static.one.testnet".parse::<AccountId>().unwrap(),
                "static.two.testnet".parse::<AccountId>().unwrap(),
            ]
        );
    }

    #[test]
    fn static_accounts_is_empty_without_static_weights() {
        let configuration = sample_configuration(YieldWeights::new_with_supply_weight(100));
        assert!(static_accounts(&configuration).is_empty());
    }
}
