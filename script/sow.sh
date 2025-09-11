#!/usr/bin/env bash

# Requires https://github.com/AlDanial/cloc

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && cd .. && pwd)"
REL_DIR=$(realpath -s --relative-to="${PWD}" "$ROOT_DIR")

cloc \
    $REL_DIR/common \
    $REL_DIR/contract \
    $REL_DIR/contract/lst-oracle \
    $REL_DIR/contract/market \
    $REL_DIR/contract/registry \
    --exclude-dir=tests \
    --not-match-f=tests.rs \
    --counted=sow.txt
