#!/usr/bin/env bash
# Cut a new immutable release blob for one contract artifact.
#
# Reproducibly builds the artifact from the CURRENT COMMIT and installs the
# bytes at contract/artifacts/res/near/<target>/<version>/<target>.wasm, then
# prints the catalog entry to paste into contract/artifacts/src/ids.rs.
#
# Released blobs are immutable: this refuses to overwrite an existing version.
# To ship new bytes, bump the crate version first, then run this.
#
#   ./script/artifact-release.sh proxy-oracle
set -euo pipefail

ARTIFACT="${1:-}"
if [[ -z "$ARTIFACT" ]]; then
    echo "usage: $0 <artifact-id>   (e.g. proxy-oracle; see ArtifactId::ALL)" >&2
    exit 2
fi

SCRIPT_DIR=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

# `cargo near build reproducible-wasm` builds from committed git state. On a
# dirty tree it would either hard-error or embed a state that does not match
# what is checked in, producing bytes nobody can reproduce.
if [[ -n "$(git status --porcelain)" ]]; then
    echo "error: working tree is dirty." >&2
    echo "Reproducible builds use committed git state — commit or stash first." >&2
    exit 1
fi

# Resolve catalog metadata for the artifact (source path, target name) and the
# crate's current version, straight from the single source of truth.
METADATA=$(cargo run --quiet -p templar-contract-artifacts \
    --features workspace-loader,clap --bin prebuild-test-contracts -- \
    --print-metadata --artifact "$ARTIFACT") || {
    echo "error: unknown artifact '$ARTIFACT'" >&2
    exit 1
}
read -r SOURCE_PATH TARGET VERSION <<<"$METADATA"
DEST_DIR="contract/artifacts/res/near/$TARGET/$VERSION"
if [[ -e "$DEST_DIR/$TARGET.wasm" ]]; then
    echo "error: $TARGET $VERSION is already released ($DEST_DIR/$TARGET.wasm)." >&2
    echo "Released blobs are immutable — bump the crate version to ship new bytes." >&2
    exit 1
fi

COMMIT=$(git rev-parse HEAD)
echo ">> building $ARTIFACT ($TARGET) $VERSION reproducibly from $COMMIT"
cargo near build reproducible-wasm --manifest-path "$SOURCE_PATH/Cargo.toml"

mkdir -p "$DEST_DIR"
cp "target/near/$TARGET/$TARGET.wasm" "$DEST_DIR/$TARGET.wasm"
SHA=$(sha256sum "$DEST_DIR/$TARGET.wasm" | cut -d' ' -f1)

cat <<EOF

>> installed $DEST_DIR/$TARGET.wasm

Add this release to the $ARTIFACT entry in contract/artifacts/src/ids.rs
(append to the release list — newest last — and add a matching
\`embedded_bytes_for_version\` arm):

        ("$VERSION", "$SHA", Some("$COMMIT")),

…and a matching arm in \`embedded_bytes_for_version\`:

        (Self::<Variant>, "$VERSION") => include_bytes!("../res/near/$TARGET/$VERSION/$TARGET.wasm"),

Then verify and commit the blob together with the catalog edit:

    ./script/check-artifact-drift.sh
EOF
