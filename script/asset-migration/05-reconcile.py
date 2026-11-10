#!/usr/bin/env python3
"""Verify every cohort market and all exact-unit settlements before any resume."""
from common import stage_main


def handle(run):
    run.require_stage("04-swap")
    if run.submit:
        run.require_write_admission()
    evidence = run.recheck_reconciliation()
    run.assert_no_unresolved()
    if run.confirmed("05-reconcile", "stage") is None:
        run.save("05-reconcile/report.json", evidence)
        run.confirm("05-reconcile", "stage", evidence)
    return evidence


if __name__ == "__main__":
    stage_main("05-reconcile", handle)
