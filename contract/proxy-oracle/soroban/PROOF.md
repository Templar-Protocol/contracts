# Defense Against Blend-Style Oracle Manipulation

This is a conditional defense model and regression guide, not a proof of source
independence, economic security, or production upgrade safety. See [audit
boundary](AUDIT.md) and [operator runbook](RUNBOOK.md).

## Threat and aggregation choices

The motivating attack manipulated one venue's reported price. Combining sources
can reduce that exposure only when their actual upstream data and failure modes
are sufficiently independent. Distinct contract addresses and quorum counts do
not establish that independence or manipulation resistance.

Soroban has one typed configuration boundary:

```rust
let proxy_config = ProxyConfig {
    aggregator: AggregatorConfig::MedianLow(MedianConfig {
        sources: soroban_sdk::vec![&env,
            SourceConfig { oracle: source_a, asset: asset.clone(), expected_decimals: 8 },
            SourceConfig { oracle: source_b, asset: asset.clone(), expected_decimals: 8 },
        ],
        min_sources: 2,
    }),
    max_age_secs: 300,
    max_clock_drift_secs: 0,
};
```

`MedianHigh(MedianConfig { sources, min_sources })` selects the upper median;
`Priority(sources)` selects the first surviving source in configured order and
has no quorum. Medians use equal unit weights, not configurable weights.
For fresh values `[100, 200]` with quorum 2, MedianLow returns 100 and MedianHigh
returns 200; Priority `[200, 100]` returns 200, falling back to 100 when the first
source is invalid. With three surviving independent sources, an equal-weight
median of `[1.06, 1.06, 106.74]` is 1.06. That example does not generalize into a
security guarantee for Priority, a reduced surviving set, or correlated sources.

Admission requires 1–16 distinct oracle addresses, valid median quorum, pins
≤ 18 matching source decimals, and required `u64` freshness bounds (age at most
seven days, drift at most one hour). Refresh discards changed decimals/base,
stale samples, and samples too far in the future. It never learns a new pin.
Zero age is allowed; feeds consumed by a Blend pool should use zero clock drift.
Latest reads require both cache-update and publication age within the inclusive
age bound, using saturating subtraction. No stale-cache fallback is promised.

## Breakers and their limits

- **StepwiseChange** compares a candidate with the last accepted sample and blocks
  a relative change strictly greater than its threshold.
- **MonotonicRun** counts same-direction transitions meeting its minimum step;
  it requires zero sampling. Small changes can remain below the threshold.
- **WindowedChangeDelta** compares means of equal-sized windows directly.
- **CumulativeChange** uses the immutable fresh accepted baseline captured at
  installation. To deliberately rebase, remove it, successfully refresh, then add
  it again. Rearm does not replace that baseline.
- **Manual trip** requires a governance proposal by `ManualTripper` or Admin and
  its applicable timelock; it is not guaranteed instantaneous intervention.

Relative rules cannot manufacture a trusted first bootstrap sample. Sample-count
windows/streaks are not wall-clock rate limits: keeper cadence matters and long
gaps can cause conservative availability trips. History capacity must cover
installed rules. Thresholds satisfy `0 < x <= 1`; a decline of exactly 100% is
not greater than threshold 1. `ArmedAfter` intentionally permits privileged
recovery during its cooldown; rearm retains histories rather than imposing a
fail-closed outage. Authorities able to disable enforcement remain trusted.

## Lifecycle and observable outcomes

Any changed policy clears served history/cache. Aggregation method, ordered
source identities/pins, or median-quorum changes reset breakers while preserving
the next-ID allocator; freshness-only changes retain breakers. Identical config
is a no-op. Removal/recreation retains an allocator tombstone. Corrupt or missing
decoder state must not silently reset an unknown allocator; archived entries
require normal restoration.

Historical records remain freshness-independent but are hidden while blocked.
Parent history is chronological; adapter `prices` returns newest-first distinct
resolution buckets, skipping unconvertible records. `price` buckets the requested
time and chooses that bucket's newest convertible record.

`RefreshFailure` retains its payload and uses aggregation codes 101 (length
mismatch) and 102 (too few valid sources), paired with
`AggregationFailureDetail { asset, code, expected, actual }`. Every blocked
refresh has `CacheBlockedDetail { asset, blocking_breaker_ids }`, empty for a
manual trip. Adds pair the existing event with `CircuitBreakerAddedDetail {
asset, breaker_id, config, baseline }`; baseline is present only for cumulative
rules. These are release contract requirements; integration and actual-host
verification must establish them, not this document.

Proposals have snapshotted maturity and no expiry. Creator/current Admin may
cancel; currently authorized peers may execute. Timelock increases do not delay
existing proposals. Persist original explicit IDs and exact payloads for retries;
contention rejection does not establish success. If recorded transaction evidence
or a matching live proposal cannot establish an uncertain outcome, stop rather
than select another ID or execute a guessed proposal.

## Regression entrypoints and deployment boundary

```bash
just test-fast -p templar-proxy-oracle-kernel blend_exploit
just -f contract/proxy-oracle/soroban/justfile test
just -f contract/proxy-oracle/soroban/justfile test-integration
```

Expected regressions include median endpoint selection, Priority fallback,
quorum/all-invalid failure, decimal drift rejection, bootstrap/cumulative
baseline behavior, halted historical reads, and exact companion diagnostics.
Commands above are instructions, not recorded verification results.

Unified runtime config XDR and governance's eight fixed groups of up to eight
unchanged proposal bodies are **fresh-stack-only**. Incompatible runtime/governance
in-place migration is skipped; existing deployments remain unchanged. No old
configuration decoder, defaults, or side maps are provided. Prove matching
runtime/governance ownership wiring, Admin authorization, adapter bindings and
base, and production timelock hardening after zero-TTL bootstrap on a new stack.
An independent adapter upgrade requires an exact old/new compiled-Wasm swap
proof; `integration-tests/tests/upgrade.rs` does not establish one. No live
identity, transaction authorization, or production upgrade approval is implied.
