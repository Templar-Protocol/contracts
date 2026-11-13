#!/usr/bin/env python3
"""Clear only campaign-owned pauses after fresh reconciliation and source checks."""
from common import require, stage_main
from governance import (check_oracle_contexts, context, feed_key, feeds, frozen_definition, later_checkpoint,
                        manual_checkpoints, manual_trip, require_no_automatic_trip, same_incident, source_ready)


def handle(run):
    run.require_stage("05-reconcile")
    run.recheck_metadata()
    if run.submit:
        run.require_write_admission()
    pause = run.confirmed("02-pause", "stage")
    require(pause is not None, "missing confirmed cohort pause evidence")
    later = later_checkpoint(run, pause, require_refresh=True)
    if later is not None:
        run.require_stage("06-resume")
        return later
    paused = {item["key"]: item for item in pause["feeds"]}
    expected_oracles = manual_checkpoints(run, "06-resume", pause["oracle_contexts"])
    cohort = feeds(run)
    completed = {feed_key(feed): run.confirmed("06-resume", feed_key(feed)) for feed in cohort}

    def before_write():
        run.require_write_admission()
        run.recheck_reconciliation()
        for current_feed in cohort:
            key = feed_key(current_feed)
            evidence = completed[key] or paused[key]
            live = frozen_definition(run, current_feed)
            same_incident(live, evidence["context"])
            if not current_feed["initial_manual"]:
                require_no_automatic_trip(live["breaker"])
                source_ready(run, current_feed)
        check_oracle_contexts(run, expected_oracles)

    results = []
    for feed in cohort:
        key = feed_key(feed)
        original = paused[key]
        done = completed[key]
        if done is None:
            if feed["initial_manual"]:
                require(original["ownership"] == "originally_paused", "original-pause evidence differs")
                live = context(run, feed)
                same_incident(live, original["context"])
                done = {"key": key, "state": "intentionally_paused", "context": live}
            else:
                require(original["ownership"] == "campaign" and original["intent_digest"] == run.intent["intent_digest"],
                        "manual pause has no campaign ownership proof")
                # This callback is invoked by ordinary immediately before each actual
                # creation/execution submission, not before read-only reconciliation.
                execution = manual_trip(run, "06-resume", feed, False, before_write,
                                        expected_oracles[feed["oracle"]])
                live = frozen_definition(run, feed)
                expected = dict(original["context"])
                expected["breaker"] = dict(expected["breaker"], is_manually_tripped=False)
                same_incident(live, expected)
                require_no_automatic_trip(live["breaker"])
                readiness = source_ready(run, feed)
                oracle_context = execution["oracle_context"]
                same_incident(run.oracle_context(feed["oracle"]), oracle_context)
                done = {"key": key, "state": "manual_trip_cleared_cache_not_verified", "context": live,
                        "source_readiness": readiness, "governance": execution, "oracle_context": oracle_context}
                expected_oracles[feed["oracle"]] = oracle_context
            run.confirm("06-resume", key, done)
            completed[key] = done
        same_incident(context(run, feed), done["context"])
        results.append(done)
    run.recheck_reconciliation()
    check_oracle_contexts(run, expected_oracles)
    evidence = {"feeds": results, "oracle_contexts": expected_oracles,
                "service_restoration": "requires_07_finalize_cached_price_verification"}
    prior = run.confirmed("06-resume", "stage")
    if prior is None:
        run.save("06-resume/report.json", evidence)
        run.confirm("06-resume", "stage", evidence)
    return prior or evidence


if __name__ == "__main__":
    stage_main("06-resume", handle)
