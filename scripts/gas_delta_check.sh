#!/usr/bin/env bash
# gas_delta_check.sh - Compare current NEAR gas usage against baseline
#
# Usage: ./scripts/gas_delta_check.sh [--threshold PERCENT]
#
# Options:
#   --threshold PERCENT  Maximum allowed increase (default: 10)
#
# Exit codes:
#   0 - Gas delta within threshold
#   1 - Gas delta exceeds threshold
#   2 - Error running gas report

set -euo pipefail

THRESHOLD=${1:-10}
BASELINE_FILE="contract/vault/near/gas_baseline.json"

if [[ "$1" == "--threshold" && -n "${2:-}" ]]; then
    THRESHOLD="$2"
fi

echo "=== NEAR Vault Gas Delta Check ==="
echo "Threshold: ${THRESHOLD}%"
echo

# Check baseline exists
if [[ ! -f "$BASELINE_FILE" ]]; then
    echo "ERROR: Baseline file not found: $BASELINE_FILE"
    exit 2
fi

# Run gas report and capture output
echo "Running gas report (this takes ~2-3 minutes)..."
GAS_OUTPUT=$(cargo run --example gas_report -p templar-vault-contract 2>/dev/null || {
    echo "ERROR: Failed to run gas report"
    exit 2
})

# Parse current gas values from output (format: | `action` | X.X Tgas |)
parse_gas() {
    local action="$1"
    echo "$GAS_OUTPUT" | grep -E "^\| \`$action\`" | sed -E 's/.*\| ([0-9.]+) Tgas.*/\1/'
}

CURRENT_SUPPLY=$(parse_gas "supply")
CURRENT_ALLOCATE=$(parse_gas "allocate")
CURRENT_WITHDRAW=$(parse_gas "withdraw")
CURRENT_EXECUTE=$(parse_gas "execute withdraw")
CURRENT_SUBMIT_CAP=$(parse_gas "submit_cap")

# Parse baseline values
BASELINE_SUPPLY=$(jq -r '.baseline.supply.gas_tgas' "$BASELINE_FILE")
BASELINE_ALLOCATE=$(jq -r '.baseline.allocate.gas_tgas' "$BASELINE_FILE")
BASELINE_WITHDRAW=$(jq -r '.baseline.withdraw.gas_tgas' "$BASELINE_FILE")
BASELINE_EXECUTE=$(jq -r '.baseline.execute_withdraw.gas_tgas' "$BASELINE_FILE")
BASELINE_SUBMIT_CAP=$(jq -r '.baseline.submit_cap.gas_tgas' "$BASELINE_FILE")

# Calculate delta percentage
calc_delta() {
    local current="$1"
    local baseline="$2"
    # (current - baseline) / baseline * 100
    echo "scale=2; ($current - $baseline) / $baseline * 100" | bc
}

# Check if delta exceeds threshold
check_delta() {
    local action="$1"
    local current="$2"
    local baseline="$3"
    local delta=$(calc_delta "$current" "$baseline")
    local abs_delta=${delta#-}  # Absolute value

    local status="OK"
    local exit_code=0

    # Check if increase exceeds threshold
    if (( $(echo "$delta > $THRESHOLD" | bc -l) )); then
        status="FAIL"
        exit_code=1
    elif (( $(echo "$delta < -$THRESHOLD" | bc -l) )); then
        status="IMPROVED"
    fi

    printf "%-20s %8s -> %8s  %+7.1f%%  [%s]\n" "$action" "${baseline} Tgas" "${current} Tgas" "$delta" "$status"
    return $exit_code
}

echo
echo "=== Gas Delta Report ==="
echo
printf "%-20s %12s -> %12s  %10s  [%s]\n" "Action" "Baseline" "Current" "Delta" "Status"
echo "--------------------------------------------------------------------------------"

FAILED=0

check_delta "supply" "$CURRENT_SUPPLY" "$BASELINE_SUPPLY" || FAILED=1
check_delta "allocate" "$CURRENT_ALLOCATE" "$BASELINE_ALLOCATE" || FAILED=1
check_delta "withdraw" "$CURRENT_WITHDRAW" "$BASELINE_WITHDRAW" || FAILED=1
check_delta "execute_withdraw" "$CURRENT_EXECUTE" "$BASELINE_EXECUTE" || FAILED=1
check_delta "submit_cap" "$CURRENT_SUBMIT_CAP" "$BASELINE_SUBMIT_CAP" || FAILED=1

echo
echo "Baseline timestamp: $(jq -r '.timestamp' "$BASELINE_FILE")"
echo "Baseline version: $(jq -r '.version' "$BASELINE_FILE")"
echo

if [[ $FAILED -eq 1 ]]; then
    echo "FAIL: Gas usage exceeds threshold by more than ${THRESHOLD}%"
    echo "Consider updating the baseline if this is expected: gas_baseline.json"
    exit 1
else
    echo "PASS: All gas deltas within ${THRESHOLD}% threshold"
    exit 0
fi
