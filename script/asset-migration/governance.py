"""Fixed DAO -> proxy governance manual-trip lifecycle; never votes or signs directly."""
from __future__ import annotations

import base64
import json
import time

from common import Stop, canonical, require, verify_receipts


def feed_key(feed):
    return f"{feed['oracle']}:{feed['feed']}"


def feeds(run):
    unique = {}
    for feed in run.intent["feeds"]:
        key = feed_key(feed)
        require(key not in unique or unique[key] == feed, f"conflicting cohort feed: {key}")
        unique[key] = feed
    return list(unique.values())


def relative(run, path):
    return str(path.relative_to(run.root))


def saved(run, path):
    return run.load(relative(run, path)) if path.exists() else None


def context_requests(run, feed):
    oracle, governance = feed["oracle"], feed["governance"]
    return [
        ("owner.getOwner", {"contract_id": oracle}),
        ("proxyOracleGovernance.getProxyOracleId", {"governance_id": governance}),
        ("proxyOracleGovernance.getGovernancePolicy", {"governance_id": governance}),
        ("proxyOracleGovernance.getRoles", {"governance_id": governance, "account_id": run.campaign["dao"]}),
        ("proxyOracle.getProxyCircuitBreakerSet", {"oracle_id": oracle, "id": feed["feed"]}),
        ("proxyOracle.getProxy", {"oracle_id": oracle, "id": feed["feed"]}),
    ]


def context(run, feed):
    return context_from_reads(feed, run.read_many(context_requests(run, feed)))


def manual_trip_policy(policy):
    return policy["method_policies"].get("admin_set_manual_trip", policy["default_target"])


def context_from_reads(feed, values):
    oracle, governance = feed["oracle"], feed["governance"]
    owner, target, policy, roles, breaker, proxy = values
    require(owner == governance, f"oracle owner changed: {oracle}")
    require(target == oracle, f"governance target mismatch: {governance}")
    method_policy = manual_trip_policy(policy)
    require("Admin" in roles or method_policy["role"] in roles,
            f"DAO lacks the deployed manual-trip governance role: {governance}")
    require(isinstance(breaker, dict) and isinstance(proxy, dict), f"missing feed context: {feed_key(feed)}")
    return {"breaker": breaker, "proxy": proxy, "policy": policy, "roles": roles,
            "owner": governance, "target": oracle}


def price_age(run, feed):
    ages = []
    configurations = [market["expected_configuration"] for market in run.intent["markets"]]
    configurations.extend(consumer["configuration"] for consumer in run.intent.get("dependency_consumers", []))
    for configuration in configurations:
        config = configuration["price_oracle_configuration"]
        if config["account_id"] == feed["oracle"] and feed["feed"] in (
                config["borrow_asset_price_id"], config["collateral_asset_price_id"]):
            ages.append(int(config["price_maximum_age_s"]))
    require(bool(ages), f"no scoped market freshness bound for {feed_key(feed)}")
    return min(ages)


def fresh_price(run, price, age, label):
    require(isinstance(price, dict) and int(price["price"]) > 0, f"missing/nonpositive {label}")
    now = int(run.read("chain.getBlock", {})["timestamp_ns"])
    published = int(price["publish_time"]) * 1_000_000_000
    require(0 <= now - published <= age * 1_000_000_000, f"stale/future {label}")
    return {"price": price, "checked_at_ns": str(now), "maximum_age_s": age}


def source_ready(run, feed):
    age = price_age(run, feed)
    params = {"oracle_id": feed["oracle"], "id": feed["feed"], "age": age}
    price = run.read("proxyOracle.getFreshSourcePrice", params)
    return {"resolution": params,
            **fresh_price(run, price, age, f"all-source readiness for {feed_key(feed)}")}



def cached_price_request(run, feed):
    return ("proxyOracle.getCachedPrice", {"oracle_id": feed["oracle"], "id": feed["feed"],
                                           "age": price_age(run, feed)})


def cached_price(run, feed):
    return run.read(*cached_price_request(run, feed))


def require_no_automatic_trip(breaker):
    for value in breaker["breakers"].values():
        status = value["status"]
        require(not (value["is_enforced"] and isinstance(status, dict) and "Tripped" in status),
                "active automatic breaker; operator inspection required")


