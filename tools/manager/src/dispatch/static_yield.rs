//! `market static-yield harvest`: accumulate and withdraw the signer's static
//! yield on every listed market, then optionally forward the proceeds.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use anyhow::Context as _;
use near_account_id::{AccountId, AccountIdRef};
use serde::Serialize;
use templar_common::asset::{BorrowAsset, BorrowAssetAmount, FungibleAsset};
use templar_gateway_client::Client;
use templar_gateway_methods_spec::{contract, market, registry, storage, token};
use templar_gateway_types::{
    common::{Pagination, WriteOperationResult},
    ContractKind, ManagedAccountId, Market, NearToken, OperationId,
};

use crate::commands::market::static_yield::Harvest;
use crate::context::{failed_receipt_contracts, print_json, CliContext};

pub(super) async fn harvest(ctx: CliContext, args: Harvest) -> anyhow::Result<()> {
    anyhow::ensure!(
        args.receiver_id.as_ref() != Some(&args.signer.account_id().0),
        "--receiver-id is the signer; omit it to keep the yield on the signer"
    );
    let (signer, client) = ctx.signing_client_for(&args.signer).await?;
    let market_ids = resolve_markets(&ctx, args.registry_id, args.market_id).await?;

    let mut markets = Vec::with_capacity(market_ids.len());
    for market_id in market_ids {
        let outcome = harvest_market(&ctx, &client, &signer, &market_id)
            .await
            .unwrap_or_else(|error| {
                let error = format!("{error:#}");
                tracing::error!(%market_id, %error, "failed to harvest static yield");
                MarketOutcome::Failed { error }
            });
        markets.push(MarketReport { market_id, outcome });
    }

    let harvested = totals(&markets);
    let transfers = match &args.receiver_id {
        Some(receiver_id) => forward(&ctx, &client, &signer, receiver_id, &harvested).await,
        None => Vec::new(),
    };

    let report = HarvestReport {
        markets,
        harvested,
        transfers,
    };
    print_json(&report)?;
    eprint!("{}", report.summary());
    report.ensure_succeeded()
}

/// The explicit markets plus every market deployed from each registry, deduplicated.
async fn resolve_markets(
    ctx: &CliContext,
    registry_ids: Vec<AccountId>,
    market_ids: Vec<AccountId>,
) -> anyhow::Result<BTreeSet<AccountId>> {
    let mut markets: BTreeSet<AccountId> = market_ids.into_iter().collect();
    for registry_id in registry_ids {
        let deployed = ctx
            .client
            .read(registry::ListDeploymentsByKind {
                registry_id: registry_id.clone(),
                args: Pagination::default(),
                kind: ContractKind::Market,
            })
            .await
            .with_context(|| format!("list markets deployed from {registry_id}"))?;
        markets.extend(deployed.account_ids);
    }
    Ok(markets)
}

/// Accumulate (where the market version requires it) and withdraw `signer`'s
/// static yield on one market.
async fn harvest_market(
    ctx: &CliContext,
    client: &Client,
    signer: &ManagedAccountId,
    market_id: &AccountId,
) -> anyhow::Result<MarketOutcome> {
    let (version, configuration) = tokio::try_join!(
        async {
            client
                .read(contract::GetVersion {
                    contract_id: market_id.clone(),
                })
                .await
                .context("read market version")
        },
        async {
            client
                .read(market::GetConfiguration {
                    market_id: market_id.clone(),
                })
                .await
                .context("read market configuration")
        },
    )?;
    let version = version
        .parsed
        .map(|version| version.cast::<Market>())
        .with_context(|| format!("unparseable market version {:?}", version.version_string))?;

    // Accumulation panics for an account without a static weight, but protocol
    // fees credit `protocol_account_id`'s record directly, so read it regardless.
    let has_static_weight = configuration.yield_weights.r#static.contains_key(&signer.0);
    let is_recipient = has_static_weight || configuration.protocol_account_id == signer.0;

    if has_static_weight && version.requires_static_yield_accumulation() {
        let result = client
            .execute_as(
                signer.clone(),
                market::AccumulateStaticYield {
                    market_id: market_id.clone(),
                    account_id: None,
                    snapshot_limit: None,
                },
            )
            .await?;
        ctx.report_checked(&result)
            .context("accumulate static yield")?;
    }

    let amount = client
        .read(market::GetStaticYield {
            market_id: market_id.clone(),
            account_id: signer.0.clone(),
        })
        .await
        .context("read static yield")?
        .borrow_asset_total();
    if amount.is_zero() {
        return Ok(if is_recipient {
            MarketOutcome::NothingAccumulated
        } else {
            MarketOutcome::NotARecipient
        });
    }

    ensure_storage_registered(client, configuration.borrow_asset.contract_id(), signer).await?;

    // Exactly the amount read, so the forwarded total cannot exceed what arrived.
    let result = client
        .execute_as(
            signer.clone(),
            market::WithdrawStaticYield {
                market_id: market_id.clone(),
                amount: Some(amount),
            },
        )
        .await?;
    ctx.report_checked(&result)
        .context("withdraw static yield")?;
    ensure_delivered(&result)?;
    tracing::info!(%market_id, %amount, "withdrew static yield");

    Ok(MarketOutcome::Withdrawn(Withdrawal {
        asset: configuration.borrow_asset,
        amount,
        decimals: u32::try_from(
            configuration
                .price_oracle_configuration
                .borrow_asset_decimals,
        )
        .ok(),
        operation_id: result.operation.id,
    }))
}

