#!/usr/bin/env bash
# Compare the wall time of the node-backed sandbox *step* across recent CI runs
# of two branches, so the harness speedup is measured on the step that runs the
# tests rather than on whole-job time (which is dominated by compilation).
#
# Usage: ci-step-times.sh [BRANCH_A] [BRANCH_B] [RUNS_PER_BRANCH]
#   defaults: perf/sandbox-gate-speedup  dev  5
#
# Requires `gh` authenticated against the repo. The step name and workflow are
# the ones in .github/workflows/test.yml.
set -euo pipefail

BRANCH_A="${1:-perf/sandbox-gate-speedup}"
BRANCH_B="${2:-dev}"
RUNS="${3:-5}"

WORKFLOW="test.yml"
JOB_NAME="NEAR Integration Tests"
STEP_NAME="Run node-backed tests on a pooled neard sandbox"

# Median of stdin numbers (one per line).
median() {
  sort -n | awk '{ a[NR] = $1 } END {
    if (NR == 0) { print "n/a"; exit }
    if (NR % 2) print a[(NR + 1) / 2]
    else printf "%.1f\n", (a[NR / 2] + a[NR / 2 + 1]) / 2
  }'
}

step_seconds_for_branch() {
  local branch="$1"
  # Successful runs of the workflow on this branch, newest first.
  local run_ids
  run_ids=$(gh run list --workflow "$WORKFLOW" --branch "$branch" \
    --status success --limit "$RUNS" --json databaseId \
    --jq '.[].databaseId')
  [ -z "$run_ids" ] && { echo "  no successful runs for $branch" >&2; return; }

  local seconds=()
  while IFS= read -r run_id; do
    # Duration of the one step, computed from its start/completion timestamps.
    local secs
    secs=$(gh api "repos/{owner}/{repo}/actions/runs/${run_id}/jobs" \
      --jq "
        [.jobs[] | select(.name == \"${JOB_NAME}\")
                 | .steps[] | select(.name == \"${STEP_NAME}\")
                 | (((.completed_at | fromdateiso8601) -
                     (.started_at | fromdateiso8601)))] | first // empty")
    [ -n "$secs" ] && seconds+=("$secs")
  done <<< "$run_ids"

  [ "${#seconds[@]}" -eq 0 ] && { echo "  step not found on $branch" >&2; return; }
  printf '%s\n' "${seconds[@]}" >&2
  echo "  $branch: median $(printf '%s\n' "${seconds[@]}" | median)s over ${#seconds[@]} run(s)"
}

echo "step: \"$STEP_NAME\""
step_seconds_for_branch "$BRANCH_A"
step_seconds_for_branch "$BRANCH_B"
