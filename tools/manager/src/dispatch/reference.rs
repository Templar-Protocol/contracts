//! Cross-check the aggregated prices against an independent source.
//!
//! The oracles can agree with each other and still be wrong together — a
//! transposed feed id, a wrapped token that does not track what it claims to.
//! Only something outside the system catches that, which is what makes the
//! spec's `symbol` and `reference` load-bearing rather than decorative.
//!
//! The distinction that matters here is between *could not check* and *checked
//! and disagrees*. Unreachable, rate-limited, or unpinnable is a warning; a
//! price outside tolerance is a failure. Collapsing the two either makes a
//! mainnet deploy hostage to a third party's uptime, or lets a real mismatch
//! through as a shrug.

use std::collections::BTreeMap;

use anyhow::Context as _;
use serde::Deserialize;
use templar_common::asset::AssetClass;
use templar_common::Decimal;
use templar_proxy_oracle_kernel::Price;

use super::scaled;

use crate::spec::{
    check::{Check, Status},
    oracle::{AssetSpec, ReferenceAsset},
    MarketSpec,
};

/// One coin as the reference API describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub id: String,
    pub symbol: String,
    pub name: String,
}

/// Where reference prices come from.
///
/// A trait so the checks are testable without a network — every test drives a
/// fixture, never the live API — and so a second source can be added without
/// touching the checks.
#[async_trait::async_trait]
pub trait ReferencePriceSource {
    /// One coin by its id. `None` when no such coin exists.
    ///
    /// Separate from [`Self::candidates`] deliberately: a pinned id needs no
    /// resolution, and making every run pay for a full catalogue to learn one
    /// coin's name would bake one implementation's cheapest strategy into the
    /// interface.
    async fn lookup(&self, id: &str) -> anyhow::Result<Option<Listing>>;

    /// Every coin using `symbol`. The full set, because picking one of several
    /// would silently verify the wrong asset.
    async fn candidates(&self, symbol: &str) -> anyhow::Result<Vec<Listing>>;

    /// Spot USD prices for the given ids.
    async fn prices(&self, ids: &[String]) -> anyhow::Result<BTreeMap<String, f64>>;

    /// Names this source in reports.
    fn label(&self) -> &'static str;
}

/// What an asset resolved to, or the verdict explaining why it did not.
///
/// The `Err` carries a [`Status`], not a message, because the two failure modes
/// are not the same kind of thing. A source that cannot be reached is
/// `Skipped` — nothing was learned. A pinned id or ticker the source does not
/// know is `Failed`: the spec asserted that coin exists and the source just
/// disproved it. Collapsing them would let a fat-fingered id read exactly like
/// a network blip and exit zero.
type Resolved = Result<Listing, Status>;

pub struct CoinGecko {
    client: reqwest::Client,
    api_key: Option<String>,
}

impl CoinGecko {
    /// `COINGECKO_API_KEY` is optional; the demo tier is ample for the two calls
    /// a preflight makes.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .user_agent("tmplrmgr")
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .context("build HTTP client")?,
            api_key: std::env::var("COINGECKO_API_KEY").ok(),
        })
    }

    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        let request = self.client.get(url);
        match &self.api_key {
            Some(key) => request.header("x-cg-demo-api-key", key),
            None => request,
        }
    }
}

#[derive(Deserialize)]
struct CoinGeckoListing {
    id: String,
    symbol: String,
    name: String,
}

impl From<CoinGeckoListing> for Listing {
    fn from(listing: CoinGeckoListing) -> Self {
        Self {
            id: listing.id,
            symbol: listing.symbol,
            name: listing.name,
        }
    }
}

#[async_trait::async_trait]
impl ReferencePriceSource for CoinGecko {
    async fn lookup(&self, id: &str) -> anyhow::Result<Option<Listing>> {
        // Everything optional is switched off: only id, symbol and name are
        // wanted, and the full document is orders of magnitude larger.
        let response = self
            .get(&format!(
                "https://api.coingecko.com/api/v3/coins/{id}?localization=false\
                 &tickers=false&market_data=false&community_data=false\
                 &developer_data=false&sparkline=false"
            ))
            .send()
            .await
            .with_context(|| format!("look up `{id}`"))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let listing: CoinGeckoListing = response
            .error_for_status()
            .with_context(|| format!("lookup of `{id}` rejected"))?
            .json()
            .await
            .with_context(|| format!("decode `{id}`"))?;

        Ok(Some(listing.into()))
    }

