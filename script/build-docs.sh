#!/usr/bin/env bash
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
ROOT_DIR=$(readlink -f "$SCRIPT_DIR/..")

rm -rf $ROOT_DIR/_site
mkdir -p $ROOT_DIR/_site/{guide,doc}

echo "Building guide..."
cd "$ROOT_DIR/docs"
mdbook build --dest-dir $ROOT_DIR/_site/guide

echo "Building Rust documentation..."
cd $ROOT_DIR
cargo doc --workspace --no-deps
cp -r $ROOT_DIR/target/doc $ROOT_DIR/_site

cp -r $ROOT_DIR/docs/static/* $ROOT_DIR/_site
