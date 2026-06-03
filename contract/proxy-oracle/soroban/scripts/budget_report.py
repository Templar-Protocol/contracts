#!/usr/bin/env python3
"""Run deterministic proxy-oracle Soroban budget scenarios and write a report.

Scenarios covered (via soroban-sdk testutils):
  1. accepted_refresh   - refresh with valid source prices succeeds
  2. blocked_refresh    - refresh when manual trip is set returns Blocked
  3. manual_trip        - set_manual_trip blocks subsequent reads
  4. config_update      - set_proxy updates proxy configuration
  5. governance_accept  - governance submit + accept cycle (via governance-contract tests)

Full Stellar resource simulation (CPU/memory instruction counts against a live
Soroban network) is NOT available locally. soroban-sdk testutils execute contract
logic in a sandboxed environment without the Stellar RPC simulation pipeline, so
precise ledger-resource figures cannot be obtained offline.

This script:
  - Runs the cargo test suites for both contracts with --nocapture
  - Captures pass/fail status per test and uses test names as scenario proxies
  - Writes a structured report to the output path

To obtain real resource budgets, run:
  stellar contract simulate --network testnet --source <IDENTITY> --wasm <WASM>

Limitation is noted in the evidence file.
"""
import argparse
import subprocess
import sys
import re
from datetime import datetime, timezone
from pathlib import Path


SCENARIOS = {
    "accepted_refresh": "refresh_updates_sep40_lastprice",
    "blocked_refresh": "manual_trip_blocks_refresh_and_cached_read",
    "manual_trip": "manual_trip_blocks_refresh_and_cached_read",
    "config_update": "set_proxy_config",
    "governance_accept": "governance",
}


def run_cargo_test(manifest_root: Path, package: str) -> tuple[bool, str]:
    """Run `cargo test -p <package> -- --nocapture` and return (ok, output)."""
    result = subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            package,
            "--features",
            "testutils",
            "--",
            "--nocapture",
        ],
        cwd=str(manifest_root),
        capture_output=True,
        text=True,
    )
    combined = result.stdout + result.stderr
    return result.returncode == 0, combined


def extract_test_results(output: str) -> dict[str, str]:
    """Parse cargo test output into {test_name: 'ok' | 'FAILED'}."""
    results: dict[str, str] = {}
    for line in output.splitlines():
        m = re.search(r"test (\S+) \.\.\. (ok|FAILED)", line)
        if m:
            results[m.group(1)] = m.group(2)
    return results


def map_scenario(
    pattern: str,
    test_results: dict[str, str],
) -> tuple[str, str]:
    """Return (matched_test_name | '<not found>', status)."""
    for name, status in test_results.items():
        if pattern in name:
            return name, status
    return "<not found>", "SKIPPED"


def main() -> None:
    parser = argparse.ArgumentParser(description="Proxy-oracle Soroban budget report")
    parser.add_argument(
        "--root",
        required=True,
        help="Workspace root (Cargo.toml directory)",
    )
    parser.add_argument(
        "--out",
        required=True,
        help="Output path for the report file",
    )
    args = parser.parse_args()

    manifest_root = Path(args.root)
    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)

    timestamp = datetime.now(timezone.utc).isoformat()

    lines: list[str] = []
    lines.append("# Proxy-Oracle Soroban Budget Report")
    lines.append(f"generated: {timestamp}")
    lines.append("")
    lines.append("## Limitation Notice")
    lines.append(
        "Full Stellar resource simulation (ledger CPU/memory instructions) is not "
        "available without a live Soroban RPC endpoint. This report uses soroban-sdk "
        "testutils to verify deterministic scenario correctness. For precise budget "
        "figures run: stellar contract simulate --network testnet --source <IDENTITY> "
        "--wasm <WASM_PATH>"
    )
    lines.append("")

    all_ok = True

    # --- Runtime tests ---
    lines.append("## Runtime Scenarios (templar-proxy-oracle-soroban-contract)")
    print("Running runtime tests...", flush=True)
    runtime_ok, runtime_output = run_cargo_test(
        manifest_root, "templar-proxy-oracle-soroban-contract"
    )
    runtime_results = extract_test_results(runtime_output)

    if not runtime_ok and not runtime_results:
        lines.append("  cargo test FAILED to run:")
        lines.append(f"  {runtime_output[:500]}")
        all_ok = False
    else:
        runtime_scenarios = {
            "accepted_refresh": "refresh_updates_sep40_lastprice",
            "blocked_refresh": "manual_trip_blocks_refresh_and_cached_read",
            "manual_trip": "manual_trip_blocks",
            "config_update": "event_proxy_set",
        }
        for scenario, pattern in runtime_scenarios.items():
            matched, status = map_scenario(pattern, runtime_results)
            ok_marker = "PASS" if status == "ok" else ("SKIP" if status == "SKIPPED" else "FAIL")
            if ok_marker == "FAIL":
                all_ok = False
            lines.append(f"  [{ok_marker}] {scenario:<25} -> {matched} ({status})")

    lines.append("")
    lines.append(f"  All runtime test results ({len(runtime_results)} tests):")
    for name, status in sorted(runtime_results.items()):
        lines.append(f"    {'ok' if status == 'ok' else 'FAIL':4}  {name}")

    # --- Governance tests ---
    lines.append("")
    lines.append(
        "## Governance Scenarios (templar-proxy-oracle-soroban-governance-contract)"
    )
    print("Running governance tests...", flush=True)
    gov_ok, gov_output = run_cargo_test(
        manifest_root, "templar-proxy-oracle-soroban-governance-contract"
    )
    gov_results = extract_test_results(gov_output)

    if not gov_ok and not gov_results:
        lines.append("  cargo test FAILED to run:")
        lines.append(f"  {gov_output[:500]}")
        all_ok = False
    else:
        gov_scenarios = {
            "governance_accept": "execute_through_governance",
        }
        for scenario, pattern in gov_scenarios.items():
            matched, status = map_scenario(pattern, gov_results)
            ok_marker = "PASS" if status == "ok" else ("SKIP" if status == "SKIPPED" else "FAIL")
            if ok_marker == "FAIL":
                all_ok = False
            lines.append(f"  [{ok_marker}] {scenario:<25} -> {matched} ({status})")

    lines.append("")
    lines.append(f"  All governance test results ({len(gov_results)} tests):")
    for name, status in sorted(gov_results.items()):
        lines.append(f"    {'ok' if status == 'ok' else 'FAIL':4}  {name}")

    # --- Summary ---
    lines.append("")
    lines.append("## Summary")
    lines.append(
        f"  runtime  tests: {'PASS' if runtime_ok else 'FAIL'} "
        f"({sum(1 for s in runtime_results.values() if s == 'ok')}/{len(runtime_results)} passed)"
    )
    lines.append(
        f"  governance tests: {'PASS' if gov_ok else 'FAIL'} "
        f"({sum(1 for s in gov_results.values() if s == 'ok')}/{len(gov_results)} passed)"
    )
    lines.append(f"  overall: {'PASS' if all_ok and runtime_ok and gov_ok else 'FAIL'}")

    report = "\n".join(lines) + "\n"
    out_path.write_text(report)
    print(f"Report written: {out_path}", flush=True)
    print(report)

    if not (all_ok and runtime_ok and gov_ok):
        sys.exit(1)


if __name__ == "__main__":
    main()
