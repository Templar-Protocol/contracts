#!/usr/bin/env bash
set -e

source ./prebuild-test-contracts.sh

cargo nextest run "$@"
