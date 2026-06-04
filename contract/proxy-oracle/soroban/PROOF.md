# Defense Against Blend-Style Oracle Manipulation

How the Templar proxy oracle's layered defenses stop the oracle-manipulation
class of attack demonstrated by the Blend exploit. See the internal Blend
exploit analysis for the source incident.

## The exploit

An attacker manipulated SDEX trades to inflate USTRY from $1.0574 to $106.7373
(100.9×). It worked because Blend used a **single source** (Reflector VWAP of
SDEX trades), a **weak deviation check** (current vs previous sample — both
inflated, so δ ≈ 0 passed the 10% threshold), and had **no circuit breaker or
kill switch**.

## Defenses

Each Templar defense closes one of those gaps; an attack must defeat all of them
at once.

- **Multi-source quorum** — with `n ≥ 3` independent sources and `min_sources =
  ⌈n/2⌉`, no single compromised source determines the price. Median of
  `[1.06, 1.06, 106.74]` is `1.06`; the attacker must compromise a majority.
- **StepwiseChange** — blocks a single step exceeding `max_relative_change`
  versus the last **accepted** price (`δ = |p_new − h_last| / h_last`). Blend
  compared against the previous sample, which could itself be manipulated;
  Templar compares against accepted history. For Blend: `δ = 100.9 > 0.10` →
  blocked.
- **MonotonicRun** — blocks sustained directional movement: same direction for
  more than `max_streak` windows with per-window change ≥ `min_relative_step_change`.
  Catches staged ramps that each stay under StepwiseChange.
- **WindowedChangeDelta** — blocks statistical outliers by comparing the recent
  window average against the historical average (`δ = |A_recent − A_history| /
  A_history`). A 100× jump makes `A_recent ≈ 50× A_history`.
- **Freshness filter** — rejects prices older than `max_age_secs` or more than
  `max_clock_drift_secs` in the future, so stale or pre-manufactured prices
  cannot be replayed.
- **Manual trip** — a `ManualTripper` operator can immediately block a feed via
  governance; untripping needs the same role.

## Defense-in-depth

| Stage | Blend | Templar |
|-------|-------|---------|
| Source IO | single SDEX VWAP | 3+ sources + quorum |
| Aggregation | pass-through | median of survivors |
| Freshness | none | `max_age_secs` + `max_clock_drift_secs` |
| Deviation | current vs previous | StepwiseChange vs accepted history |
| Sustained move | none | MonotonicRun |
| Statistical anomaly | none | WindowedChangeDelta |
| Emergency stop | none | governed manual trip |

To land the Blend attack against this stack an attacker must *simultaneously*
compromise a majority of independent sources, stay within 10% of accepted
history (impossible for a 100× jump), keep any ramp under `max_streak` windows
at < 1% per step, keep the recent average within the windowed bound, land inside
the freshness window, and avoid any operator manual trip.

## Example configuration

```rust
let proxy_config = ProxyConfig {
    sources: vec![pyth, redstone, reflector],
    min_sources: 2,
    max_age_secs: Some(300),
    max_clock_drift_secs: Some(60),
};
let stepwise  = StepwiseChange { max_relative_change: dec("0.10") };
let monotonic = MonotonicRun { max_streak: 3, min_relative_step_change: dec("0.01") };
let windowed  = WindowedChangeDelta { window_len: 2, lookback_windows: 3, max_relative_change_delta: dec("0.15") };
```

Set `history_len` ≥ the largest lookback any installed breaker needs. Guard the
`ManualTripper` role: any holder can both trip and untrip.

## Demonstration

```bash
cargo test -p templar-proxy-oracle-soroban-contract --features testutils blend_exploit
```

| Scenario | Expected outcome |
|----------|------------------|
| 3 sources, 1 manipulated | median returns the honest price |
| 100× single-step jump | StepwiseChange trips, cache blocked |
| 20% pump × 4 windows | MonotonicRun trips on window 4 |
| 50× statistical outlier | WindowedChangeDelta blocks |
| 10-minute-old price | freshness filter rejects |
| operator manual trip | all future refreshes blocked, event emitted |

## Limitations

- **Source independence** — if all sources derive from the same exchange, the
  effective source count is 1.
- **Governance integrity** — a compromised governance key can disable breakers.
- **Operator vigilance** — manual trip requires monitoring and a timely response.
- **Gradual manipulation** — sub-1%-per-window moves sustained over hundreds of
  windows can stay under MonotonicRun, but still require compromising a majority
  of independent sources.
