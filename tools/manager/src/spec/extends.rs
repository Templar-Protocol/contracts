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

use super::{MarketSpec, RawMarketSpec, SCHEMA_VERSION};

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
             build that speaks it. Schema 5 made every amount state its unit \
             (`0.04 tokens`, `1 atom`), so a schema 4 file must be re-authored \
             rather than renumbered.",
            path.display(),
        );
    }

    // Parsed in two steps, because the two failures are different. The raw shape
    // is what the file states, so it is what an unread key is measured against;
    // the conversion below is where a proxy that names no governance stops
    // being expressible.
    let raw: RawMarketSpec = merged
        .clone()
        .try_into()
        .with_context(|| format!("invalid market spec {}", path.display()))?;
    ensure_every_key_was_read(&merged, &raw, path)?;

    let mut spec = MarketSpec::try_from(raw)
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

/// Refuse a document carrying a key the spec did not take.
///
/// `deny_unknown_fields` covers this crate's own structs, but not the on-chain
/// types they embed — `AmountRange`, the interest-rate strategies, the fees,
/// `YieldWeights`. A typo in one of those deserializes to that field's default:
/// `maximim` leaves a range unbounded and deploys.
fn ensure_every_key_was_read(
    merged: &Value,
    raw: &RawMarketSpec,
    path: &Path,
) -> anyhow::Result<()> {
    let read = Value::try_from(raw).context("re-serialize the spec to find unread keys")?;

    let mut unread = Vec::new();
    collect_unread(merged, &read, "", &mut unread);
    anyhow::ensure!(
        unread.is_empty(),
        "{} states {} that nothing reads: {}. A key spelled wrongly is dropped \
         silently and its field takes its default value.",
        path.display(),
        if unread.len() == 1 { "a key" } else { "keys" },
        unread.join(", "),
    );
    Ok(())
}

/// Keys present in `input` and absent from `read`. Keys only: `Decimal` does not
/// round-trip its text (`"1.2"` re-serializes as `"1.199…9"`), so comparing
/// values would report every market as broken.
fn collect_unread(input: &Value, read: &Value, path: &str, unread: &mut Vec<String>) {
    let join = |key: &str| {
        if path.is_empty() {
            key.to_owned()
        } else {
            format!("{path}.{key}")
        }
    };

    match (input, read) {
        (Value::Table(input), Value::Table(read)) => {
            for (key, value) in input {
                match read.get(key) {
                    Some(read) => collect_unread(value, read, &join(key), unread),
                    None => unread.push(join(key)),
                }
            }
        }
        (Value::Array(input), Value::Array(read)) => {
            for (index, (value, read)) in input.iter().zip(read).enumerate() {
                collect_unread(value, read, &format!("{path}[{index}]"), unread);
            }
        }
        _ => {}
    }
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

#[cfg(test)]
mod tests {
    use super::{ensure_every_key_was_read, Path, RawMarketSpec, Value};
    use crate::spec::plan::testing::alpha_market;

    /// The file shape the alpha fixture is written in.
    fn raw() -> RawMarketSpec {
        alpha_market().into()
    }

    /// The case `deny_unknown_fields` cannot see: `AmountRange` is an on-chain
    /// type, so a misspelled `maximum` deserializes to `None` — an unbounded
    /// range — and the market deploys with it.
    #[test]
    fn a_typo_inside_an_embedded_on_chain_type_is_refused() {
        let spec = raw();
        let mut merged = Value::try_from(&spec).expect("a spec serializes");
        merged["market"]["borrow_range"]
            .as_table_mut()
            .expect("a range is a table")
            .insert("maximim".to_owned(), Value::String("1".to_owned()));

        let error = ensure_every_key_was_read(&merged, &spec, Path::new("m.toml"))
            .expect_err("nothing reads `maximim`");

        assert!(
            format!("{error:#}").contains("market.borrow_range.maximim"),
            "the refusal must name the key by its full path: {error:#}"
        );
    }

    /// And a spec that states only what it means passes. The 45 checked-in
    /// specs are the wider proof; this one names the mechanism.
    #[test]
    fn a_spec_stating_only_what_it_means_is_accepted() {
        let spec = raw();
        let merged = Value::try_from(&spec).expect("a spec serializes");

        ensure_every_key_was_read(&merged, &spec, Path::new("m.toml")).expect("nothing is unread");
    }
}
