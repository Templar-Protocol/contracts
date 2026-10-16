//! Fresh indicative USD prices for borrowed and active supplied principal.
//! Provider payloads are fetched once per source, then verified separately by each
//! adapter. Nothing is submitted and stored prices are never used as a fallback.

use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, ensure, Context as _};
use async_trait::async_trait;
use futures::{stream, StreamExt as _};
use near_account_id::AccountId;
use templar_common::{
    market::PriceOracleConfiguration,
    oracle::{lazer, pyth, redstone},
    Decimal, Nanoseconds,
};
use templar_gateway_core::{
    DispatchRead, GatewayContext, HasNearClient, NearClient, OraclePayloadSource,
};
use templar_gateway_methods_spec::oracle::{
    GetPriceResolutionDependencies, LazerOraclePrices, LazerPriceEntry, RedStoneOraclePrices,
    RedStonePriceEntry, ResolvePrice,
};
use templar_gateway_oracle_updates_dispatch::{
    Dispatch as OracleUpdatesDispatch, OracleSourceArgs, ProvidesLazerSource,
    ProvidesRedStoneSource,
};
use templar_gateway_oracle_updates_spec::oracle::{GetLazerUpdate, GetRedStoneUpdate};
use templar_proxy_oracle_near_common::request::OracleRequest;

use crate::context::{self, CliContext};

const CONCURRENCY: usize = 8;
type PriceKey = (AccountId, pyth::PriceIdentifier);
type Inputs = BTreeMap<OracleRequest, Result<Feed, String>>;

/// Numeric invariants established when a fresh source or resolved price is validated.
#[derive(Clone, Copy)]
pub(crate) struct ValuationPrice {
    mantissa: u64,
    exponent: i32,
}

#[derive(Clone)]
enum Feed {
    Lazer(lazer::FeedData),
    RedStone(redstone::FeedData),
}

/// Reuse one provider payload across adapter verification calls, including when
/// several adapters consume the same feed. Each call requests a subset of the
/// union fetched from the provider; the adapters still verify that exact payload.
#[derive(Clone)]
struct Preview<Id> {
    base: GatewayContext,
    payload: Arc<Vec<u8>>,
    id: PhantomData<Id>,
}

impl<Id: Clone + Send + Sync + 'static> HasNearClient for Preview<Id> {
    fn near_client(&self) -> &NearClient {
        self.base.near_client()
    }
}

#[async_trait]
impl<Id: Send + Sync> OraclePayloadSource for Preview<Id> {
    type PriceId = Id;
    type Error = std::io::Error;

    async fn fetch_payload(&self, _: &[Id]) -> Result<Vec<u8>, Self::Error> {
        // The gateway's source interface requires an owned payload.
        Ok(self.payload.as_ref().clone())
    }
}

impl ProvidesLazerSource for Preview<u32> {
    type LazerSource = Self;
    fn lazer_source(&self) -> &Self {
        self
    }
}

impl ProvidesRedStoneSource for Preview<redstone::FeedId> {
    type RedStoneSource = Self;
    fn redstone_source(&self) -> &Self {
        self
    }
}

