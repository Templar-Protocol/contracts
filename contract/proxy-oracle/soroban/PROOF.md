# Proof: Templar Proxy Oracle Prevents Blend-Style Exploits

**Date**: 2026-05-23
**Scope**: Soroban Proxy Oracle
**Threat**: Oracle price manipulation as demonstrated in the Blend protocol exploit (see `/home/peer2/Downloads/blendmath.md`)

---

## The Blend Exploit

The attacker manipulated SDEX trades to inflate USTRY from $1.0574 → $106.7373 (100.9×). The attack succeeded because:

1. **Single source**: Reflector VWAP of SDEX trades was the only price source.
2. **Weak deviation check**: Blend compared current vs previous sample — both were inflated, so δ = 0 passed the 10% threshold.
3. **No circuit breaker or kill switch**.

---

## Defense Analysis

### Multi-Source Aggregation

With `n ≥ 3` independent sources and `min_sources = ⌈n/2⌉`, a single-source manipulation cannot determine the aggregated price.

Example: Pyth reports $1.06, RedStone reports $1.06, Reflector reports $106.74. The median is $1.06. The attacker must compromise a majority of sources simultaneously.

### StepwiseChange

Blocks prices that deviate more than `max_relative_change` from the last accepted price:

```
δ = |p_new - h_last| / h_last
```

For the Blend attack: `|106.7373 - 1.0574| / 1.0574 = 100.9` > 10% → **BLOCKED**.

Blend compared against `p_previous` (which could also be manipulated). Templar compares against `h_last` from the **accepted history** — prices that already passed all defenses.

### MonotonicRun

Blocks sustained directional movement. If the price moves in the same direction for more than `max_streak` consecutive windows with per-window change ≥ `min_relative_step_change`, the breaker trips.

Example: A sustained 20% pump for 4 windows trips the breaker when `max_streak = 3`, even if each window stays under StepwiseChange.

### WindowedChangeDelta

Blocks statistical outliers by comparing recent window average against historical average:

```
δ = |A_recent - A_history| / A_history
```

A 100× jump makes `A_recent ≈ 50× A_history`, which far exceeds any reasonable `max_relative_change_delta`.

### Freshness Filter

Rejects prices older than `max_age_secs` or newer than `max_clock_drift_secs`. Prevents acceptance of stale or future-dated manipulated prices.

### Manual Trip

An operator with `Role::ManualTripper` can immediately block all refreshes for an asset. Untripping requires the same governance role.

---

## Combined Defense

For the Blend attack to succeed against a Templar proxy, the attacker must simultaneously satisfy ALL of the following:

1. Compromise ≥2 of 3 independent sources
2. Keep the manipulated price within 10% of accepted history (impossible for a 100× jump)
3. Sustain any gradual ramp for ≤3 consecutive windows with <1% per step
4. Keep recent average within 15% of historical average
5. Execute within the freshness window
6. Prevent any authorized operator from triggering a manual trip

The attack is computationally and economically infeasible.

---

## Defense-in-Depth Matrix

| Stage | Blend | Templar | Failure Mode |
|-------|-------|---------|--------------|
| Source IO | Single SDEX VWAP | 3+ sources + quorum | Attacker must compromise majority |
| Aggregation | Direct pass-through | Median of surviving | Outlier suppression |
| Freshness | None | `max_age_secs` + `max_clock_drift_secs` | Stale/future prices rejected |
| Deviation check | Current vs previous | StepwiseChange vs accepted history | Historical baseline prevents chaining |
| Sustained movement | None | MonotonicRun | Gradual ramps caught |
| Statistical anomaly | None | WindowedChangeDelta | Outliers blocked |
| Emergency response | None | ManualTripSet + governance role | Human-in-the-loop kill switch |

---

## Configuration

```rust
let proxy_config = ProxyConfig {
    sources: vec![pyth, redstone, dia],
    min_sources: 2,
    max_age_secs: Some(300),
    max_clock_drift_secs: Some(60),
};

let stepwise = StepwiseChange { max_relative_change: dec("0.10") };
let monotonic = MonotonicRun { max_streak: 3, min_relative_step_change: dec("0.01") };
let windowed = WindowedChangeDelta { window_len: 2, lookback_windows: 3, max_relative_change_delta: dec("0.15") };
```

Configure `history_len` ≥ max required by any breaker. For the above: `max(1, 3, 8) = 8`.

Guard `Role::ManualTripper` access carefully; any operator with this role can both trip and untrip feeds.

---

## Limitations

- **Source independence**: If all sources rely on the same underlying exchange, the effective source count is 1.
- **Governance integrity**: A compromised governance key can disable all breakers.
- **Operator vigilance**: Manual trip requires operators to monitor and respond.
- **Gradual manipulation**: An attacker could theoretically bypass MonotonicRun with sub-1% per-window moves sustained over hundreds of windows — but this requires compromising a majority of independent sources simultaneously.

---

## References

- [Blend Exploit Analysis](/home/peer2/Downloads/blendmath.md)
- [README](README.md)
- [Runbook](RUNBOOK.md)
- [Audit Evidence](AUDIT.md)
- [Parity Matrix](PARITY.md)