    async fn candidates(&self, symbol: &str) -> anyhow::Result<Vec<Listing>> {
        // The whole catalogue, because only the complete set proves a ticker is
        // unambiguous. Paid for only when a spec declines to pin an id.
        let listings: Vec<CoinGeckoListing> = self
            .get("https://api.coingecko.com/api/v3/coins/list")
            .send()
            .await
            .context("request the coin list")?
            .error_for_status()
            .context("coin list request rejected")?
            .json()
            .await
            .context("decode the coin list")?;

        Ok(listings
            .into_iter()
            .map(Listing::from)
            .filter(|listing| listing.symbol.eq_ignore_ascii_case(symbol))
            .collect())
    }

    async fn prices(&self, ids: &[String]) -> anyhow::Result<BTreeMap<String, f64>> {
        #[derive(Deserialize)]
        struct Quote {
            usd: f64,
        }

        let quotes: BTreeMap<String, Quote> = self
            .get(&format!(
                "https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies=usd",
                ids.join(",")
            ))
            .send()
            .await
            .context("request prices")?
            .error_for_status()
            .context("price request rejected")?
            .json()
            .await
            .context("decode prices")?;

        Ok(quotes
            .into_iter()
            .map(|(id, quote)| (id, quote.usd))
            .collect())
    }

    fn label(&self) -> &'static str {
        "coingecko"
    }
}

/// `reference.price.{collateral,borrow,pair}`.
pub(super) async fn checks(
    source: &dyn ReferencePriceSource,
    spec: &MarketSpec,
    collateral: Option<Price>,
    borrow: Option<Price>,
) -> Vec<Check> {
    let collateral_resolved = resolve(source, "collateral", &spec.collateral).await;
    let borrow_resolved = resolve(source, "borrow", &spec.borrow).await;

    let wanted: Vec<String> = [&collateral_resolved, &borrow_resolved]
        .into_iter()
        .filter_map(|resolved| Some(resolved.as_ref().ok()?.id.clone()))
        .collect();

    let quotes = if wanted.is_empty() {
        BTreeMap::new()
    } else {
        match source.prices(&wanted).await {
            Ok(quotes) => quotes,
            // Unreachable is "could not check", never "checked and agrees".
            Err(error) => {
                return skip_all(&format!(
                    "{} did not answer ({error:#}), so nothing was cross-checked. \
                     This is not evidence the prices agree.",
                    source.label()
                ))
            }
        }
    };

    let collateral_tolerance = tolerance(&spec.collateral, spec);
    let borrow_tolerance = tolerance(&spec.borrow, spec);
    // The ratio carries both legs' deviations, so it is graded against the looser
    // band. Using one leg's would contradict an override the operator set on the
    // other — the override exists precisely for the leg that needs room.
    let pair_tolerance = if collateral_tolerance > borrow_tolerance {
        collateral_tolerance
    } else {
        borrow_tolerance
    };
    let (collateral_check, collateral_reference) = compare(
        "collateral",
        source.label(),
        &collateral_resolved,
        collateral,
        &quotes,
        collateral_tolerance,
    );
    let (borrow_check, borrow_reference) = compare(
        "borrow",
        source.label(),
        &borrow_resolved,
        borrow,
        &quotes,
        borrow_tolerance,
    );

    vec![
        collateral_check,
        borrow_check,
        pair(
            source.label(),
            collateral,
            borrow,
            collateral_reference,
            borrow_reference,
            pair_tolerance,
        ),
    ]
}

/// Report all three checks as not-run for one shared reason.
fn skip_all(reason: &str) -> Vec<Check> {
    ["collateral", "borrow", "pair"]
        .into_iter()
        .map(|side| {
            Check::new(
                format!("reference.price.{side}"),
                Status::Skipped {
                    reason: reason.to_owned(),
                },
            )
        })
        .collect()
}

fn tolerance<A: AssetClass>(asset: &AssetSpec<A>, spec: &MarketSpec) -> Decimal {
    asset
        .reference_tolerance
        .unwrap_or(spec.market.reference_tolerance)
}

