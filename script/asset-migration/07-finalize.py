#!/usr/bin/env python3
"""Refresh through Sputnik and prove actual fresh cached prices, not source estimates."""
from common import Stop, require, stage_main
from governance import (cached_price, check_oracle_contexts, context, feed_key, feeds, fresh_price,
                        frozen_definition, price_age, refresh_execution, refreshed_context, require_no_automatic_trip,
                        same_incident, source_ready)


def observations(run, cohort):
    result = []
    for feed in cohort:
        item = {"key": feed_key(feed), "initial_manual": feed["initial_manual"]}
        try:
            item["context"] = context(run, feed)
            item["cached_price"] = cached_price(run, feed)
        except Stop as error:
            item["observation_error"] = str(error)
        result.append(item)
    return result


def verify_final(run, cohort, refreshed, oracle_contexts):
    reconciliation = run.recheck_reconciliation()
    run.assert_no_unresolved()
    evidence = []
    for feed in cohort:
        live = frozen_definition(run, feed)
        require(live["breaker"]["is_manually_tripped"] == feed["initial_manual"],
                f"final manual state differs: {feed_key(feed)}")
        if feed["initial_manual"]:
            original = run.confirmed("02-pause", feed_key(feed))
            require(original is not None and original["ownership"] == "originally_paused", "missing original pause evidence")
            same_incident(live, original["context"])
            require(cached_price(run, feed) is None, "originally paused feed exposes cached price")
            evidence.append({"key": feed_key(feed), "state": "intentionally_paused", "context": live,
                             "cached_price": None})
        else:
            refreshed_context(run, feed)
            require_no_automatic_trip(live["breaker"])
            readiness = source_ready(run, feed)
            cached = fresh_price(run, cached_price(run, feed), price_age(run, feed),
                                 f"actual on-chain cached price for {feed_key(feed)}")
            evidence.append({"key": feed_key(feed), "state": "healthy_fresh_cached_price", "context": live,
                             "cache": cached, "source_readiness": readiness})
    identities = {}
    for market in run.intent["markets"]:
        identity = run.identity(market["account"])
        require(identity == market["account_identity"], f"final restored code/linkage differs: {market['account']}")
        require(run.read("market.getConfiguration", {"market_id": market["account"]}) == market["expected_configuration"],
                f"final market configuration differs: {market['account']}")
        identities[market["account"]] = identity
    check_oracle_contexts(run, oracle_contexts)
    return {"intent_digest": run.intent["intent_digest"], "feeds": evidence,
            "refresh_operations": refreshed, "reconciliation": reconciliation, "market_identities": identities,
            "oracle_contexts": oracle_contexts}


def handle(run):
    run.require_stage("06-resume")
    if run.submit:
        run.require_write_admission()
    cohort = feeds(run)
    prior = run.confirmed("07-finalize", "stage")
    if prior is not None:
        # Completion is not a perpetual health certificate. A resumed invocation
        # re-reads cache freshness, accounting and full incident context.
        try:
            for oracle, expected in prior["oracle_contexts"].items():
                same_incident(run.oracle_context(oracle), expected)
            verify_final(run, cohort, prior["refresh_operations"], prior["oracle_contexts"])
        except Stop as error:
            run.record("07-finalize", "stage", "failed", {"code": error.code, "error": error.message,
                       "actual_feed_state": observations(run, cohort), "automatic_repause": False,
                       "previous_completion_no_longer_current": True})
            raise
        return prior
    resume = run.confirmed("06-resume", "stage")
    expected_contexts = dict(resume["oracle_contexts"])
    groups = {}
    for feed in cohort:
        if not feed["initial_manual"]:
            groups.setdefault(feed["oracle"], []).append(feed)
    for oracle in groups:
        completed = run.confirmed("07-finalize", f"refresh:{oracle}")
        if completed is not None:
            refresh_execution(completed["execution"]["receipts"], oracle, groups[oracle])
            expected_contexts[oracle] = completed["oracle_context"]
    refreshed = []
    try:
        for oracle, group in sorted(groups.items()):
            key = f"refresh:{oracle}"
            done = run.confirmed("07-finalize", key)
            if done is None:
                directory = run.operation_dir("07-finalize", key)
                plan = run.plan("proxyOracle.updatePrices", {"oracle_id": oracle,
                                "price_ids": sorted(feed["feed"] for feed in group)},
                                run.campaign["dao"], directory / "refresh.plan.json")

                def before_write():
                    run.require_write_admission()
                    run.recheck_reconciliation()
                    check_oracle_contexts(run, expected_contexts)
                    for feed in group:
                        live = frozen_definition(run, feed)
                        require(not live["breaker"]["is_manually_tripped"], "feed manually paused after resume")
                        require_no_automatic_trip(live["breaker"])
                        source_ready(run, feed)

                def verify_execution(result):
                    refresh_execution(result, oracle, group)
                    for feed in cohort:
                        if feed["oracle"] == oracle:
                            live = refreshed_context(run, feed)
                            if feed["initial_manual"]:
                                same_incident(live, run.confirmed("02-pause", feed_key(feed))["context"])

                execution = run.ordinary("07-finalize", key, plan,
                                         f"migration {run.intent['intent_digest']} refresh {oracle}",
                                         before_write=before_write, verify_execution=verify_execution)
                prices = []
                for feed in group:
                    live = refreshed_context(run, feed)
                    require(not live["breaker"]["is_manually_tripped"], "feed manually tripped during refresh")
                    require_no_automatic_trip(live["breaker"])
                    prices.append({"key": feed_key(feed), **fresh_price(run, cached_price(run, feed), price_age(run, feed),
                                   f"actual refreshed cache for {feed_key(feed)}")})
                done = {"oracle": oracle, "execution": execution, "prices": prices,
                        "oracle_context": run.oracle_context(oracle)}
                run.confirm("07-finalize", key, done)
            refresh_execution(done["execution"]["receipts"], oracle, group)
            same_incident(run.oracle_context(oracle), done["oracle_context"])
            expected_contexts[oracle] = done["oracle_context"]
            refreshed.append(done)
        evidence = verify_final(run, cohort, refreshed, expected_contexts)
    except Stop as error:
        # A failed refresh does not roll back a prior unpause. Record the real
        # open/manual/cache state; emergency re-pause is a new authorized action.
        event = "waiting" if error.code == 10 else "unknown" if error.code == 30 else "failed"
        run.record("07-finalize", "stage", event, {"code": error.code, "error": error.message,
                   "actual_feed_state": observations(run, cohort), "automatic_repause": False})
        raise
    report = run.path("07-finalize/report.json")
    if report.exists():
        frozen = run.load("07-finalize/report.json")
        require(frozen["oracle_contexts"] == evidence["oracle_contexts"]
                and frozen["market_identities"] == evidence["market_identities"]
                and frozen["refresh_operations"] == evidence["refresh_operations"],
                "final report differs from freshly verified immutable execution")
        evidence = frozen
    else:
        run.save("07-finalize/report.json", evidence)
    run.confirm("07-finalize", "stage", evidence)
    return evidence


if __name__ == "__main__":
    stage_main("07-finalize", handle)