def same_incident(actual, expected):
    # Histories are incident context too: even legitimate updates require inspection.
    require(actual == expected, "pause/incident context changed; operator inspection required")


def frozen_definition(run, feed):
    live = context(run, feed)
    require(live["proxy"] == feed["initial_proxy"]
            and live["policy"] == feed["governance_policy"]
            and live["roles"] == feed["governance_roles"],
            f"frozen proxy/governance definition changed: {feed_key(feed)}")
    return live


def refreshed_context(run, feed):
    live = frozen_definition(run, feed)
    # Refresh may append observations; it cannot reconfigure, rearm, or clear an
    # existing incident. Newly tripped breakers also require operator inspection.
    unchanged = dict(live["breaker"])
    for history in ("accepted_history", "observed_history"):
        unchanged[history] = dict(unchanged[history], entries=feed["initial_breaker"][history]["entries"])
    same_incident(unchanged, feed["initial_breaker"])
    return live


def check_oracle_contexts(run, expected):
    for oracle, checkpoint in expected.items():
        same_incident(run.oracle_context(oracle), checkpoint)


def predict_manual_trip(run, feed, tripped, metadata, checkpoint):
    """Freeze the shared native transition, never a post-approval observation."""
    argv = ["proxy-oracle", "predict-manual-trip", "--oracle-id", feed["oracle"],
            "--price-id", feed["feed"], "--actor", feed["governance"],
            "--metadata-base64", metadata]
    if tripped:
        argv.append("--tripped")
    prediction = run.command(argv)
    require(prediction["oracle_id"] == feed["oracle"], "manual-trip prediction names another oracle")
    for field in ("storage_digest", "storage_entries"):
        require(prediction["before"][field] == checkpoint[field],
                "oracle changed before the frozen manual-trip transition; operator inspection required")
    require(prediction["effective_code_sha256"] == checkpoint["effective_code_sha256"],
            "oracle code changed before the frozen manual-trip transition")
    return {"before": checkpoint, "after": dict(checkpoint, **prediction["after"]),
            "prediction": prediction}


def decoded_call(plan, receiver, method):
    require(plan["receiver_id"] == receiver and len(plan["actions"]) == 1,
            "unexpected specialized governance plan target/action count")
    require(not plan.get("continue_on_failure", False), "governance plan continues on failure")
    action = plan["actions"][0]
    require(set(action) == {"FunctionCall"}, "governance plan is not a FunctionCall")
    call = action["FunctionCall"]
    require(call["method_name"] == method, "unexpected specialized governance method")
    return json.loads(base64.b64decode(call["args"], validate=True))


def event_data(result, executor, standard, event):
    verify_receipts(result)
    found = []
    for receipt in result["receipts"]:
        if receipt["executor_id"] != executor:
            continue
        for line in receipt["logs"]:
            if not line.startswith("EVENT_JSON:"):
                continue
            value = json.loads(line[len("EVENT_JSON:"):])
            if value.get("standard") == standard and value.get("event") == event:
                require(isinstance(value.get("data"), dict), "contract event data must be an object")
                found.append(value["data"])
    return found


def manual_event(result, feed, tripped, metadata):
    matches = [entry for entry in event_data(result, feed["oracle"], "templar-proxy-oracle", "circuit_breaker_manual_trip_set")
               if entry.get("price_id") == feed["feed"]]
    require(matches == [{"price_id": feed["feed"], "is_manually_tripped": tripped,
                         "actor": feed["governance"], "metadata": metadata}],
            "target did not emit the exact campaign manual-trip event")
    return matches[0]


def governance_event(result, feed, proposal_id, event):
    expected = {"id": proposal_id, "kind": "TargetFunctionCall", "method": "admin_set_manual_trip"}
    require(event_data(result, feed["governance"], "templar-governance", event) == [expected],
            f"governance receipts do not prove exact {event} ID/kind/method")


def manual_execution(result, feed, proposal_id, tripped, metadata):
    governance_event(result, feed, proposal_id, "executed")
    return manual_event(result, feed, tripped, metadata)


