//! Compile `releases/` into the catalog's release lists.
//!
//! Release history is one file per release so that concurrent catalog PRs
//! cannot conflict — see `releases/README.md`. Malformed rows fail the build
//! rather than surfacing at runtime.

use std::{collections::BTreeMap, fmt::Write as _, path::Path};

const SOURCE: &str = "releases";
const COLUMNS: usize = 5;

fn main() {
    println!("cargo:rerun-if-changed={SOURCE}");

    let mut by_artifact: BTreeMap<String, Vec<Release>> = BTreeMap::new();

    let dir = std::fs::read_dir(SOURCE).unwrap_or_else(|e| panic!("{SOURCE}: {e}"));
    for entry in dir {
        let path = entry.unwrap_or_else(|e| panic!("{SOURCE}: {e}")).path();
        if path.extension().is_none_or(|extension| extension != "tsv") {
            continue;
        }
        let release = parse(&path);
        by_artifact
            .entry(pascal_case(&release.artifact))
            .or_default()
            .push(release);
    }

    let mut generated = String::from(
        "// @generated from releases/ by build.rs — do not edit.\n\
         pub(crate) fn releases_for(id: ArtifactId) -> &'static [ArtifactRelease] {\n\
         \x20   match id {\n",
    );
    for (variant, releases) in &mut by_artifact {
        // `read_dir` order is unspecified, and `current()` is defined as the
        // last entry, so the ordering the catalog promises has to be imposed
        // here rather than inherited from the filesystem. `parse` has already
        // rejected anything unsortable.
        releases.sort_by_key(|release| version_key(&release.version));

        write!(generated, "        ArtifactId::{variant} => &[").expect("writing to a String");
        for release in releases {
            write!(
                generated,
                "ArtifactRelease {{ version: {:?}, tag: {:?}, asset: {:?}, sha256: {:?} }},",
                release.version, release.tag, release.asset, release.sha256,
            )
            .expect("writing to a String");
        }
        generated.push_str("],\n");
    }
    // Artifacts absent from the directory have never been released.
    generated.push_str("        _ => &[],\n    }\n}\n");

    let Some(out_dir) = std::env::var_os("OUT_DIR") else {
        panic!("OUT_DIR is unset; cargo always provides it to a build script")
    };
    let out = Path::new(&out_dir).join("releases.rs");
    std::fs::write(&out, generated).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
}

struct Release {
    artifact: String,
    version: String,
    tag: String,
    asset: String,
    sha256: String,
}

fn parse(path: &Path) -> Release {
    let name = path.display();
    let contents = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{name}: {e}"));

    let row = contents.trim();
    let fields = row.split('\t').collect::<Vec<_>>();
    assert!(
        fields.len() == COLUMNS,
        "{name}: expected {COLUMNS} tab-separated fields, found {}. Columns are \
         artifact, version, tag, asset, sha256.",
        fields.len()
    );

    let release = Release {
        artifact: fields[0].to_owned(),
        version: fields[1].to_owned(),
        tag: fields[2].to_owned(),
        asset: fields[3].to_owned(),
        sha256: fields[4].to_ascii_lowercase(),
    };

    for (field, value) in [
        ("artifact", &release.artifact),
        ("version", &release.version),
        ("tag", &release.tag),
        ("asset", &release.asset),
    ] {
        assert!(!value.is_empty(), "{name}: {field} is empty");
    }
    assert!(
        release.sha256.len() == 64 && release.sha256.chars().all(|c| c.is_ascii_hexdigit()),
        "{name}: sha256 `{}` is not a 64-char hex digest",
        release.sha256,
    );
    assert!(
        version_key(&release.version).is_some(),
        "{name}: version `{}` is not `major.minor.patch`. Releases are ordered \
         by this, and an unparsable one would sort nondeterministically.",
        release.version,
    );

    // The filename is the uniqueness key that makes concurrent catalog PRs
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

/// Orders versions numerically, so `0.10.0` follows `0.9.0`.
///
/// `None` for anything that is not three numeric components. Defaulting instead
/// would let two versions collapse to the same key, and a stable sort would then
/// fall back to `read_dir` order — making the generated catalog, and `current()`
/// with it, differ between machines.
fn version_key(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let key = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(key)
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
