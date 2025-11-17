#!/usr/bin/env bash

# Run each fuzz target for 2 minutes
for t in $(cargo +nightly fuzz list); do
  echo "=== Running $t ==="
  cargo +nightly fuzz run "$t" -- -max_total_time=120
done