def refresh_execution(result, oracle, group):
    """The proxy emits no success event: its callback returns the accepted-cache map."""
    verify_receipts(result)
    expected = {feed["feed"] for feed in group}
    matches = []
    for receipt in result["receipts"]:
        returned = receipt.get("return_value")
        if receipt["executor_id"] != oracle or not isinstance(returned, dict) or returned.get("encoding") != "json":
            continue
        value = returned["value"]
        if not isinstance(value, dict) or set(value) != expected:
            continue
        for status in value.values():
            require(isinstance(status, dict) and set(status) == {"Accepted"},
                    "refresh did not return an accepted cache result for every requested feed")
            accepted = status["Accepted"]
            require(isinstance(accepted, dict) and set(accepted) == {"price"}, "malformed accepted-cache result")
            price = accepted["price"]
            require(isinstance(price, dict) and set(price) == {"price", "conf", "expo", "publish_time_ns"}
                    and int(price["price"]) > 0 and int(price["publish_time_ns"]) >= 0,
                    "refresh returned a malformed/nonpositive accepted price")
        matches.append(value)
    require(len(matches) == 1, "execution lacks the exact oracle accepted-cache callback return")
    return matches[0]


def governance_proposal(run, feed, proposal_id):
    return run.read("proxyOracleGovernance.getProposal", {"governance_id": feed["governance"], "id": proposal_id})


