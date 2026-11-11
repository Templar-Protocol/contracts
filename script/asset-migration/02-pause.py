#!/usr/bin/env python3
"""Pause each cohort feed through two separately approved Sputnik actions."""
from common import require, stage_main
from governance import (cached_price, check_oracle_contexts, feed_key, feeds, frozen_definition,
                        later_checkpoint, manual_checkpoints, manual_trip, same_incident)


def handle(run):
    run.require_stage("01-register")
    if run.submit:
        run.require_write_admission()
    prior = run.confirmed("02-pause", "stage")
    if prior is not None:
        later = later_checkpoint(run, prior)
        if later is not None:
            return later
        for oracle, expected in prior["oracle_contexts"].items():
            same_incident(run.oracle_context(oracle), expected)
        for feed in feeds(run):
            require(frozen_definition(run, feed)["breaker"]["is_manually_tripped"],
                    f"cohort feed no longer paused: {feed_key(feed)}")
            require(cached_price(run, feed) is None, f"paused feed exposes a cached price: {feed_key(feed)}")
        return prior
    cohort = feeds(run)
    initial_path = "02-pause/initial-oracle-contexts.json"
    if run.path(initial_path).exists():
        initial = run.load(initial_path)
    else:
        require(not any(event["stage"] == "02-pause" and event["operation"] is not None
                        and event["event"] == "attempt_started" for event in run.events),
                "pause already started without its initial raw checkpoint; operator inspection required")
        for feed in cohort:
            same_incident(frozen_definition(run, feed)["breaker"], feed["initial_breaker"])
        initial = {oracle: run.oracle_context(oracle) for oracle in sorted({feed["oracle"] for feed in cohort})}
        run.save(initial_path, initial)
        run.record("02-pause", "oracle-contexts", "prepared", {"oracle_contexts": initial})
    expected_oracles = manual_checkpoints(run, "02-pause", initial)
    results = []

    def check_cohort():
        run.require_write_admission()
        check_oracle_contexts(run, expected_oracles)
        for completed in results:
            completed_feed = next(feed for feed in cohort if feed_key(feed) == completed["key"])
            same_incident(frozen_definition(run, completed_feed), completed["context"])
            require(cached_price(run, completed_feed) is None, "completed pause exposes a cached price")

    for feed in cohort:
        key = feed_key(feed)
        done = run.confirmed("02-pause", key)
        if done is None:
            if feed["initial_manual"]:
                live = frozen_definition(run, feed)
                same_incident(live["breaker"], feed["initial_breaker"])
                require(live["breaker"]["is_manually_tripped"], "original manual pause disappeared")
                evidence = {"key": key, "ownership": "originally_paused", "context": live}
            else:
                def before_write():
                    check_cohort()
                    live = frozen_definition(run, feed)
                    same_incident(live["breaker"], feed["initial_breaker"])
                    require(not live["breaker"]["is_manually_tripped"], "foreign manual pause; inspect before continuing")
                execution = manual_trip(run, "02-pause", feed, True, before_write,
                                        expected_oracles[feed["oracle"]])
                live = frozen_definition(run, feed)
                expected_breaker = dict(feed["initial_breaker"], is_manually_tripped=True)
                same_incident(live["breaker"], expected_breaker)
                oracle_context = execution["oracle_context"]
                same_incident(run.oracle_context(feed["oracle"]), oracle_context)
                expected_oracles[feed["oracle"]] = oracle_context
                evidence = {"key": key, "ownership": "campaign", "intent_digest": run.intent["intent_digest"],
                            "context": live, "governance": execution}
            require(cached_price(run, feed) is None, f"manual pause did not block cached reads: {key}")
            run.confirm("02-pause", key, evidence)
            done = evidence
        live = frozen_definition(run, feed)
        same_incident(live, done["context"])
        require(live["breaker"]["is_manually_tripped"], f"manual pause missing: {key}")
        require(cached_price(run, feed) is None, f"paused cached price available: {key}")
        results.append(done)
    # Each checkpoint must equal its frozen predecessor plus exactly the own
    # native manual-trip transition, even when the final DAO vote arrived later.
    check_oracle_contexts(run, expected_oracles)
    evidence = {"feeds": results, "oracle_contexts": expected_oracles}
    run.save("02-pause/report.json", evidence)
    run.confirm("02-pause", "stage", evidence)
    return evidence


if __name__ == "__main__":
    stage_main("02-pause", handle)