/// One result per config, for its borrow asset; supplied principal uses that
/// same asset. A missing/failed dependency invalidates the entire valuation,
/// even when a proxy aggregator could otherwise accept a partial source set.
pub(crate) async fn fresh_prices(
    ctx: &CliContext,
    args: &OracleSourceArgs,
    configs: &[PriceOracleConfiguration],
) -> Vec<Result<ValuationPrice, String>> {
    let dependencies = load_dependencies(ctx, configs).await;

    let mut lazer_groups: BTreeMap<AccountId, BTreeSet<u32>> = BTreeMap::new();
    let mut redstone_groups: BTreeMap<AccountId, BTreeSet<redstone::FeedId>> = BTreeMap::new();
    for requests in dependencies
        .values()
        .filter_map(|result| result.as_ref().ok())
    {
        for request in requests {
            match request {
                OracleRequest::Lazer(request) => {
                    lazer_groups
                        .entry(request.oracle_id.clone())
                        .or_default()
                        .insert(request.feed_id);
                }
                OracleRequest::RedStone(request) => {
                    redstone_groups
                        .entry(request.oracle_id.clone())
                        .or_default()
                        .insert(request.price_id.clone());
                }
                OracleRequest::Pyth(_) => unreachable!("unsupported dependencies rejected above"),
            }
        }
    }
    let mut inputs = Inputs::new();
    // Do not build unused sources: RedStone-only scans need no Lazer token.
    if !lazer_groups.is_empty() {
        load_lazer(ctx, args, lazer_groups, &mut inputs).await;
    }
    if !redstone_groups.is_empty() {
        load_redstone(ctx, args, redstone_groups, &mut inputs).await;
    }

    let resolutions: BTreeSet<_> = configs
        .iter()
        .map(|config| {
            (
                config.account_id.clone(),
                config.borrow_asset_price_id,
                u64::from(config.price_maximum_age_s),
            )
        })
        .collect();
    let resolved: BTreeMap<_, _> = stream::iter(resolutions)
        .map(|(oracle_id, price_id, age)| {
            let dependencies = &dependencies;
            let inputs = &inputs;
            async move {
                let result = async {
                    let requests = dependencies
                        .get(&(oracle_id.clone(), price_id))
                        .ok_or_else(|| "missing price dependency request".to_owned())?
                        .as_ref()
                        .map_err(Clone::clone)?;
                    let request = resolution(oracle_id.clone(), price_id, age, requests, inputs)
                        .map_err(|error| format!("{error:#}"))?;
                    let price = ctx
                        .client
                        .read(request)
                        .await
                        .map_err(|error| format!("resolve fresh price on {oracle_id}: {error}"))?
                        .ok_or_else(|| {
                            format!("fresh price unresolved on {oracle_id} (maximum age {age}s)")
                        })?;
                    validate_price(&price, age).map_err(|error| format!("{error:#}"))
                }
                .await;
                ((oracle_id, price_id, age), result)
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    configs
        .iter()
        .map(|config| {
            resolved
                .get(&(
                    config.account_id.clone(),
                    config.borrow_asset_price_id,
                    u64::from(config.price_maximum_age_s),
                ))
                .cloned()
                .unwrap_or_else(|| Err("missing fresh price resolution".to_owned()))
        })
        .collect()
}

async fn load_dependencies(
    ctx: &CliContext,
    configs: &[PriceOracleConfiguration],
) -> BTreeMap<PriceKey, Result<BTreeSet<OracleRequest>, String>> {
    let keys = configs
        .iter()
        .map(|config| (config.account_id.clone(), config.borrow_asset_price_id))
        .collect::<BTreeSet<_>>();
    stream::iter(keys)
        .map(|(oracle_id, price_id)| async move {
            let result = ctx.client.read(GetPriceResolutionDependencies {
                oracle_id: oracle_id.clone(),
                price_id,
            }).await.map_err(|error| format!("discover fresh price dependencies on {oracle_id}: {error}"))
                .and_then(|result| {
                    let requests: BTreeSet<_> = result.requests.into_iter().collect();
                    if let Some(OracleRequest::Pyth(request)) = requests.iter().find(|request| matches!(request, OracleRequest::Pyth(_))) {
                        return Err(format!("fresh Pyth price unavailable: gateway has no typed provider-backed verified Pyth preview for {} feed {}; direct/Pyth/LST dependencies cannot use stored prices", request.oracle_id, hex::encode(request.price_id.0)));
                    }
                    if requests.is_empty() {
                        return Err(format!("no fresh price dependencies returned by {oracle_id}"));
                    }
                    Ok(requests)
                });
            ((oracle_id, price_id), result)
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await
}

async fn load_lazer(
    ctx: &CliContext,
    args: &OracleSourceArgs,
    groups: BTreeMap<AccountId, BTreeSet<u32>>,
    inputs: &mut Inputs,
) {
    let feed_ids: Vec<_> = groups
        .values()
        .flatten()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let source = async {
        let base = GatewayContext::new(ctx.network_config().clone())?;
        let source = context::lazer_source(base.clone(), &args.lazer)?;
        let payload = source.lazer_source().fetch_payload(&feed_ids).await?;
        Ok::<_, anyhow::Error>(Preview::<u32> {
            base,
            payload: Arc::new(payload),
            id: PhantomData,
        })
    }
    .await
    .map_err(|error| format!("fetch fresh Lazer payload: {error:#}"));
    let mut rows = stream::iter(groups)
        .map(|(oracle_id, feeds)| {
            let source = &source;
            async move {
                let result = match source {
                    Ok(source) => <OracleUpdatesDispatch as DispatchRead<
                        GetLazerUpdate,
                        Preview<u32>,
                    >>::dispatch(
                        GetLazerUpdate {
                            oracle_id: oracle_id.clone(),
                            feed_ids: feeds.iter().copied().collect(),
                        },
                        source.clone(),
                    )
                    .await
                    .map_err(|error| format!("verify fresh Lazer payload on {oracle_id}: {error}")),
                    Err(error) => Err(error.clone()),
                };
                (oracle_id, feeds, result)
            }
        })
        .buffer_unordered(CONCURRENCY);
    while let Some((oracle_id, feeds, result)) = rows.next().await {
        for feed_id in feeds {
            let value = result.as_ref().map_err(Clone::clone).and_then(|response| {
                response
                    .get(&feed_id)
                    .and_then(Option::as_ref)
                    .cloned()
                    .map(Feed::Lazer)
                    .ok_or_else(|| {
                        format!(
                            "verified Lazer payload missing usable feed {feed_id} on {oracle_id}"
                        )
                    })
            });
            inputs.insert(OracleRequest::lazer(oracle_id.clone(), feed_id), value);
        }
    }
}

async fn load_redstone(
    ctx: &CliContext,
    args: &OracleSourceArgs,
    groups: BTreeMap<AccountId, BTreeSet<redstone::FeedId>>,
    inputs: &mut Inputs,
) {
    let feed_ids: Vec<_> = groups
        .values()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let source = async {
        let base = GatewayContext::new(ctx.network_config().clone())?;
        let source = context::redstone_source(base.clone(), &args.redstone)?;
        let payload = source.redstone_source().fetch_payload(&feed_ids).await?;
        Ok::<_, anyhow::Error>(Preview::<redstone::FeedId> {
            base,
            payload: Arc::new(payload),
            id: PhantomData,
        })
    }
    .await
    .map_err(|error| format!("fetch fresh RedStone payload: {error:#}"));
    let mut rows = stream::iter(groups)
        .map(|(oracle_id, feeds)| {
            let source = &source;
            async move {
                let result = match source {
                    Ok(source) => <OracleUpdatesDispatch as DispatchRead<
                        GetRedStoneUpdate,
                        Preview<redstone::FeedId>,
                    >>::dispatch(
                        GetRedStoneUpdate {
                            oracle_id: oracle_id.clone(),
                            feed_ids: feeds.iter().cloned().collect(),
                        },
                        source.clone(),
                    )
                    .await
                    .map_err(|error| {
                        format!("verify fresh RedStone payload on {oracle_id}: {error}")
                    }),
                    Err(error) => Err(error.clone()),
                };
                (oracle_id, feeds, result)
            }
        })
        .buffer_unordered(CONCURRENCY);
    while let Some((oracle_id, feeds, result)) = rows.next().await {
        for feed_id in feeds {
            let value = result.as_ref().map_err(Clone::clone).and_then(|response| {
                response
                    .iter()
                    .find(|entry| entry.feed_id == feed_id)
                    .map(|entry| Feed::RedStone(entry.data.clone()))
                    .ok_or_else(|| {
                        format!("verified RedStone payload missing feed {feed_id} on {oracle_id}")
                    })
            });
            inputs.insert(OracleRequest::redstone(oracle_id.clone(), feed_id), value);
        }
    }
}

fn resolution(
    oracle_id: AccountId,
    price_id: pyth::PriceIdentifier,
    age: u64,
    requests: &BTreeSet<OracleRequest>,
    inputs: &Inputs,
) -> anyhow::Result<ResolvePrice> {
    let mut lazer: BTreeMap<AccountId, Vec<LazerPriceEntry>> = BTreeMap::new();
    let mut redstone: BTreeMap<AccountId, Vec<RedStonePriceEntry>> = BTreeMap::new();
    for request in requests {
        let input = inputs
            .get(request)
            .ok_or_else(|| anyhow!("missing fresh source input: {request:?}"))?
            .as_ref()
            .map_err(|error| anyhow!("{error}"))?;
        let price = match (request, input) {
            (OracleRequest::Lazer(request), Feed::Lazer(data)) => {
                lazer
                    .entry(request.oracle_id.clone())
                    .or_default()
                    .push(LazerPriceEntry {
                        feed_id: request.feed_id,
                        data: data.clone(),
                    });
                data.to_ema_price()
            }
            (OracleRequest::RedStone(request), Feed::RedStone(data)) => {
                redstone
                    .entry(request.oracle_id.clone())
                    .or_default()
                    .push(RedStonePriceEntry {
                        feed_id: request.price_id.clone(),
                        data: data.clone(),
                    });
                data.to_pyth_price()
            }
            _ => anyhow::bail!("unsupported or mismatched fresh source input: {request:?}"),
        }
        .ok_or_else(|| anyhow!("fresh source cannot project price: {request:?}"))?;
        validate_price(&price, age).with_context(|| format!("fresh source {request:?}"))?;
    }
    Ok(ResolvePrice {
        oracle_id,
        price_id,
        age,
        pyth: Vec::new(),
        lazer: lazer
            .into_iter()
            .map(|(oracle_id, response)| LazerOraclePrices {
                oracle_id,
                response,
            })
            .collect(),
        redstone: redstone
            .into_iter()
            .map(|(oracle_id, response)| RedStoneOraclePrices {
                oracle_id,
                response,
            })
            .collect(),
    })
}

fn validate_price(price: &pyth::Price, age: u64) -> anyhow::Result<ValuationPrice> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch")?;
    let now = Nanoseconds::from_ns(
        u64::try_from(now.as_nanos()).context("system clock timestamp overflow")?,
    );
    validate_price_at(price, age, now)
}

fn validate_price_at(
    price: &pyth::Price,
    age: u64,
    now: Nanoseconds,
) -> anyhow::Result<ValuationPrice> {
    ensure!(price.price.0 > 0, "USD price must be positive");
    ensure!(
        (-38..=115).contains(&price.expo),
        "price exponent must be between -38 and 115"
    );
    let published = price
        .publish_time
        .try_into_time()
        .context("invalid price timestamp")?;
    ensure!(
        now.saturating_sub(published) <= Nanoseconds::from_secs(age),
        "fresh source price exceeds maximum age {age}s"
    );
    Ok(ValuationPrice {
        mantissa: price.price.0.unsigned_abs(),
        exponent: price.expo,
    })
}

/// Token decimals are bounded by Decimal's 38 fractional decimal digits.
pub(crate) fn token_amount(amount: u128, decimals: i32) -> anyhow::Result<Decimal> {
    ensure!(
        (0..=38).contains(&decimals),
        "token decimals must be between 0 and 38"
    );
    scale(Decimal::from(amount), -decimals)
}

/// Value principal only, without accrued interest, fees, or collateral/TVL.
/// Decimal has a 128-bit binary fractional part; division truncates only at
/// that representation's precision, never to an integer token or USD amount.
pub(crate) fn principal_value(
    amount: u128,
    decimals: i32,
    price: &ValuationPrice,
) -> anyhow::Result<Decimal> {
    ensure!(
        (0..=38).contains(&decimals),
        "token decimals must be between 0 and 38"
    );
    // A u128 amount times a positive i64 mantissa fits in 191 whole bits;
    // Decimal provides 384. Integer multiplication is exact and cannot overflow.
    let value = Decimal::from(amount) * Decimal::from(price.mantissa);
    scale(value, price.exponent - decimals)
}

fn scale(value: Decimal, exponent: i32) -> anyhow::Result<Decimal> {
    value
        .mul_pow10(exponent)
        .context("principal value exceeds Decimal range or precision")
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::json_types::{I64, U64};
    use templar_common::oracle::pyth::PythTimestamp;

    fn raw_price(mantissa: i64, expo: i32) -> pyth::Price {
        pyth::Price {
            price: I64(mantissa),
            conf: U64(1),
            expo,
            publish_time: PythTimestamp::from_secs(1_800_000_000),
        }
    }

    fn price(mantissa: i64, expo: i32) -> ValuationPrice {
        validate_price_at(
            &raw_price(mantissa, expo),
            60,
            Nanoseconds::from_secs(1_800_000_000),
        )
        .unwrap()
    }

    #[test]
    fn token_scales_preserve_fractional_amounts() {
        for decimals in [6, 7, 8, 24] {
            let amount = 10u128.pow(decimals) + 10u128.pow(decimals - 1) * 5;
            let scale = i32::try_from(decimals).unwrap();
            assert_eq!(
                token_amount(amount, scale).unwrap(),
                Decimal::from(3u8) / 2u8
            );
            let smallest = token_amount(1, scale).unwrap();
            assert!(smallest > Decimal::ZERO);
            let expected: Decimal = format!("0.{}1", "0".repeat(decimals as usize - 1))
                .parse()
                .unwrap();
            assert!(smallest.near_equal(expected));
        }
    }

    #[test]
    fn exact_known_principal_valuation() {
        assert_eq!(
            principal_value(1_250_000, 6, &price(200_000_000, -8)).unwrap(),
            Decimal::from(5u8) / 2u8
        );
        assert_eq!(
            principal_value(25_000_000, 7, &price(125, 1)).unwrap(),
            Decimal::from(3_125u32)
        );
        assert_eq!(
            principal_value(250_000_000, 8, &price(12_345_000_000, -6)).unwrap(),
            Decimal::from(61_725u32) / 2u8
        );
        assert_eq!(
            principal_value(1_500_000_000_000_000_000_000_000, 24, &price(2_000_000, -6)).unwrap(),
            Decimal::from(3u8)
        );
    }

    #[test]
    fn multiply_before_scaling_preserves_tiny_tokens() {
        // Normalize once after integer multiplication, rather than amplifying
        // token normalization's binary rounding by a large price mantissa.
        let value = principal_value(1, 38, &price(1_000_000_000_000_000_000, -18)).unwrap();
        assert!(value > Decimal::ZERO);
        assert!(value.near_equal(token_amount(1, 38).unwrap()));
        let amplified = principal_value(1, 38, &price(1_000_000_000_000_000_000, 0)).unwrap();
        assert!(amplified.near_equal("0.00000000000000000001".parse().unwrap()));
        let large = principal_value(u128::MAX, 0, &price(i64::MAX, 0)).unwrap();
        assert!(large > Decimal::from(u128::MAX));
    }

    #[test]
    fn zero_principal_does_not_hide_invalid_prices_or_scales() {
        assert_eq!(principal_value(0, 6, &price(1, -8)).unwrap(), Decimal::ZERO);
        let now = Nanoseconds::from_secs(1_800_000_000);
        for mantissa in [0, -1, i64::MIN] {
            assert!(validate_price_at(&raw_price(mantissa, -8), 60, now).is_err());
        }
        for expo in [-39, 116, i32::MIN, i32::MAX] {
            assert!(validate_price_at(&raw_price(1, expo), 60, now).is_err());
        }
        for amount in [0, 1] {
            for decimals in [-1, 39, i32::MIN, i32::MAX] {
                assert!(token_amount(amount, decimals).is_err());
                assert!(principal_value(amount, decimals, &price(1, 0)).is_err());
            }
        }
    }

    #[test]
    fn overflow_and_underflow_are_errors() {
        assert!(principal_value(u128::MAX, 0, &price(i64::MAX, 115)).is_err());
        assert!(principal_value(1, 38, &price(1, -38)).is_err());
        assert_eq!(
            token_amount(u128::MAX, 0).unwrap(),
            Decimal::from(u128::MAX)
        );
    }

    #[test]
    fn freshness_preserves_each_configuration_age() {
        let mut price = raw_price(100, -2);
        price.publish_time = PythTimestamp::from_secs(50);
        let now = Nanoseconds::from_secs(100);
        assert!(validate_price_at(&price, 60, now).is_ok());
        assert!(validate_price_at(&price, 50, now).is_ok());
        assert!(validate_price_at(&price, 49, now).is_err());
        price.publish_time = PythTimestamp::from_secs(-1);
        assert!(validate_price_at(&price, 60, now).is_err());
    }

    #[test]
    fn missing_or_failed_source_prevents_partial_resolution() {
        let oracle_id: AccountId = "oracle.near".parse().unwrap();
        let request = OracleRequest::lazer(oracle_id.clone(), 7);
        let requests = BTreeSet::from([request.clone()]);
        assert!(resolution(
            oracle_id.clone(),
            pyth::PriceIdentifier([0; 32]),
            60,
            &requests,
            &Inputs::new()
        )
        .is_err());
        let inputs = Inputs::from([(request, Err("signature verification failed".to_owned()))]);
        let error = resolution(
            oracle_id,
            pyth::PriceIdentifier([0; 32]),
            60,
            &requests,
            &inputs,
        )
        .unwrap_err();
        assert!(error.to_string().contains("signature verification failed"));
    }
}