def manual_trip(run, stage, feed, tripped, before_write, oracle_checkpoint):
    """Two independent ordinary DAO approvals with one pinned governance ID/body."""
    key = feed_key(feed)
    directory = run.operation_dir(stage, key)
    spec_path = directory / "governance.json"
    spec = saved(run, spec_path)
    if spec is None:
        live = context(run, feed)
        proposal_id = run.read("proxyOracleGovernance.nextProposalId", {"governance_id": feed["governance"]})
        metadata = base64.b64encode(canonical({"schema": 1, "intent_digest": run.intent["intent_digest"],
                                              "stage": stage, "oracle": feed["oracle"], "feed": feed["feed"],
                                              "tripped": tripped})).decode("ascii")
        transition = predict_manual_trip(run, feed, tripped, metadata, oracle_checkpoint)
        argv = ["proxy-oracle", "governance", "create-proposal", "--governance-id", feed["governance"],
                "--id", str(proposal_id), "--requested-ttl", "0ns"]
        operation_args = ["oracle", "set-manual-trip", "--price-id", feed["feed"], "--metadata-base64", metadata]
        if tripped:
            operation_args.append("--tripped")
        plan_path = run.plan_cmd(argv, run.campaign["dao"], directory / "create.plan.json",
                                 subcommand=operation_args)
        args = decoded_call(run.load(relative(run, plan_path)), feed["governance"], "create_proposal")
        require(args["id"] == proposal_id, "planned governance ID differs")
        operation = args["operation"]
        require(set(operation) == {"TargetFunctionCall"}, "manual-trip plan is not an oracle target call")
        target = operation["TargetFunctionCall"]
        require(target["method_name"] == "admin_set_manual_trip", "manual-trip plan names a different target method")
        target_args = json.loads(base64.b64decode(target["args"], validate=True))
        require(target_args == {"id": feed["feed"], "is_manually_tripped": tripped, "metadata": metadata},
                "specialized plan does not carry the exact campaign trip/feed/metadata")
        spec = {"governance_id": proposal_id, "operation": args["operation"], "metadata": metadata,
                "requested_ttl": args["requested_ttl"], "initial_context": live,
                "create_plan": relative(run, plan_path), "oracle_transition": transition}
        run.save(relative(run, spec_path), spec)
        run.record(stage, key, "prepared", {"governance": spec})
    require("oracle_transition" in spec,
            "manual operation lacks a frozen causal oracle transition; operator inspection required")
    transition = spec["oracle_transition"]
    context(run, feed)
    create_key, execute_key = key + ":create", key + ":execute"
    created_path = directory / "created.json"
    created = saved(run, created_path)
    if created is None:
        existing = governance_proposal(run, feed, spec["governance_id"])
        if existing is not None:
            require(existing["operation"] == spec["operation"] and existing["created_by"] == run.campaign["dao"],
                    "governance ID collision; refusing occupied proposal")
        def check_create():
            same_incident(context(run, feed), spec["initial_context"])
            require(governance_proposal(run, feed, spec["governance_id"]) is None,
                    "governance proposal ID already occupied before creation")
            require(run.read("proxyOracleGovernance.nextProposalId", {"governance_id": feed["governance"]}) == spec["governance_id"],
                    "governance proposal ID consumed; a new inspected attempt is required")
            before_write()

        creation = run.ordinary(stage, create_key, run.path(spec["create_plan"]),
                                f"migration {run.intent['intent_digest']} {stage} create {key}",
                                before_write=check_create,
                                verify_execution=lambda result: governance_event(
                                    result, feed, spec["governance_id"], "created"))
        proposal = governance_proposal(run, feed, spec["governance_id"])
        require(isinstance(proposal, dict), "created governance proposal missing or consumed before verification")
        require(proposal["operation"] == spec["operation"] and proposal["created_by"] == run.campaign["dao"],
                "created governance proposal differs from frozen body/creator")
        method_policy = manual_trip_policy(spec["initial_context"]["policy"])
        require(int(proposal["ttl"]) >= max(int(spec["requested_ttl"]), int(method_policy["ttl"])),
                "effective governance TTL below the frozen deployed policy bound")
        created = {"proposal": proposal, "creation": creation}
        run.save(relative(run, created_path), created)
    execute_path = directory / "execution.json"
    executed = saved(run, execute_path)
    if executed is None:
        same_incident(oracle_checkpoint, transition["before"])
        proposal = governance_proposal(run, feed, spec["governance_id"])
        if proposal is not None:
            require(proposal == created["proposal"], "governance proposal body/TTL/creator changed")
            ready_at = int(proposal["created_at"]) + int(proposal["ttl"])
            deadline = time.monotonic() + (int(run.campaign["approval_wait_seconds"]) if run.wait else 0)
            while int(run.read("chain.getBlock", {})["timestamp_ns"]) < ready_at:
                if time.monotonic() >= deadline:
                    run.record(stage, key, "waiting", {"governance_id": spec["governance_id"], "ready_at_ns": str(ready_at)})
                    raise Stop(10, f"governance timelock pending for {key}")
                time.sleep(min(1.0, max(0.0, deadline - time.monotonic())))
        # If the proposal disappeared, only the durable ordinary lifecycle may reconcile
        # an already executed transaction; it must never infer success from consumption.
        plan_path = run.plan_cmd(["proxy-oracle", "governance", "execute-proposal", "--governance-id",
                                  feed["governance"], "--id", str(spec["governance_id"])],
                                 run.campaign["dao"], directory / "execute.plan.json")
        args = decoded_call(run.load(relative(run, plan_path)), feed["governance"], "execute_proposal")
        require(args == {"id": spec["governance_id"]}, "execution plan ID differs")
        def check_execute():
            same_incident(context(run, feed), spec["initial_context"])
            actual = governance_proposal(run, feed, spec["governance_id"])
            require(actual == created["proposal"], "governance proposal disappeared or changed before execution")
            require(int(run.read("chain.getBlock", {})["timestamp_ns"]) >=
                    int(actual["created_at"]) + int(actual["ttl"]), "effective on-chain timelock not elapsed")
            before_write()

        def verify_execution(result):
            manual_execution(result, feed, spec["governance_id"], tripped, spec["metadata"])
            # Successful own receipts do not explain intervening foreign changes.
            # Compare the full predicted state, including every feed's cache epoch,
            # before ordinary() may confirm this late successful execution.
            same_incident(run.oracle_context(feed["oracle"]), transition["after"])

        execution = run.ordinary(stage, execute_key, plan_path,
                                 f"migration {run.intent['intent_digest']} {stage} execute {key}",
                                 before_write=check_execute, verify_execution=verify_execution)
        event = manual_execution(execution["receipts"], feed, spec["governance_id"], tripped, spec["metadata"])
        require(governance_proposal(run, feed, spec["governance_id"]) is None, "governance proposal not consumed")
        executed = {"governance_id": spec["governance_id"], "operation": spec["operation"],
                    "created": created, "execution": execution, "manual_event": event,
                    "oracle_context": transition["after"]}
        run.save(relative(run, execute_path), executed)
    governance_event(created["creation"]["receipts"], feed, spec["governance_id"], "created")
    manual_execution(executed["execution"]["receipts"], feed, spec["governance_id"], tripped, spec["metadata"])
    same_incident(executed["oracle_context"], transition["after"])
    return executed


