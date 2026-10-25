//! Compile `releases/` into the catalog's release lists.
//!
//! Release history is one file per release so that independently recorded rows
//! cannot conflict — see `RELEASES.md`. Malformed rows fail the build
//! rather than surfacing at runtime.

use std::{collections::BTreeMap, fmt::Write as _, path::Path};

const SOURCE: &str = "releases";
const COLUMNS: usize = 6;
/// Rows for the Soroban proxy-oracle stack. Their keys are `soroban-`-prefixed
/// precisely so this one test separates them from the NEAR catalog.
const SOROBAN_PREFIX: &str = "soroban-";

fn main() {
    println!("cargo:rerun-if-changed={SOURCE}");

    let mut by_artifact: BTreeMap<String, Vec<Release>> = BTreeMap::new();
    let mut by_soroban_artifact: BTreeMap<String, Vec<Release>> = BTreeMap::new();

    let dir = std::fs::read_dir(SOURCE).unwrap_or_else(|e| panic!("{SOURCE}: {e}"));
    for entry in dir {
        let entry = entry.unwrap_or_else(|e| panic!("{SOURCE}: {e}"));
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "tsv") {
            continue;
        }
        // `DirEntry::file_type` does not follow links. A symlink's target lives
        // outside `releases/`, where the append-only CI check does not look, so
        // its digest could be changed later without touching a guarded path.
        let kind = entry
            .file_type()
            .unwrap_or_else(|e| panic!("{SOURCE}: {e}"));
        assert!(
            kind.is_file(),
            "{} must be a regular file: only a file's own contents are covered \
             by the append-only check.",
            path.display(),
        );
        let release = parse(&path);
        // An unknown key in either table pascal-cases onto a variant that does
        // not exist, so the generated match fails to compile and names the row
        // that caused it — the same protection for both catalogs.
        let (table, variant) = match release.artifact.strip_prefix(SOROBAN_PREFIX) {
            None => (&mut by_artifact, pascal_case(&release.artifact)),
            Some(key) => (&mut by_soroban_artifact, pascal_case(key)),
        };
        table.entry(variant).or_default().push(release);
    }

    let mut generated = String::from("// @generated from releases/ by build.rs — do not edit.\n");
    write_lookup(
        &mut generated,
        "releases_for",
        "ArtifactId",
        &mut by_artifact,
    );
    write_lookup(
        &mut generated,
        "soroban_releases_for",
        "SorobanArtifactId",
        &mut by_soroban_artifact,
    );

    let Some(out_dir) = std::env::var_os("OUT_DIR") else {
        panic!("OUT_DIR is unset; cargo always provides it to a build script")
    };
    let out = Path::new(&out_dir).join("releases.rs");
    std::fs::write(&out, generated).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
}

/// One catalog's release lookup. An artifact absent from the directory has
/// never been released, and a catalog with no rows at all is the same answer
/// for every id — written as one, so the generated code stays what clippy asks
/// for rather than a match on a wildcard.
fn write_lookup(
    generated: &mut String,
    function: &str,
    enum_name: &str,
    table: &mut BTreeMap<String, Vec<Release>>,
) {
    if table.is_empty() {
        writeln!(
            generated,
            "pub(crate) fn {function}(_: {enum_name}) -> &'static [ArtifactRelease] {{\n\
             \x20   &[]\n\
             }}\n",
        )
        .expect("writing to a String");
        return;
    }

    write!(
        generated,
        "pub(crate) fn {function}(id: {enum_name}) -> &'static [ArtifactRelease] {{\n\
         \x20   match id {{\n",
    )
    .expect("writing to a String");
    for (variant, releases) in table {
        // `read_dir` order is unspecified and `current()` is the last entry, so
        // the ordering has to be imposed rather than inherited.
        releases.sort_by_key(|release| version_key(&release.version));

        write!(generated, "        {enum_name}::{variant} => &[").expect("writing to a String");
        for release in releases {
            write!(
                generated,
                "ArtifactRelease {{ version: {:?}, tag: {:?}, asset: {:?}, sha256: {:?}, \
                 length: {} }},",
                release.version,
                release.tag,
                release.asset,
                release.sha256,
                digit_groups(release.length),
            )
            .expect("writing to a String");
        }
        generated.push_str("],\n");
    }
    generated.push_str("        _ => &[],\n    }\n}\n");
}

