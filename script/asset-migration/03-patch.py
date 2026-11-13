#!/usr/bin/env python3
"""Authorize and execute exactly one stamped atomic configuration patch per market."""
import json
from pathlib import Path

from common import require, stage_main, verify_receipts


def relative(run, path):
    return str(Path(path).relative_to(run.root))


def handle(run):
    run.require_stage("02-pause")
    results = []
    for market in run.intent["markets"]:
        account = market["account"]
        done = run.confirmed("03-patch", account)
        if done is not None:
            require(run.read("market.getConfiguration", {"market_id": account}) == market["expected_configuration"],
                    "confirmed market configuration changed")
            require(run.identity(account) == market["account_identity"], "confirmed restore identity changed")
            require(run.market_storage(market) == run.load_snapshot(done["baseline"]), "post-patch market storage drift")
            results.append(done)
            continue
        directory = run.operation_dir("03-patch", account)
        spec = directory / "config-patch.toml"
        plan = directory / "patch-plan.json"
        if not spec.exists():
            run.command(["market", "generate-patch", "config", account, "--changes", str(run.path(market["changes"])), "--out", str(spec)])
        if not plan.exists():
            run.command(["patch", "plan", str(spec), "--out", str(plan), "--signer-id", account,
                         "--public-key", market["public_key"]], json_output=False)
        reviewed = json.loads(plan.read_text())
        if reviewed.get("dry_run") is None:
            run.command(["patch", "dry-run", "--plan", str(plan)], json_output=False)
        run.seal_tree(directory)

        def before_write():
            run.require_paused()
            run.recheck_progress()

        execution = run.mpc("03-patch", account, market, plan,
                            f"migration {run.intent['intent_digest']} atomic market configuration {account}",
                            before_write=before_write)
        verify_receipts(execution["receipts"])
        require(run.read("market.getConfiguration", {"market_id": account}) == market["expected_configuration"], "patch configuration postcondition failed")
        require(run.identity(account) == market["account_identity"], "patch did not restore exact original code/linkage")
        expected = run.load_snapshot(market["snapshot"])
        expected[b"STATE"] = (directory / "config-patch.blobs/new-state.bin").read_bytes()
        actual, baseline = run.snapshot(account, relative(run, directory / "post-patch"))
        require(actual == expected, "patch changed nonconfiguration storage or produced an unexpected STATE")
        evidence = {"account": account, "configuration": market["expected_configuration"],
                    "identity": market["account_identity"], "baseline": baseline,
                    "plan": relative(run, plan), "receipts": execution["receipts"], "mpc": execution}
        run.confirm("03-patch", account, evidence)
        results.append(evidence)
    evidence = {"markets": results, "atomicity": "each market patch only; inventory conversion is a separate transaction"}
    if run.confirmed("03-patch", "stage") is None:
        run.save("03-patch/report.json", evidence)
        run.confirm("03-patch", "stage", evidence)
    return evidence


if __name__ == "__main__":
    stage_main("03-patch", handle)
