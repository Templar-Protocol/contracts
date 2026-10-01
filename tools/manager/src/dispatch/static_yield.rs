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
    common::Pagination, ContractKind, ManagedAccountId, Market, NearToken, OperationId,
};

use crate::commands::market::static_yield::Harvest;
use crate::context::{failed_receipt_contracts, print_json, CliContext};
use crate::spec::amount::Amount;

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

    let mut totals = totals(&markets);
    if let Some(receiver_id) = &args.receiver_id {
        for total in &mut totals {
            total.forwarded = Some(forward(&ctx, &client, &signer, receiver_id, total).await);
        }
    }

    let report = HarvestReport { markets, totals };
    print_json(&report)?;
    eprint!("{}", Summary(&report.totals));
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

async fn harvest_market(
    ctx: &CliContext,
    client: &Client,
    signer: &ManagedAccountId,
    market_id: &AccountId,
) -> anyhow::Result<MarketOutcome> {
    let configuration = client
        .read(market::GetConfiguration {
            market_id: market_id.clone(),
        })
        .await
        .context("read market configuration")?;

    // Accumulation panics for an account without a static weight, but protocol
    // fees credit `protocol_account_id`'s record directly, so read it regardless.
    let has_static_weight = configuration.yield_weights.r#static.contains_key(&signer.0);
    if has_static_weight {
        let version = client
            .read(contract::GetVersion {
                contract_id: market_id.clone(),
            })
            .await
            .context("read market version")?;
        let version = version
            .parsed
            .map(|version| version.cast::<Market>())
            .with_context(|| format!("unparseable market version {:?}", version.version_string))?;
        if version.requires_static_yield_accumulation() {
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
        let is_recipient = has_static_weight || configuration.protocol_account_id == signer.0;
        return Ok(if is_recipient {
            MarketOutcome::NothingToWithdraw
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
    // The market's callback absorbs a failed payout by restoring the yield, so
    // the operation still reports success.
    let failed = failed_receipt_contracts(&result);
    anyhow::ensure!(
        failed.is_empty(),
        "payout failed on {}, so the yield stays on the market",
        failed.join(", ")
    );
    tracing::info!(%market_id, %amount, "withdrew static yield");

    Ok(MarketOutcome::Withdrawn(Withdrawal {
        asset: configuration.borrow_asset,
        amount,
        decimals: u8::try_from(
            configuration
                .price_oracle_configuration
                .borrow_asset_decimals,
        )
        .ok(),
        operation_id: result.operation.id,
    }))
}

/// Fail before withdrawing when `signer` cannot receive the borrow asset. A
/// token without NEP-145 storage bounds has no requirement to check.
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
    total: &Total,
) -> Forwarded {
    let sent = async {
        let result = client
            .execute_as(
                signer.clone(),
                token::Transfer {
                    token: token::TokenReference::from(&total.asset),
                    receiver_id: receiver_id.clone(),
                    amount: u128::from(total.amount).into(),
                    memo: None,
                },
            )
            .await?;
        ctx.report_checked(&result)?;
        anyhow::Ok(result.operation.id)
    };
    match sent.await {
        Ok(operation_id) => Forwarded::Sent { operation_id },
        Err(error) => {
            let error = format!("{error:#}");
            tracing::error!(asset = %total.asset, %receiver_id, %error, "failed to forward yield");
            Forwarded::Failed { error }
        }
    }
}

/// Withdrawals summed per asset, so each asset is forwarded in one transfer.
fn totals(markets: &[MarketReport]) -> Vec<Total> {
    let mut totals = BTreeMap::new();
    for market in markets {
        if let MarketOutcome::Withdrawn(withdrawal) = &market.outcome {
            totals
                .entry(&withdrawal.asset)
                .or_insert_with(|| Total {
                    asset: withdrawal.asset.clone(),
                    amount: BorrowAssetAmount::zero(),
                    decimals: withdrawal.decimals,
                    forwarded: None,
                })
                .amount += withdrawal.amount;
        }
    }
    totals.into_values().collect()
}

#[derive(Debug, Serialize)]
struct HarvestReport {
    markets: Vec<MarketReport>,
    totals: Vec<Total>,
}

impl HarvestReport {
    fn ensure_succeeded(&self) -> anyhow::Result<()> {
        let failed = self
            .markets
            .iter()
            .any(|market| matches!(market.outcome, MarketOutcome::Failed { .. }))
            || self
                .totals
                .iter()
                .any(|total| matches!(total.forwarded, Some(Forwarded::Failed { .. })));
        anyhow::ensure!(!failed, "some markets or transfers failed; see the report");
        Ok(())
    }
}

struct Summary<'a>(&'a [Total]);

impl fmt::Display for Summary<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return writeln!(f, "Harvested nothing.");
        }
        writeln!(f, "Harvested:")?;
        for total in self.0 {
            let raw = u128::from(total.amount);
            let amount = total.decimals.map_or(Amount::Atoms(raw), |decimals| {
                Amount::from_base_units(raw, decimals)
            });
            writeln!(f, "  {}: {amount}", total.asset)?;
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
    NothingToWithdraw,
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
    /// The market's configured borrow-asset decimals, for the summary only.
    #[serde(skip)]
    decimals: Option<u8>,
    operation_id: OperationId,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Total {
    asset: FungibleAsset<BorrowAsset>,
    amount: BorrowAssetAmount,
    #[serde(skip)]
    decimals: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    forwarded: Option<Forwarded>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum Forwarded {
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

    fn withdrawn(asset: FungibleAsset<BorrowAsset>, amount: u128) -> MarketOutcome {
        MarketOutcome::Withdrawn(Withdrawal {
            asset,
            amount: amount.into(),
            decimals: Some(6),
            operation_id: OperationId("op".to_owned()),
        })
    }

    fn total(asset: FungibleAsset<BorrowAsset>, amount: u128, decimals: Option<u8>) -> Total {
        Total {
            asset,
            amount: amount.into(),
            decimals,
            forwarded: None,
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
            market("a.testnet", withdrawn(usdc(), 10)),
            market("b.testnet", withdrawn(usdt(), 7)),
            market("c.testnet", withdrawn(usdc(), 5)),
            market("d.testnet", MarketOutcome::NothingToWithdraw),
            market("e.testnet", MarketOutcome::NotARecipient),
            market("f.testnet", failed()),
        ];
        assert_eq!(
            totals(&markets),
            [total(usdc(), 15, Some(6)), total(usdt(), 7, Some(6))]
        );
    }

    #[rstest]
    #[case::nothing(vec![], "Harvested nothing.\n")]
    #[case::scaled_and_raw(
        vec![total(usdc(), 1_500_000, Some(6)), total(usdt(), 7, None)],
        "Harvested:\n  nep141:usdc.testnet: 1.5 tokens\n  nep141:usdt.testnet: 7 atoms\n"
    )]
    fn summarizes_totals(#[case] totals: Vec<Total>, #[case] expected: &str) {
        assert_eq!(Summary(&totals).to_string(), expected);
    }

    #[rstest]
    #[case::all_ok(withdrawn(usdc(), 1), None, true)]
    #[case::not_a_recipient(MarketOutcome::NotARecipient, None, true)]
    #[case::market_failed(failed(), None, false)]
    #[case::transfer_failed(
        withdrawn(usdc(), 1),
        Some(Forwarded::Failed { error: "boom".to_owned() }),
        false
    )]
    fn report_fails_on_any_failure(
        #[case] outcome: MarketOutcome,
        #[case] forwarded: Option<Forwarded>,
        #[case] succeeds: bool,
    ) {
        let markets = vec![market("a.testnet", outcome)];
        let mut totals = totals(&markets);
        if let Some(total) = totals.first_mut() {
            total.forwarded = forwarded;
        }
        let report = HarvestReport { markets, totals };
        assert_eq!(report.ensure_succeeded().is_ok(), succeeds);
    }

    #[test]
    fn report_serializes_flat_tagged_outcomes() {
        let report = HarvestReport {
            markets: vec![
                market("a.testnet", withdrawn(usdc(), 10)),
                market("b.testnet", MarketOutcome::NothingToWithdraw),
                market("c.testnet", MarketOutcome::NotARecipient),
                market("d.testnet", failed()),
            ],
            totals: vec![Total {
                forwarded: Some(Forwarded::Sent {
                    operation_id: OperationId("op".to_owned()),
                }),
                ..total(usdc(), 10, Some(6))
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
                    { "market_id": "b.testnet", "outcome": "nothing_to_withdraw" },
                    { "market_id": "c.testnet", "outcome": "not_a_recipient" },
                    { "market_id": "d.testnet", "outcome": "failed", "error": "boom" },
                ],
                "totals": [{
                    "asset": usdc,
                    "amount": "10",
                    "forwarded": { "outcome": "sent", "operation_id": "op" },
                }],
            })
        );
    }
}
