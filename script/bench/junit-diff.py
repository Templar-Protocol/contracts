#!/usr/bin/env python3
"""Compare two nextest JUnit XML reports and show where the time went.

Usage: junit-diff.py BEFORE.xml AFTER.xml

Prints total wall time, per-suite totals, and the largest per-test deltas, so a
change to the sandbox harness or node config can be shown to help (or not).
`just test-sandbox` writes the report to `target/nextest/sandbox/junit.xml`; copy
it aside before and after a change and pass both here.
"""

import sys
import xml.etree.ElementTree as ET
from collections import defaultdict


def load(path):
    """Return {(suite, test): seconds} and {suite: seconds} from a JUnit file."""
    tests = {}
    suites = defaultdict(float)
    root = ET.parse(path).getroot()
    for suite in root.iter("testsuite"):
        suite_name = suite.get("name", "")
        for case in suite.iter("testcase"):
            # A retried case appears more than once; keep the longest attempt,
            # which is what a run actually waited on.
            seconds = float(case.get("time", "0") or 0)
            key = (suite_name, case.get("name", ""))
            tests[key] = max(tests.get(key, 0.0), seconds)
    for (suite_name, _), seconds in tests.items():
        suites[suite_name] += seconds
    return tests, suites


def fmt(seconds):
    return f"{seconds:8.2f}s"


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    before_tests, before_suites = load(sys.argv[1])
    after_tests, after_suites = load(sys.argv[2])

    before_total = sum(before_tests.values())
    after_total = sum(after_tests.values())
    print("== total ==")
    print(f"  before {fmt(before_total)}")
    print(f"  after  {fmt(after_total)}")
    delta = after_total - before_total
    pct = (delta / before_total * 100) if before_total else 0.0
    print(f"  delta  {fmt(delta)}  ({pct:+.1f}%)")

    print("\n== per suite (after − before) ==")
    suites = sorted(
        set(before_suites) | set(after_suites),
        key=lambda s: after_suites.get(s, 0) - before_suites.get(s, 0),
    )
    for suite in suites:
        b = before_suites.get(suite, 0.0)
        a = after_suites.get(suite, 0.0)
        print(f"  {a - b:+8.2f}s  {fmt(b)} -> {fmt(a)}  {suite}")

    print("\n== 25 largest per-test movers (after − before) ==")
    shared = set(before_tests) & set(after_tests)
    movers = sorted(shared, key=lambda k: after_tests[k] - before_tests[k])
    for key in movers[:25]:
        b, a = before_tests[key], after_tests[key]
        print(f"  {a - b:+8.2f}s  {fmt(b)} -> {fmt(a)}  {key[0]} {key[1]}")

    only_after = set(after_tests) - set(before_tests)
    only_before = set(before_tests) - set(after_tests)
    if only_after:
        print(f"\n  {len(only_after)} test(s) only in AFTER (added/renamed)")
    if only_before:
        print(f"  {len(only_before)} test(s) only in BEFORE (removed/renamed)")


if __name__ == "__main__":
    main()
