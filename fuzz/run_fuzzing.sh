#!/usr/bin/env bash
set -euo pipefail

# Run each fuzz target for 2 minutes. The committed seed corpus (seeds/<target>,
# if present) is passed as a read-only extra corpus alongside the evolving
# corpus/<target> — libFuzzer writes new units to the FIRST dir (corpus/), so
# seeds/ never grows here.
cd "$(dirname "$0")"
readonly FUZZ_TOOLCHAIN=nightly-2025-11-25
readonly FUZZ_TARGET=x86_64-unknown-linux-gnu


for t in $(cargo +"$FUZZ_TOOLCHAIN" fuzz list); do
  echo "=== Running $t ==="
  mkdir -p "corpus/$t"
  seed_arg=()
  [ -d "seeds/$t" ] && seed_arg=("seeds/$t")
  cargo +"$FUZZ_TOOLCHAIN" fuzz run --target "$FUZZ_TARGET" "$t" "corpus/$t" "${seed_arg[@]}" -- -max_total_time=120
done
