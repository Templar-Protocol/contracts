#!/usr/bin/env python3
"""Register only required token recipients through ordinary DAO proposals."""
from common import registration_funded, require, stage_main, verify_receipts


def handle(run):
    run.require_stage("00-prepare")
    if run.submit:
        run.require_write_admission()
    results = []
    for registration in run.intent["registrations"]:
        contract, account = registration["contract_id"], registration["account_id"]
        key = f"{contract}:{account}"
        live = run.read("storage.getBalanceOf", {"contract_id": contract, "account_id": account})
        prior = run.confirmed("01-register", key)
        if prior is not None:
            require(registration_funded(registration, live), "confirmed registration funding disappeared")
            results.append(prior)
            continue
        submitted = any(event["stage"] == "01-register" and event["key"] == key
                        and event["operation"] == "dao"
                        and event["event"] in ("attempt_started", "submitted", "unknown", "confirmed")
                        for event in run.events)
        if registration_funded(registration, live) and not submitted:
            evidence = {"contract_id": contract, "account_id": account,
                        "already_registered": live is not None, "funding_required": False,
                        "minimum": registration["minimum"], "balance": live}
        else:
            directory = run.operation_dir("01-register", key)
            plan = run.plan("storage.ensureDeposit", {"contract_id": contract, "account_id": account,
                            "mode": {"mode": "minimum_total", "amount": registration["minimum"]}},
                            run.campaign["dao"], directory / "registration.plan.json")

            def before_write():
                run.require_write_admission()
                run.recheck_progress()

            def verify_execution(result):
                balances = [receipt.get("return_value") for receipt in result["receipts"]
                            if receipt["executor_id"] == contract]
                require(any(isinstance(value, dict) and value.get("encoding") == "json"
                            and isinstance(value.get("value"), dict)
                            and isinstance(value["value"].get("total"), str)
                            and value["value"]["total"].isdecimal()
                            and int(value["value"]["total"]) >= int(registration["minimum"])
                            for value in balances),
                        "final vote did not execute the expected storage registration")

            execution = run.ordinary("01-register", key, plan, f"migration {run.intent['intent_digest']} register {account} on {contract}",
                                     before_write=before_write, verify_execution=verify_execution)
            verify_receipts(execution["receipts"])
            live = run.read("storage.getBalanceOf", {"contract_id": contract, "account_id": account})
            require(registration_funded(registration, live), "registration did not settle")
            evidence = {"contract_id": contract, "account_id": account, "already_registered": False,
                        "funding_required": True, "minimum": registration["minimum"],
                        "balance": live, "execution": execution}
        run.confirm("01-register", key, evidence)
        results.append(evidence)
    evidence = {"registrations": results, "converter_receiver_registration": "not required by pinned local profile"}
    if run.confirmed("01-register", "stage") is None:
        run.save("01-register/report.json", evidence)
        run.confirm("01-register", "stage", evidence)
    return evidence


if __name__ == "__main__":
    stage_main("01-register", handle)