/// A withdrawal pays out through a token transfer whose failure the market's
/// callback absorbs by restoring the yield record, leaving the operation
/// `Succeeded` with nothing delivered.
fn ensure_delivered(result: &WriteOperationResult) -> anyhow::Result<()> {
    let failed = failed_receipt_contracts(result);
    if failed.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "withdrawal transfer failed (receipts on {}); the yield stays on the market",
            failed.join(", ")
        )
    }
}

/// Fail early when `signer` cannot receive the borrow asset. A token without
/// NEP-145 storage bounds has no requirement to check.
async fn ensure_storage_registered(
    client: &Client,
    asset_contract: &AccountIdRef,
    signer: &ManagedAccountId,
) -> anyhow::Result<()> {
    let Ok(bounds) = client
        .read(storage::GetBalanceBounds {
            contract_id: asset_contract.to_owned(),
        })
        .await
    else {
        return Ok(());
    };
    let total = client
        .read(storage::GetBalanceOf {
            contract_id: asset_contract.to_owned(),
            account_id: signer.0.clone(),
        })
        .await
        .with_context(|| format!("read storage balance on {asset_contract}"))?
        .balance
        .map_or(NearToken::from_yoctonear(0), |balance| balance.total);
    anyhow::ensure!(
        total >= bounds.bounds.min,
        "insufficient storage deposit on {asset_contract}: {total} < minimum {}",
        bounds.bounds.min
    );
    Ok(())
}

async fn forward(
    ctx: &CliContext,
    client: &Client,
    signer: &ManagedAccountId,
    receiver_id: &AccountId,
    harvested: &[Harvested],
) -> Vec<TransferReport> {
    let mut transfers = Vec::with_capacity(harvested.len());
    for Harvested { asset, amount, .. } in harvested {
        let outcome = match transfer(ctx, client, signer, receiver_id, asset, *amount).await {
            Ok(operation_id) => TransferOutcome::Sent { operation_id },
            Err(error) => {
                let error = format!("{error:#}");
                tracing::error!(%asset, %amount, %receiver_id, %error, "failed to forward yield");
                TransferOutcome::Failed { error }
            }
        };
        transfers.push(TransferReport {
            asset: asset.clone(),
            amount: *amount,
            outcome,
        });
    }
    transfers
}

async fn transfer(
    ctx: &CliContext,
    client: &Client,
    signer: &ManagedAccountId,
    receiver_id: &AccountId,
    asset: &FungibleAsset<BorrowAsset>,
    amount: BorrowAssetAmount,
) -> anyhow::Result<OperationId> {
    let result = client
        .execute_as(
            signer.clone(),
            token::Transfer {
                token: token::TokenReference::from(asset),
                receiver_id: receiver_id.clone(),
                amount: u128::from(amount).into(),
                memo: None,
            },
        )
        .await?;
    ctx.report_checked(&result)?;
    Ok(result.operation.id)
}

/// Withdrawals summed per asset, so each asset is forwarded in one transfer.
/// Decimals are dropped when the markets sharing an asset disagree on them.
fn totals(markets: &[MarketReport]) -> Vec<Harvested> {
    markets
        .iter()
        .filter_map(|market| match &market.outcome {
            MarketOutcome::Withdrawn(withdrawal) => Some(withdrawal),
            MarketOutcome::NothingAccumulated
            | MarketOutcome::NotARecipient
            | MarketOutcome::Failed { .. } => None,
        })
        .fold(
            BTreeMap::<_, (BorrowAssetAmount, Option<u32>)>::new(),
            |mut totals, withdrawal| {
                totals
                    .entry(withdrawal.asset.clone())
                    .and_modify(|(amount, decimals)| {
                        *amount += withdrawal.amount;
                        if *decimals != withdrawal.decimals {
                            *decimals = None;
                        }
                    })
                    .or_insert((withdrawal.amount, withdrawal.decimals));
                totals
            },
        )
        .into_iter()
        .map(|(asset, (amount, decimals))| Harvested {
            asset,
            amount,
            decimals,
        })
        .collect()
}