struct Release {
    artifact: String,
    version: String,
    tag: String,
    asset: String,
    sha256: String,
    length: usize,
}

fn parse(path: &Path) -> Release {
    let name = path.display();
    let contents = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{name}: {e}"));

    let row = contents.trim();
    let fields = row.split('\t').collect::<Vec<_>>();
    assert!(
        fields.len() == COLUMNS,
        "{name}: expected {COLUMNS} tab-separated fields, found {}. Columns are \
         artifact, version, tag, asset, sha256, length.",
        fields.len()
    );

    let release = Release {
        artifact: fields[0].to_owned(),
        version: fields[1].to_owned(),
        tag: fields[2].to_owned(),
        asset: fields[3].to_owned(),
        sha256: fields[4].to_ascii_lowercase(),
        length: byte_length(fields[5])
            .unwrap_or_else(|| panic!("{name}: length `{}` is not a plain byte count", fields[5])),
    };

    assert!(
        release.sha256.len() == 64 && release.sha256.chars().all(|c| c.is_ascii_hexdigit()),
        "{name}: sha256 `{}` is not a 64-char hex digest",
        release.sha256,
    );
    assert!(
        is_canonical_artifact(&release.artifact),
        "{name}: artifact `{}` is not canonical kebab-case; it would join another \
         artifact's release list.",
        release.artifact,
    );
    assert!(
        version_key(&release.version).is_some(),
        "{name}: version `{}` is not canonically spelled `major.minor.patch`, \
         which releases are ordered by.",
        release.version,
    );
    for (field, value) in [("tag", &release.tag), ("asset", &release.asset)] {
        assert!(
            is_url_safe(value),
            "{name}: {field} `{value}` is not usable as a URL path segment.",
        );
    }

    // The filename is the uniqueness key that makes independently recorded rows
    // conflict-free, so it has to agree with the row it holds.
    let expected = format!("{}@{}.tsv", release.artifact, release.version);
    let actual = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    assert!(
        actual == expected,
        "{name} describes {}@{} and should be named {expected}",
        release.artifact,
        release.version,
    );

    release
}

/// `221984` -> `221_984`, the spelling `clippy::unreadable_literal` asks for in
/// the generated catalog.
fn digit_groups(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + (digits.len() - 1) / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push('_');
        }
        grouped.push(digit);
    }
    grouped
}

/// A released asset's size in bytes.
///
/// `None` unless canonically spelled, and never zero: `0400000` and `+400000`
/// parse, but neither is what `wc -c` reports, so both mean the row was written
/// by something other than the release build.
fn byte_length(length: &str) -> Option<usize> {
    let value = length.parse::<usize>().ok()?;
    (value.to_string() == length && value > 0).then_some(value)
}

/// Orders versions numerically, so `0.10.0` follows `0.9.0`.
///
/// `None` unless canonically spelled: `1.03.0` would key the same as `1.3.0`,
/// and equal keys sort by `read_dir` order, which differs between machines.
fn version_key(version: &str) -> Option<(u64, u64, u64)> {
    let mut components = version.split('.');
    let mut next = || {
        let component = components.next()?;
        let value = component.parse::<u64>().ok()?;
        (value.to_string() == component).then_some(value)
    };
    let key = (next()?, next()?, next()?);
    components.next().is_none().then_some(key)
}

/// Canonical kebab-case. `Market`, `market-` and `proxy--oracle` otherwise
/// pascal-case onto an existing variant and join its release list.
fn is_canonical_artifact(artifact: &str) -> bool {
    !artifact.is_empty()
        && artifact.split('-').all(|word| {
            !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Usable as a URL path segment verbatim, so `asset_url` needs no encoding.
fn is_url_safe(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `proxy-oracle` -> `ProxyOracle`, matching `ArtifactId`'s kebab-case naming.
fn pascal_case(kebab: &str) -> String {
    kebab
        .split('-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}