def manual_checkpoints(run, stage, initial):
    """Advance only from retained, event-proven target executions, in cohort order."""
    checkpoints = dict(initial)
    for feed in feeds(run):
        path = run.operation_dir(stage, feed_key(feed)) / "execution.json"
        execution = saved(run, path)
        if execution is None:
            continue
        spec = run.load(relative(run, path.parent / "governance.json"))
        require(execution["governance_id"] == spec["governance_id"]
                and execution["operation"] == spec["operation"], "manual checkpoint governance binding differs")
        manual_execution(execution["execution"]["receipts"], feed, spec["governance_id"],
                         stage == "02-pause", spec["metadata"])
        require("oracle_transition" in spec,
                "manual checkpoint lacks a frozen causal transition; operator inspection required")
        transition = spec["oracle_transition"]
        same_incident(transition["before"], checkpoints[feed["oracle"]])
        same_incident(execution["oracle_context"], transition["after"])
        checkpoints[feed["oracle"]] = execution["oracle_context"]
    return checkpoints


def certify_pause(run, pause):
    initial_path = "02-pause/initial-oracle-contexts.json"
    require(run.path(initial_path).exists(), "pause lacks its initial causal checkpoint")
    checkpoints = manual_checkpoints(run, "02-pause", run.load(initial_path))
    same_incident(checkpoints, pause["oracle_contexts"])


def later_checkpoint(run, pause, require_refresh=False):
    """Validate a later certified state without replaying an earlier pause."""
    certify_pause(run, pause)
    cohort = feeds(run)
    resumed = {feed_key(feed): run.confirmed("06-resume", feed_key(feed)) for feed in cohort}
    refreshed = {oracle: run.confirmed("07-finalize", f"refresh:{oracle}")
                 for oracle in {feed["oracle"] for feed in cohort}}
    if not any(refreshed.values()) and (require_refresh or not any(resumed.values())):
        return None
    expected_oracles = manual_checkpoints(run, "06-resume", pause["oracle_contexts"])
    paused = {item["key"]: item for item in pause["feeds"]}
    for oracle, done in refreshed.items():
        if done is not None:
            group = [feed for feed in cohort if feed["oracle"] == oracle and not feed["initial_manual"]]
            refresh_execution(done["execution"]["receipts"], oracle, group)
            expected_oracles[oracle] = done["oracle_context"]
    check_oracle_contexts(run, expected_oracles)
    current = []
    for feed in cohort:
        key = feed_key(feed)
        done = resumed[key]
        live = frozen_definition(run, feed)
        expected_manual = feed["initial_manual"] if done is not None else True
        require(live["breaker"]["is_manually_tripped"] == expected_manual,
                f"manual state differs from latest certified checkpoint: {key}")
        if refreshed[feed["oracle"]] is None or feed["initial_manual"]:
            same_incident(live, (done or paused[key])["context"])
        if expected_manual:
            require(cached_price(run, feed) is None, f"paused cached price available: {key}")
            state = "intentionally_paused" if feed["initial_manual"] else "campaign_paused"
        elif refreshed[feed["oracle"]] is not None:
            refreshed_context(run, feed)
            require_no_automatic_trip(live["breaker"])
            fresh_price(run, cached_price(run, feed), price_age(run, feed), f"final cached price for {key}")
            state = "healthy_fresh_cached_price"
        else:
            state = "manual_trip_cleared_cache_not_verified"
        current.append({"key": key, "state": state, "context": live})
    run.recheck_reconciliation()
    return {"historical_pause": pause, "current_feeds": current, "oracle_contexts": expected_oracles}
