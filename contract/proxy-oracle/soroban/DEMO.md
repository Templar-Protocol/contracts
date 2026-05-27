# Blend Exploit Prevention Demo

## The Blend Exploit (2026-02-22)

The Blend protocol was exploited via oracle price manipulation:

1. **Single-source attack**: SDEX VWAP via Reflector was manipulated, inflating USTRY from ~$1.06 to ~$106.74 (**100.9x jump**).
2. **Deviation check bypass**: Blend compared current vs previous price — both were inflated, so δ = 0 passed the 10% threshold.
3. **No circuit breaker or kill switch**: The 100x jump was accepted.

Root causes: single source of truth, naive deviation check comparing adjacent samples, no circuit breakers, no manual override.

---

## How Templar Prevents This

### 1. Multi-Source Aggregation + Quorum

Blend used one source. Templar requires ≥3 with `min_sources` quorum:

```rust
let proxy_config = ProxyConfig {
    sources: vec![pyth, redstone, reflector],
    min_sources: 2,  // Need 2 of 3
};
```

Result: Manipulating 1 source is insufficient. The median of [1.06, 1.06, 106.74] = 1.06.

### 2. StepwiseChange Circuit Breaker

Blend compared `p_current` vs `p_previous`. Templar compares against **accepted history**:

```rust
StepwiseChange { max_relative_change: dec("0.10") }
```

- Accepted history: $1.0574
- Manipulated price: $106.7373
- Relative change: **100.9x** > 10% → **BLOCKED**

### 3. MonotonicRun Circuit Breaker

Detects sustained directional ramps:

```rust
MonotonicRun { max_streak: 3, min_relative_step_change: dec("0.01") }
```

A 20% pump sustained for 4 windows trips the breaker even if each window stays under StepwiseChange.

### 4. WindowedChangeDelta Circuit Breaker

Catches statistical outliers by comparing recent vs historical averages:

```rust
WindowedChangeDelta { window_len: 2, lookback_windows: 3, max_relative_change_delta: dec("0.15") }
```

A 100x jump makes the recent average ~50x the historical — far above 15%.

### 5. Freshness Filter

Rejects stale or future-dated prices:

```rust
max_age_secs: Some(300),         // 5 minutes
max_clock_drift_secs: Some(60),  // 1 minute
```

Prevents acceptance of pre-manufactured stale prices.

### 6. Manual Trip (Kill Switch)

```rust
set_manual_trip(operator, asset, true, metadata);
```

Emergency operators with `Role::ManualTripper` can immediately block all refreshes. The same role is required to untrip.

---

## Demo Scenarios

```bash
just -f contract/proxy-oracle/soroban/justfile build
cargo test -p templar-proxy-oracle-soroban-contract blend_exploit -- --nocapture
```

| Scenario | What Happens |
|----------|-------------|
| Multi-source with 1 manipulated | Median aggregation returns honest price |
| 100x price jump | StepwiseChange trips, cache blocked |
| Sustained 20% pump × 4 windows | MonotonicRun trips on window 4 |
| Statistical 50x outlier | WindowedChangeDelta blocks |
| 10-minute-old price | Freshness filter rejects |
| Operator manual trip | All future refreshes blocked, event emitted |

---

## Defense-in-Depth Summary

| Blend Weakness | Templar Defense |
|---------------|----------------|
| Single-source oracle | Multi-source + quorum + median |
| Naive adjacent-sample deviation | StepwiseChange vs accepted history |
| Sustained manipulation across windows | MonotonicRun detects streaks |
| Massive single-window jumps | WindowedChangeDelta vs historical avg |
| No emergency stop | ManualTripSet with governance role |
| No freshness enforcement | max_age_secs + max_clock_drift_secs |