/// Which listing an asset refers to, or why that cannot be established.
async fn resolve<A: AssetClass>(
    source: &dyn ReferencePriceSource,
    side: &str,
    asset: &AssetSpec<A>,
) -> Resolved {
    match asset.reference.clone().unwrap_or_default() {
        ReferenceAsset::Unlisted { reason } => Err(Status::Skipped {
            reason: format!("the spec records this {side} asset as unlisted: {reason}"),
        }),
        ReferenceAsset::CoinGecko { id } => match source.lookup(&id).await {
            Ok(Some(listing)) => Ok(listing),
            // The spec asserted this coin exists. It does not.
            Ok(None) => Err(Status::failed(format!(
                "no coin has the pinned id `{id}`, so the {side} asset's `reference` \
                 names something that does not exist"
            ))),
            Err(error) => Err(Status::Skipped {
                reason: format!("could not look up `{id}`: {error:#}"),
            }),
        },
        ReferenceAsset::ByTicker => {
            let Some(symbol) = &asset.symbol else {
                return Err(Status::Skipped {
                    reason: format!(
                        "the {side} asset has no `symbol` to resolve; set one, or pin \
                         `reference` to an id"
                    ),
                });
            };
            match source.candidates(symbol).await {
                Err(error) => Err(Status::Skipped {
                    reason: format!("could not resolve `{symbol}`: {error:#}"),
                }),
                Ok(candidates) => match candidates.as_slice() {
                    [only] => Ok(only.clone()),
                    // A ticker the source does not know is a typo, not an outage.
                    [] => Err(Status::failed(format!(
                        "no coin uses the ticker `{symbol}`"
                    ))),
                    // Never a first match: picking one of several would silently
                    // verify the wrong asset, which is worse than not checking.
                    many => Err(Status::failed(format!(
                        "`{symbol}` is ambiguous — {} coins use it ({}{}). Pin one \
                         with `reference = {{ coin_gecko = {{ id = \"…\" }} }}`.",
                        many.len(),
                        many.iter()
                            .take(5)
                            .map(|listing| listing.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                        if many.len() > 5 { ", …" } else { "" }
                    ))),
                },
            }
        }
    }
}

/// Compare one leg, returning its check and the reference price it used.
fn compare(
    side: &str,
    label: &str,
    resolved: &Resolved,
    aggregated: Option<Price>,
    quotes: &BTreeMap<String, f64>,
    tolerance: Decimal,
) -> (Check, Option<f64>) {
    let id = format!("reference.price.{side}");

    let listing = match resolved {
        Ok(listing) => listing,
        Err(status) => return (Check::new(id, status.clone()), None),
    };

    let Some(aggregated) = aggregated.map(|price| scaled(&price)) else {
        return (
            Check::new(
                id,
                Status::Skipped {
                    reason: format!(
                        "the {side} leg produced no aggregate to compare against \
                         {} `{}`",
                        label, listing.id
                    ),
                },
            ),
            None,
        );
    };

    let Some(&reference) = quotes.get(&listing.id) else {
        return (
            Check::new(
                id,
                Status::Skipped {
                    reason: format!("{label} returned no price for `{}`", listing.id),
                },
            ),
            None,
        );
    };

    // The resolved name is half the point: it is how a human confirms the right
    // coin was pulled, not merely a matching ticker.
    let identity = format!("{} `{}` \"{}\"", label, listing.id, listing.name);
    let Some(deviation) = deviation(aggregated, reference) else {
        return (
            Check::new(
                id,
                Status::failed(format!(
                    "{identity} reports 0, so no comparison is possible"
                )),
            ),
            None,
        );
    };

    // Lossy is right here: this is a tolerance band, not an exact comparison.
    let within = deviation <= tolerance.to_f64_lossy();
    let detail = format!(
        "{identity} ${reference} vs aggregated ${aggregated} ({:+.3}%)",
        signed_percent(aggregated, reference)
    );

    (
        Check::new(
            id,
            if within {
                Status::passed(detail)
            } else {
                Status::failed(format!(
                    "{detail}, outside the {tolerance} band. Either the feed is wrong \
                     or this asset does not track what `reference` claims."
                ))
            },
        ),
        Some(reference),
    )
}

/// The ratio check — the strongest of the three.
///
/// Comparing ratios cancels any USD-denomination drift between the two APIs, so
/// it catches a transposed feed while tolerating the small disagreement two
/// independent price sources always have.
fn pair(
    label: &str,
    collateral: Option<Price>,
    borrow: Option<Price>,
    collateral_reference: Option<f64>,
    borrow_reference: Option<f64>,
    tolerance: Decimal,
) -> Check {
    let id = "reference.price.pair";
    let (Some(collateral), Some(borrow), Some(collateral_reference), Some(borrow_reference)) =
        (collateral, borrow, collateral_reference, borrow_reference)
    else {
        return Check::new(
            id,
            Status::Skipped {
                reason: "both legs must have an aggregate and a reference price".to_owned(),
            },
        );
    };

    let aggregated = scaled(&borrow);
    if aggregated == 0.0 || borrow_reference == 0.0 {
        return Check::new(
            id,
            Status::Skipped {
                reason: "a borrow price of zero admits no ratio".to_owned(),
            },
        );
    }
    let ours = scaled(&collateral) / aggregated;
    let theirs = collateral_reference / borrow_reference;

    let Some(deviation) = deviation(ours, theirs) else {
        return Check::new(
            id,
            Status::Skipped {
                reason: "the reference ratio is zero, so no comparison is possible".to_owned(),
            },
        );
    };

    let detail = format!(
        "{label} {theirs} vs aggregated {ours} ({:+.3}%)",
        signed_percent(ours, theirs)
    );
    Check::new(
        id,
        if deviation <= tolerance.to_f64_lossy() {
            Status::passed(detail)
        } else {
            Status::failed(format!(
                "{detail}, outside the {tolerance} band. A ratio disagreeing this far \
                 usually means a transposed feed id."
            ))
        },
    )
}

/// Absolute relative difference. `None` when the reference is zero.
fn deviation(ours: f64, theirs: f64) -> Option<f64> {
    (theirs != 0.0).then(|| ((ours - theirs) / theirs).abs())
}

fn signed_percent(ours: f64, theirs: f64) -> f64 {
    if theirs == 0.0 {
        0.0
    } else {
        (ours - theirs) / theirs * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::{Listing, ReferencePriceSource};
    use std::collections::BTreeMap;

    /// A fixed catalogue. The point of the trait: no test reaches the network,
    /// so the rules below are pinned rather than dependent on today's market.
    struct Fixture {
        listings: Vec<Listing>,
        prices: BTreeMap<String, f64>,
        listings_fail: bool,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                listings: vec![
                    listing("ripple", "xrp", "XRP"),
                    listing("usd-coin", "usdc", "USDC"),
                    // Two coins share the ticker `bat`, which is what makes
                    // resolving by ticker ambiguous.
                    listing("basic-attention-token", "bat", "Basic Attention Token"),
                    listing("batcoin", "bat", "BatCoin"),
                ],
                prices: [("ripple".to_owned(), 3.0), ("usd-coin".to_owned(), 1.0)]
                    .into_iter()
                    .collect(),
                listings_fail: false,
            }
        }
    }

    fn listing(id: &str, symbol: &str, name: &str) -> Listing {
        Listing {
            id: id.to_owned(),
            symbol: symbol.to_owned(),
            name: name.to_owned(),
        }
    }

    #[async_trait::async_trait]
    impl ReferencePriceSource for Fixture {
        async fn lookup(&self, id: &str) -> anyhow::Result<Option<Listing>> {
            if self.listings_fail {
                anyhow::bail!("simulated outage");
            }
            Ok(self.listings.iter().find(|l| l.id == id).cloned())
        }

        async fn candidates(&self, symbol: &str) -> anyhow::Result<Vec<Listing>> {
            if self.listings_fail {
                anyhow::bail!("simulated outage");
            }
            Ok(self
                .listings
                .iter()
                .filter(|l| l.symbol.eq_ignore_ascii_case(symbol))
                .cloned()
                .collect())
        }

        async fn prices(&self, ids: &[String]) -> anyhow::Result<BTreeMap<String, f64>> {
            Ok(ids
                .iter()
                .filter_map(|id| self.prices.get(id).map(|price| (id.clone(), *price)))
                .collect())
        }

        fn label(&self) -> &'static str {
            "fixture"
        }
    }

    fn spec() -> crate::spec::MarketSpec {
        crate::spec::extends::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../deployments/alpha/iethfxrp-ixlmusdc.toml"),
        )
        .expect("fixture spec should load")
    }

    fn price(value: i64, expo: i32) -> templar_proxy_oracle_kernel::Price {
        templar_proxy_oracle_kernel::Price {
            price: value,
            conf: 0,
            expo,
            publish_time_ns: templar_common::Nanoseconds::from_ns(0),
        }
    }

    fn find<'a>(
        checks: &'a [crate::spec::check::Check],
        id: &str,
    ) -> &'a crate::spec::check::Status {
        &checks
            .iter()
            .find(|check| check.id == id)
            .unwrap_or_else(|| panic!("{id} should be reported"))
            .status
    }

    /// Agreement inside the band passes, and the report names the resolved id
    /// *and* name — which is how a human confirms the right coin was pulled.
    #[tokio::test]
    async fn agreement_passes_and_names_the_coin() {
        let checks = super::checks(
            &Fixture::new(),
            &spec(),
            Some(price(300_100_000, -8)),
            Some(price(100_000_000, -8)),
        )
        .await;

        let crate::spec::check::Status::Passed { detail } =
            find(&checks, "reference.price.collateral")
        else {
            panic!("expected a pass: {checks:#?}")
        };
        assert!(detail.contains("ripple"), "{detail}");
        assert!(detail.contains("XRP"), "{detail}");
    }

    /// A price outside the band is a hard failure, not a shrug — this is the
    /// "checked and disagrees" half of the distinction.
    #[tokio::test]
    async fn disagreement_fails() {
        let checks = super::checks(
            &Fixture::new(),
            &spec(),
            // Reference says 3.0; claiming 6.0 is 100% out.
            Some(price(600_000_000, -8)),
            Some(price(100_000_000, -8)),
        )
        .await;

        assert!(
            matches!(
                find(&checks, "reference.price.collateral"),
                crate::spec::check::Status::Failed { .. }
            ),
            "{checks:#?}"
        );
    }

    /// An unreachable source must never read as agreement — a deploy should not
    /// be blocked by a third party's outage, nor waved through by it.
    #[tokio::test]
    async fn an_outage_is_skipped_not_passed() {
        let mut fixture = Fixture::new();
        fixture.listings_fail = true;

        let checks = super::checks(
            &fixture,
            &spec(),
            Some(price(300_000_000, -8)),
            Some(price(100_000_000, -8)),
        )
        .await;

        for check in &checks {
            assert!(
                matches!(check.status, crate::spec::check::Status::Skipped { .. }),
                "{check:#?}"
            );
        }
    }

    /// A pinned id the source does not know is a failure, not a shrug: the spec
    /// asserted that coin exists, and the source just disproved it. This is the
    /// same distinction the account checks make, and the one most easily lost.
    #[tokio::test]
    async fn a_wrong_pinned_id_fails() {
        let mut spec = spec();
        spec.collateral.reference = Some(crate::spec::oracle::ReferenceAsset::CoinGecko {
            id: "ripple-typo".to_owned(),
        });

        let checks = super::checks(
            &Fixture::new(),
            &spec,
            Some(price(300_000_000, -8)),
            Some(price(100_000_000, -8)),
        )
        .await;

        let status = find(&checks, "reference.price.collateral");
        assert!(
            matches!(status, crate::spec::check::Status::Failed { .. }),
            "a nonexistent pinned id must fail, not skip: {status:#?}"
        );
    }

    /// The ratio carries both legs' deviations, so an override on either leg
    /// must widen its band — otherwise the pair check contradicts a leg check
    /// the operator deliberately configured.
    #[tokio::test]
    async fn the_pair_band_honours_either_leg_override() {
        let mut spec = spec();
        spec.borrow.reference_tolerance = Some(templar_common::dec!("0.5"));

        // Borrow 20% off: inside its own 50% band, and the ratio must follow.
        let checks = super::checks(
            &Fixture::new(),
            &spec,
            Some(price(300_000_000, -8)),
            Some(price(120_000_000, -8)),
        )
        .await;

        assert!(
            matches!(
                find(&checks, "reference.price.pair"),
                crate::spec::check::Status::Passed { .. }
            ),
            "{checks:#?}"
        );
    }

    /// The ratio cancels USD drift between the two sources, so it still passes
    /// when both legs are uniformly off.
    #[tokio::test]
    async fn the_ratio_tolerates_uniform_drift() {
        // Both legs 10% high: each leg fails its own band, the ratio does not.
        let checks = super::checks(
            &Fixture::new(),
            &spec(),
            Some(price(330_000_000, -8)),
            Some(price(110_000_000, -8)),
        )
        .await;

        assert!(
            matches!(
                find(&checks, "reference.price.pair"),
                crate::spec::check::Status::Passed { .. }
            ),
            "{checks:#?}"
        );
    }
}
