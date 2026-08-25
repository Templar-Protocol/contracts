//! Profile composition for versioned TOML specs.
//!
//! Each file merges its `extends` chain beneath itself before one
//! deserialization. [`toml::Value`] carries profile fragments that cannot
//! satisfy a complete spec on their own.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use serde::{de::DeserializeOwned, Serialize};
use toml::Value;

use super::{MarketSpec, RawMarketSpec, SCHEMA_VERSION};

/// Read a market spec, applying profiles and validating every authored key.
pub fn load(path: &Path) -> anyhow::Result<MarketSpec> {
    let raw: RawMarketSpec = load_raw(path, SCHEMA_VERSION, "market export")?;
    let mut spec = MarketSpec::try_from(raw)
        .with_context(|| format!("invalid market spec {}", path.display()))?;
    spec.extends.clear();
    Ok(spec)
}

/// Read a versioned TOML spec after resolving its `extends` chain.
///
/// The caller owns conversion from the raw file shape into its domain model;
/// this keeps profile composition and typo detection identical for every spec.
pub fn load_raw<T>(path: &Path, schema_version: u32, regenerate: &str) -> anyhow::Result<T>
where
    T: DeserializeOwned + Serialize,
{
    let mut visiting = BTreeSet::new();
    let merged = resolve(path, &mut visiting)?;

    if let Some(schema) = merged.get("schema").and_then(toml::Value::as_integer) {
        let migration_note = if schema_version == SCHEMA_VERSION {
            " Schema 5 made every amount state its unit (`0.04 tokens`, `1 atom`), \
             so a schema 4 file must be re-authored rather than renumbered."
        } else {
            ""
        };
        anyhow::ensure!(
            schema == i64::from(schema_version),
            "{} declares schema {schema} but this build understands {schema_version}. \
             Re-generate it with `{regenerate}`.{migration_note}",
            path.display(),
        );
    }

    let raw: T = merged
        .clone()
        .try_into()
        .with_context(|| format!("invalid spec {}", path.display()))?;
    ensure_every_key_was_read(&merged, &raw, path)?;
    Ok(raw)
}

/// Refuse a document carrying a key the spec did not take.
///
/// `deny_unknown_fields` covers this crate's own structs, but not the on-chain
/// types they embed — `AmountRange`, the interest-rate strategies, the fees,
/// `YieldWeights`. A typo in one of those deserializes to that field's default:
/// `maximim` leaves a range unbounded and deploys.
fn ensure_every_key_was_read<T: Serialize>(
    merged: &Value,
    raw: &T,
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
    absolutize_file_paths(&mut value, &canonical);

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

fn absolutize_file_paths(value: &mut Value, declaring_file: &Path) {
    match value {
        Value::Table(table) if table.len() == 1 => {
            if let Some(Value::String(path)) = table.get_mut("file") {
                let base = declaring_file.parent().unwrap_or(Path::new("."));
                *path = base.join(&*path).to_string_lossy().into_owned();
                return;
            }
            for (_, value) in table.iter_mut() {
                absolutize_file_paths(value, declaring_file);
            }
        }
        Value::Table(table) => {
            for (_, value) in table.iter_mut() {
                absolutize_file_paths(value, declaring_file);
            }
        }
        Value::Array(values) => {
            for value in values {
                absolutize_file_paths(value, declaring_file);
            }
        }
        _ => {}
    }
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