/// `amount` in whole units, e.g. `1234.5` for 1234500000 at 6 decimals.
fn format_units(amount: u128, decimals: u32) -> String {
    let Some(scale) = 10u128.checked_pow(decimals) else {
        return amount.to_string();
    };
    let whole = amount / scale;
    let fraction = amount % scale;
    if fraction == 0 {
        return whole.to_string();
    }
    let width = decimals as usize;
    let fraction = format!("{fraction:0width$}");
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

#[derive(Debug, Serialize)]
struct HarvestReport {
    markets: Vec<MarketReport>,
    harvested: Vec<Harvested>,
    transfers: Vec<TransferReport>,
}

impl HarvestReport {
    fn summary(&self) -> Summary<'_> {
        Summary(&self.harvested)
    }

    fn ensure_succeeded(&self) -> anyhow::Result<()> {
        let failed_markets = self
            .markets
            .iter()
            .filter(|market| matches!(market.outcome, MarketOutcome::Failed { .. }))
            .count();
        let failed_transfers = self
            .transfers
            .iter()
            .filter(|transfer| matches!(transfer.outcome, TransferOutcome::Failed { .. }))
            .count();
        anyhow::ensure!(
            failed_markets == 0 && failed_transfers == 0,
            "{failed_markets} market(s) and {failed_transfers} transfer(s) failed"
        );
        Ok(())
    }
}

/// The human-readable revenue line printed after the JSON report.
struct Summary<'a>(&'a [Harvested]);

impl fmt::Display for Summary<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return writeln!(f, "Harvested nothing.");
        }
        writeln!(f, "Harvested:")?;
        for harvested in self.0 {
            let amount = u128::from(harvested.amount);
            match harvested.decimals {
                Some(decimals) => write!(f, "  {}", format_units(amount, decimals))?,
                None => write!(f, "  {amount} (raw units)")?,
            }
            writeln!(f, " {}", harvested.asset)?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct MarketReport {
    market_id: AccountId,
    #[serde(flatten)]
    outcome: MarketOutcome,
}

#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum MarketOutcome {
    Withdrawn(Withdrawal),
    NothingAccumulated,
    /// The signer neither has a static weight nor is the protocol account.
    NotARecipient,
    Failed {
        error: String,
    },
}

#[derive(Debug, Serialize)]
struct Withdrawal {
    asset: FungibleAsset<BorrowAsset>,
    amount: BorrowAssetAmount,
    /// The market's configured borrow-asset decimals; only used for display.
    #[serde(skip)]
    decimals: Option<u32>,
    operation_id: OperationId,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Harvested {
    asset: FungibleAsset<BorrowAsset>,
    amount: BorrowAssetAmount,
    #[serde(skip_serializing_if = "Option::is_none")]
    decimals: Option<u32>,
}

#[derive(Debug, Serialize)]
struct TransferReport {
    asset: FungibleAsset<BorrowAsset>,
    amount: BorrowAssetAmount,
    #[serde(flatten)]
    outcome: TransferOutcome,
}

#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum TransferOutcome {
    Sent { operation_id: OperationId },
    Failed { error: String },
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    fn usdc() -> FungibleAsset<BorrowAsset> {
        FungibleAsset::nep141("usdc.testnet".parse().unwrap())
    }

    fn usdt() -> FungibleAsset<BorrowAsset> {
        FungibleAsset::nep141("usdt.testnet".parse().unwrap())
    }

    fn market(id: &str, outcome: MarketOutcome) -> MarketReport {
        MarketReport {
            market_id: id.parse().unwrap(),
            outcome,
        }
    }

    fn withdrawn(asset: FungibleAsset<BorrowAsset>, amount: u128, decimals: u32) -> MarketOutcome {
        MarketOutcome::Withdrawn(Withdrawal {
            asset,
            amount: amount.into(),
            decimals: Some(decimals),
            operation_id: OperationId("op".to_owned()),
        })
    }

    fn harvested(
        asset: FungibleAsset<BorrowAsset>,
        amount: u128,
        decimals: Option<u32>,
    ) -> Harvested {
        Harvested {
            asset,
            amount: amount.into(),
            decimals,
        }
    }

    fn failed() -> MarketOutcome {
        MarketOutcome::Failed {
            error: "boom".to_owned(),
        }
    }

