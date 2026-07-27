//! Turn `releases.tsv` into the catalog's release lists.
//!
//! The released history is data, not code. Keeping it in a table means CI
//! appends a release by writing one line — no parsing Rust, no splicing text
//! into a source file, no `cargo fmt` afterwards to tidy up — and a release PR
//! diffs as exactly the row that was added.
//!
//! Validation happens here rather than at runtime: a malformed row fails the
//! build of the crate that would have served it.

use std::{collections::BTreeMap, fmt::Write as _, path::Path};

const SOURCE: &str = "releases.tsv";
const COLUMNS: usize = 5;

fn main() {
    println!("cargo:rerun-if-changed={SOURCE}");

    let table = std::fs::read_to_string(SOURCE).unwrap_or_else(|e| panic!("{SOURCE}: {e}"));

    // BTreeMap keyed by variant so the generated match arms are deterministic;
    // the Vec preserves file order, which is the release order the catalog
    // promises (oldest first).
    let mut by_artifact: BTreeMap<String, Vec<Release>> = BTreeMap::new();

    for (number, line) in table.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |what: &str| -> String { format!("{SOURCE}:{}: {what}", number + 1) };

        let fields = line.split('\t').collect::<Vec<_>>();
        assert!(
            fields.len() == COLUMNS,
            "{}",
            at(&format!(
                "expected {COLUMNS} tab-separated fields, found {}. Columns are \
                 artifact, version, tag, asset, sha256.",
                fields.len()
            ))
        );

        let [artifact, version, tag, asset, sha256] =
            [fields[0], fields[1], fields[2], fields[3], fields[4]];

        for (name, value) in [
            ("artifact", artifact),
            ("version", version),
            ("tag", tag),
            ("asset", asset),
        ] {
            assert!(!value.is_empty(), "{}", at(&format!("{name} is empty")));
        }
        assert!(
            sha256.len() == 64 && sha256.chars().all(|c| c.is_ascii_hexdigit()),
            "{}",
            at(&format!("sha256 `{sha256}` is not a 64-char hex digest"))
        );

        by_artifact
            .entry(pascal_case(artifact))
            .or_default()
            .push(Release {
                version: version.to_owned(),
                tag: tag.to_owned(),
                asset: asset.to_owned(),
                sha256: sha256.to_ascii_lowercase(),
            });
    }

    let mut generated = String::from(
        "// @generated from releases.tsv by build.rs — do not edit.\n\
         pub(crate) fn releases_for(id: ArtifactId) -> &'static [ArtifactRelease] {\n\
         \x20   match id {\n",
    );
    for (variant, releases) in &by_artifact {
        write!(generated, "        ArtifactId::{variant} => &[")
            .expect("writing to a String cannot fail");
        for release in releases {
            write!(
                generated,
                "ArtifactRelease {{ version: {:?}, tag: {:?}, asset: {:?}, sha256: {:?} }},",
                release.version, release.tag, release.asset, release.sha256,
            )
            .expect("writing to a String cannot fail");
        }
        generated.push_str("],\n");
    }
    // Artifacts absent from the table have simply never been released — mocks,
    // and any contract that has not shipped yet.
    generated.push_str("        _ => &[],\n    }\n}\n");

    let Some(out_dir) = std::env::var_os("OUT_DIR") else {
        panic!("OUT_DIR is unset; cargo always provides it to a build script")
    };
    let out = Path::new(&out_dir).join("releases.rs");
    std::fs::write(&out, generated).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
}

struct Release {
    version: String,
    tag: String,
    asset: String,
    sha256: String,
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
