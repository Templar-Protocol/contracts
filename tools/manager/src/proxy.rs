use std::path::Path;

use anyhow::Context as _;
use serde::Deserialize;
use serde_json::{json, Map, Value};

/// Load a proxy-configuration file, accepting both the canonical
/// `{aggregator: {MedianLow|…}, freshness_filter}` shape and the legacy
/// `{aggregator: {method, filter}, entries}` shape that the checked-in
/// `proxy-*.json` files under `tools/manager/fixtures/deployed/` still use.
///
/// Legacy reshaping only supports the `MedianLow` aggregator method (it bails on
/// any other) and only carries the `min_sources`/`max_age`/`max_clock_drift`
/// filter fields. Once the checked-in files are migrated to the canonical shape,
/// `reshape_legacy` and this sniffing can be deleted and callers can parse the
/// proxy config directly.
pub(super) fn load_proxy_file(path: &Path) -> anyhow::Result<Value> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read proxy file {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse proxy file {}", path.display()))?;

    if is_canonical_shape(&value) {
        return Ok(value);
    }

    reshape_legacy(value).with_context(|| format!("reshape legacy proxy file {}", path.display()))
}

fn is_canonical_shape(value: &Value) -> bool {
    if value.get("freshness_filter").is_some() {
        return true;
    }
    value
        .get("aggregator")
        .and_then(|a| a.as_object())
        .is_some_and(|a| {
            a.contains_key("MedianLow")
                || a.contains_key("MedianHigh")
                || a.contains_key("Priority")
        })
}

#[derive(Deserialize)]
struct LegacyProxy {
    aggregator: LegacyAggregator,
    entries: Vec<Value>,
}

#[derive(Deserialize)]
struct LegacyAggregator {
    method: String,
    filter: LegacyFilter,
}

#[derive(Deserialize)]
struct LegacyFilter {
    min_sources: u32,
    #[serde(default)]
    max_age: Option<String>,
    #[serde(default)]
    max_clock_drift: Option<String>,
}

fn reshape_legacy(value: Value) -> anyhow::Result<Value> {
    let legacy: LegacyProxy = serde_json::from_value(value)?;
    if legacy.aggregator.method != "MedianLow" {
        anyhow::bail!(
            "unsupported legacy aggregator method `{}`; only MedianLow is supported \
             (use the canonical proxy shape or the `write` fallback for others)",
            legacy.aggregator.method
        );
    }

    let mut median_low = Map::new();
    median_low.insert("sources".to_owned(), Value::Array(legacy.entries));
    median_low.insert(
        "min_sources".to_owned(),
        json!(legacy.aggregator.filter.min_sources),
    );

    let mut freshness = Map::new();
    if let Some(max_age) = legacy.aggregator.filter.max_age {
        freshness.insert("max_age_ns".to_owned(), json!(max_age));
    }
    if let Some(max_clock_drift) = legacy.aggregator.filter.max_clock_drift {
        freshness.insert("max_clock_drift_ns".to_owned(), json!(max_clock_drift));
    }

    Ok(json!({
        "aggregator": { "MedianLow": Value::Object(median_low) },
        "freshness_filter": Value::Object(freshness),
    }))
}