    #[test]
    fn totals_sum_withdrawals_per_asset_and_ignore_the_rest() {
        let markets = [
            market("a.testnet", withdrawn(usdc(), 10, 6)),
            market("b.testnet", withdrawn(usdt(), 7, 6)),
            market("c.testnet", withdrawn(usdc(), 5, 6)),
            market("d.testnet", MarketOutcome::NothingAccumulated),
            market("e.testnet", MarketOutcome::NotARecipient),
            market("f.testnet", failed()),
        ];
        assert_eq!(
            totals(&markets),
            [
                harvested(usdc(), 15, Some(6)),
                harvested(usdt(), 7, Some(6))
            ]
        );
    }

    #[test]
    fn totals_drop_decimals_the_markets_disagree_on() {
        let markets = [
            market("a.testnet", withdrawn(usdc(), 10, 6)),
            market("b.testnet", withdrawn(usdc(), 5, 18)),
        ];
        assert_eq!(totals(&markets), [harvested(usdc(), 15, None)]);
    }

    #[rstest]
    #[case::whole(1_000_000, 6, "1")]
    #[case::fraction_trimmed(1_234_500_000, 6, "1234.5")]
    #[case::below_one(1, 6, "0.000001")]
    #[case::zero(0, 6, "0")]
    #[case::no_decimals(42, 0, "42")]
    #[case::scale_overflows(42, 39, "42")]
    fn formats_whole_units(#[case] amount: u128, #[case] decimals: u32, #[case] expected: &str) {
        assert_eq!(format_units(amount, decimals), expected);
    }

    #[rstest]
    #[case::nothing(vec![], "Harvested nothing.\n")]
    #[case::scaled_and_raw(
        vec![harvested(usdc(), 1_500_000, Some(6)), harvested(usdt(), 7, None)],
        "Harvested:\n  1.5 nep141:usdc.testnet\n  7 (raw units) nep141:usdt.testnet\n"
    )]
    fn summarizes_harvested_quantities(#[case] harvested: Vec<Harvested>, #[case] expected: &str) {
        let report = HarvestReport {
            markets: Vec::new(),
            harvested,
            transfers: Vec::new(),
        };
        assert_eq!(report.summary().to_string(), expected);
    }

    #[rstest]
    #[case::all_ok(withdrawn(usdc(), 1, 6), None, true)]
    #[case::not_a_recipient(MarketOutcome::NotARecipient, None, true)]
    #[case::market_failed(failed(), None, false)]
    #[case::transfer_failed(
        withdrawn(usdc(), 1, 6),
        Some(TransferOutcome::Failed { error: "boom".to_owned() }),
        false
    )]
    fn report_fails_on_any_failure(
        #[case] market_outcome: MarketOutcome,
        #[case] transfer_outcome: Option<TransferOutcome>,
        #[case] succeeds: bool,
    ) {
        let markets = vec![market("a.testnet", market_outcome)];
        let report = HarvestReport {
            harvested: totals(&markets),
            markets,
            transfers: transfer_outcome
                .into_iter()
                .map(|outcome| TransferReport {
                    asset: usdc(),
                    amount: 1.into(),
                    outcome,
                })
                .collect(),
        };
        assert_eq!(report.ensure_succeeded().is_ok(), succeeds);
    }

    #[test]
    fn report_serializes_flat_tagged_outcomes() {
        let markets = vec![
            market("a.testnet", withdrawn(usdc(), 10, 6)),
            market("b.testnet", MarketOutcome::NothingAccumulated),
            market("c.testnet", MarketOutcome::NotARecipient),
            market("d.testnet", failed()),
        ];
        let report = HarvestReport {
            harvested: totals(&markets),
            markets,
            transfers: vec![TransferReport {
                asset: usdc(),
                amount: 10.into(),
                outcome: TransferOutcome::Sent {
                    operation_id: OperationId("op".to_owned()),
                },
            }],
        };
        let usdc = serde_json::to_value(usdc()).unwrap();
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            json!({
                "markets": [
                    {
                        "market_id": "a.testnet",
                        "outcome": "withdrawn",
                        "asset": usdc,
                        "amount": "10",
                        "operation_id": "op",
                    },
                    { "market_id": "b.testnet", "outcome": "nothing_accumulated" },
                    { "market_id": "c.testnet", "outcome": "not_a_recipient" },
                    { "market_id": "d.testnet", "outcome": "failed", "error": "boom" },
                ],
                "harvested": [{ "asset": usdc, "amount": "10", "decimals": 6 }],
                "transfers": [
                    {
                        "asset": usdc,
                        "amount": "10",
                        "outcome": "sent",
                        "operation_id": "op",
                    },
                ],
            })
        );
    }
}
