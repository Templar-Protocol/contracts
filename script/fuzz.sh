#!/usr/bin/env bash
set -ex

TARGETS=(
  fuzz_borrow
  fuzz_borrow_invariants
  fuzz_decimal_arithmetic
  fuzz_decimal_parsing
  fuzz_decimals
  fuzz_interest_math
  fuzz_liquidations
  fuzz_liquidator_logic
  fuzz_liquidator_transactions
  fuzz_market_creation
  fuzz_price
  fuzz_price_calculations
  fuzz_supply
)

for TARGET in "${TARGETS[@]}"; do
  echo "Running fuzz target: $TARGET"
  cargo +nightly fuzz run "$TARGET"
done
