#!/usr/bin/env python3
"""Convert each distinct old inventory separately and prove terminal settlement."""
import json

from common import require, stage_main, verify_receipts


def inventory_key(market, role):
    return f"{market['account']}:{role.get('inventory_role', role['role'])}"


def unique_roles(market):
    seen = set()
    for role in market["roles"]:
        key = inventory_key(market, role)
        if key not in seen:
            seen.add(key)
            yield key, role


def expected_new_before(run, market, current_role):
    value = int(current_role["initial_new_balance"])
    for key, role in unique_roles(market):
        if key == inventory_key(market, current_role):
            continue
        done = run.confirmed("04-swap", key)
        if done is not None and role["new_reference"] == current_role["new_reference"]:
            value += int(done["amount"])
    return value


def settlement(receipts, converter, market, role, amount):
    matches = []
    for receipt in receipts["receipts"]:
        if receipt["executor_id"] != converter:
            continue
        for line in receipt["logs"]:
            if not line.startswith("EVENT_JSON:"):
                continue
            event = json.loads(line[len("EVENT_JSON:"):])
            if (event.get("standard"), event.get("version"), event.get("event")) != ("templar_test_asset_converter", "1.0.0", "settlement"):
                continue
            for result in event["data"]:
                if result["route"] == role["route_id"] and result["sender"] == market["account"]:
                    matches.append(result)
    require(len(matches) == 1, "conversion has no unique terminal converter settlement event")
    result = matches[0]
    require(result["recipient"] == market["account"] and result["old_asset"] == role["old_asset"] and
            result["new_asset"] == role["new_asset"], "conversion token or recipient differs")
    require(result["output_succeeded"] is True and int(result["input"]) == amount and
            int(result["consumed"]) == amount and int(result["output"]) == amount and int(result["refunded"]) == 0,
            "conversion did not consume and deliver exact raw units without a refund")
    # The receiver's refund event is only a request. Verify the old token resolver too.
    consumed = []
    for receipt in receipts["receipts"]:
        if receipt["executor_id"] != role["old_reference"]["contract_id"]:
            continue
        returned = receipt.get("return_value")
        if not isinstance(returned, dict) or returned.get("encoding") != "json":
            continue
        value = returned["value"]
        if isinstance(value, list):
            if len(value) != 1:
                continue
            value = value[0]
        if isinstance(value, str) and value.isdecimal():
            consumed.append(int(value))
    require(amount in consumed, "old token resolver did not confirm complete input consumption")
    return result


def handle(run):
    run.require_stage("03-patch")
    run.recheck_metadata()
    results = []
    for market in run.intent["markets"]:
        patched = run.confirmed("03-patch", market["account"])
        require(patched is not None, "market patch has no domain postcondition evidence")
        for key, role in unique_roles(market):
            done = run.confirmed("04-swap", key)
            if done is not None:
                results.append(done)
                continue
            directory = run.operation_dir("04-swap", key)
            frozen_path = directory / "inventory.json"
            if frozen_path.exists():
                frozen = run.load(str(frozen_path.relative_to(run.root)))
            else:
                run.require_paused()
                require(run.read("market.getConfiguration", {"market_id": market["account"]}) == market["expected_configuration"], "pre-conversion config drift")
                require(run.identity(market["account"]) == market["account_identity"], "pre-conversion code/linkage drift")
                require(run.market_storage(market) == run.load_snapshot(patched["baseline"]), "pre-conversion market storage drift")
                old_before = run.balance(role["old_reference"], market["account"])
                new_before = run.balance(role["new_reference"], market["account"])
                require(old_before == int(role["initial_old_balance"]), "old inventory changed outside the frozen campaign")
                require(new_before == expected_new_before(run, market, role), "new inventory changed outside confirmed conversions")
                frozen = {"account": market["account"], "role": role["role"], "inventory_role": role["inventory_role"],
                          "old_reference": role["old_reference"], "new_reference": role["new_reference"],
                          "route_id": role["route_id"], "amount": str(old_before), "old_before": str(old_before),
                          "new_before": str(new_before), "expected_refund": "0", "expected_output": str(old_before),
                          "converter_code_sha256": run.campaign["converter_code_sha256"]}
                run.save(str(frozen_path.relative_to(run.root)), frozen)
            amount = int(frozen["amount"])
            if amount == 0:
                require(run.balance(role["old_reference"], market["account"]) == 0 and
                        run.balance(role["new_reference"], market["account"]) == int(frozen["new_before"]), "zero inventory checkpoint drift")
                evidence = {**frozen, "old_after": "0", "new_after": frozen["new_before"], "zero_inventory": True, "receipts": None}
            else:
                plan = run.plan("token.transferCall", {"token": role["old_reference"], "receiver_id": run.campaign["converter_account"],
                                "amount": frozen["amount"], "msg": json.dumps({"route": role["route_id"]}, separators=(",", ":"))},
                                market["account"], directory / "conversion.plan.json")

                def before_write():
                    run.require_paused()
                    run.recheck_progress()
                    require(run.balance(role["old_reference"], market["account"]) == int(frozen["old_before"]) and
                            run.balance(role["new_reference"], market["account"]) == int(frozen["new_before"]), "conversion inventory drift")

                execution = run.mpc("04-swap", key, market, plan,
                                    f"migration {run.intent['intent_digest']} inventory {key}", before_write=before_write)
                receipts = execution["receipts"]
                verify_receipts(receipts)
                # Plain output delivery must not become a new market supply/collateral deposit.
                require(run.market_storage(market) == run.load_snapshot(patched["baseline"]), "conversion credited or otherwise mutated market storage")
                old_after = run.balance(role["old_reference"], market["account"])
                new_after = run.balance(role["new_reference"], market["account"])
                require(old_after == 0 and new_after == int(frozen["new_before"]) + amount, "conversion balance conservation failed")
                event = settlement(receipts, run.campaign["converter_account"], market, role, amount)
                evidence = {**frozen, "old_after": str(old_after), "new_after": str(new_after), "zero_inventory": False,
                            "receipts": receipts, "settlement": event, "mpc": execution}
            run.confirm("04-swap", key, evidence)
            results.append(evidence)
    current = run.recheck_reconciliation()
    evidence = {"inventories": results, "current": current, "rollback": "none; successful prior swaps remain terminal"}
    if run.confirmed("04-swap", "stage") is None:
        run.save("04-swap/report.json", evidence)
        run.confirm("04-swap", "stage", evidence)
    return evidence


if __name__ == "__main__":
    stage_main("04-swap", handle)
