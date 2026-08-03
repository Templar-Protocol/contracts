//! Profile composition for [`super::MarketSpec`]: `extends` merges each listed
//! file beneath the declaring one, in order, before a single deserialization.
//!
//! Merged as [`toml::Value`] because a profile is a fragment — it does not
//! satisfy `MarketSpec`'s required fields, so it cannot be deserialized first.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use toml::Value;

use super::{MarketSpec, SCHEMA_VERSION};

/// Read `path`, resolve its `extends` chain, and deserialize the result.
pub fn load(path: &Path) -> anyhow::Result<MarketSpec> {
    let mut visiting = BTreeSet::new();
    let merged = resolve(path, &mut visiting)?;

    // Read before deserializing. Every struct here is `deny_unknown_fields`, so
    // a document from a newer build fails on the first field this one does not
    // know — and the author reads "unknown field `x`" when the real answer is
    // that their spec is a version this build cannot speak.
    if let Some(schema) = merged.get("schema").and_then(toml::Value::as_integer) {
        anyhow::ensure!(
            schema == i64::from(SCHEMA_VERSION),
            "{} declares schema {schema} but this build understands \
             {SCHEMA_VERSION}. Re-generate it with `market export`, or use a \
             build that speaks it.",
            path.display(),
        );
    }

    let mut spec: MarketSpec = merged
        .try_into()
        .with_context(|| format!("invalid market spec {}", path.display()))?;

    // Unreachable via the check above unless `schema` was absent or not an
    // integer, in which case deserialization has now produced the real value.
    anyhow::ensure!(
        spec.schema == SCHEMA_VERSION,
        "{} declares schema {} but this build understands {SCHEMA_VERSION}",
        path.display(),
        spec.schema,
    );

    // The chain is fully applied; leaving the paths in would imply otherwise to
    // anything that re-serializes the spec (e.g. `market export`).
    spec.extends.clear();
    Ok(spec)
}

/// The merged `toml::Value` for `path`, with its `extends` chain applied.
fn resolve(path: &Path, visiting: &mut BTreeSet<PathBuf>) -> anyhow::Result<Value> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("spec file not found: {}", path.display()))?;

    if !visiting.insert(canonical.clone()) {
        anyhow::bail!("`extends` cycle through {}", canonical.display());
    }

    let raw = std::fs::read_to_string(&canonical)
        .with_context(|| format!("read {}", canonical.display()))?;
    let mut value: Value =
        toml::from_str(&raw).with_context(|| format!("parse {} as TOML", canonical.display()))?;

    let parents = take_extends(&mut value, &canonical)?;

    // Parents merge left to right, then the declaring file wins over all of
    // them — so a market can always override whatever a profile set.
    let mut merged = Value::Table(toml::Table::new());
    for parent in parents {
        merge(&mut merged, resolve(&parent, visiting)?);
    }
    merge(&mut merged, value);

    visiting.remove(&canonical);
    Ok(merged)
}

/// Remove `extends` from `value`, returning the paths resolved relative to the
/// file that declared them.
fn take_extends(value: &mut Value, declaring_file: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let Some(table) = value.as_table_mut() else {
        anyhow::bail!("{} must be a TOML table", declaring_file.display());
    };
    let Some(extends) = table.remove("extends") else {
        return Ok(Vec::new());
    };

    let base = declaring_file.parent().unwrap_or(Path::new("."));
    extends
        .as_array()
        .context("`extends` must be an array of paths")?
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(|relative| base.join(relative))
                .context("`extends` entries must be strings")
        })
        .collect()
}

/// Merge `overlay` into `base`, with `overlay` winning, stopping at `[section]`
/// keys: values below replace wholesale.
///
/// The depth limit is load-bearing. Merging an externally-tagged enum key by
/// key yields `{ Flat, Proportional }`, which deserializes to nothing, and a
/// range stating only a minimum would inherit the profile's maximum.
fn merge(base: &mut Value, overlay: Value) {
    merge_to_depth(base, overlay, 2);
}

fn merge_to_depth(base: &mut Value, overlay: Value, depth: usize) {
    match (base, overlay) {
        (Value::Table(base), Value::Table(overlay)) if depth > 0 => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(existing) => merge_to_depth(existing, value, depth - 1),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}
