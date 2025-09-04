#!/usr/bin/env bash
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
ROOT_DIR=$(readlink -f "$SCRIPT_DIR/..")

mkdir -p $ROOT_DIR/_site/{guide,doc}
rm -fr $ROOT_DIR/_site/*

echo "Building guide..."
cd "$ROOT_DIR/docs"
mdbook build
cp -r "$ROOT_DIR/docs/book/html" "$ROOT_DIR/_site/guide"

echo "Building Rust documentation..."
cd $ROOT_DIR
cargo doc --workspace --no-deps
cp -r "$ROOT_DIR/target/doc" "$ROOT_DIR/_site/doc"

echo "docs.templarfi.org" > $ROOT_DIR/_site/CNAME
