# Fuzzing

`cargo-fuzz` / libFuzzer targets for the contract crates. Targets live in
`fuzz_targets/` and are registered as `[[bin]]` entries in `Cargo.toml`.

**Before writing or changing a target, read [`PRINCIPLES.md`](./PRINCIPLES.md).**
It is the normative, research-grounded guidance on what makes a fuzz target
effective (fuzz the real code, keep the oracle independent and able to fail, treat
crashes as findings, don't narrow inputs to dodge bugs). Agents should also skim
[`AGENTS.md`](./AGENTS.md) for operational notes. The sections below are
operational reference; the principles are normative.

**When a target crashes, follow [`TRIAGE.md`](./TRIAGE.md)** — the step-by-step
runbook for reproducing, classifying, fixing, and turning the crash into a
durable regression seed.

## Running

```bash
# List enabled targets
cargo +nightly fuzz list

# Run one target (also feed the committed seeds when present)
cargo +nightly fuzz run <target> corpus/<target> seeds/<target> -- -max_total_time=120

# Run every enabled target for 2 minutes each (CI-style smoke)
./run_fuzzing.sh
```

### Corpus vs. seeds

- **`corpus/`** — the large, evolving libFuzzer corpus. **Gitignored.** Persisted
  across CI runs via the Actions cache; regenerable, so it is not in version
  control.
- **`seeds/`** — a small, curated, **committed** set of inputs (regression
  reproducers for tracked bugs + coverage bootstrap). Fed read-only alongside
  `corpus/` on every run. See [`seeds/README.md`](./seeds/README.md), including
  how to promote a CI-found crash into a durable regression seed.
- **`artifacts/`** — where libFuzzer writes a crashing input. **Gitignored**,
  transient. A CI crash is uploaded as a 90-day workflow artifact, **not**
  auto-committed — promote it to `seeds/` to make it durable.

Targets build with `overflow-checks` on, so a checked-arithmetic overflow that
is an *intentional* contract safety property will surface as a libFuzzer crash.
That is expected — see principles 2 and 4 in `PRINCIPLES.md` for how to handle it (assert
the boundary; don't make it unreachable).

## Healthy targets — use these as references

| Target | Why it's a good model |
| --- | --- |
| `fuzz_vault_state_machine` | Stateful: drives arbitrary sequences of **real** kernel transitions and re-checks invariants after each step. Bounds inputs in a *targeted, documented* way (`truncate_plan`). |
| `fuzz_soroban_storage_codecs` | Real encode→decode round-trips for every policy/state codec. (Raw-byte decoding isn't fuzzed — it over-allocates on an unbounded length prefix, [ENG-345](https://linear.app/templar-protocol/issue/ENG-345).) |
| `fuzz_liquidations` | Calls the real `BorrowPosition::liquidatable_collateral` and asserts its safety invariants (seize ≤ collateral, zero-liability ⇒ zero, underwater ⇒ all). Documents + skips one tracked denominator-underflow bug (P2/P4). |
| `fuzz_borrow_overflow`, `fuzz_supply_overflow` | Boundary backstops: drive the operand sums across the full `u128` range and assert exactness up to the overflow boundary. The abort direction (which libfuzzer-sys can't observe) is covered by the paired `#[should_panic]` unit tests. |
| `fuzz_snapshot` | Round-trip oracle: Borsh (storage codec) asserted **bit-lossless**; JSON (view codec) asserted to round-trip every *structural* field exactly while tolerating `Decimal`'s lossy display precision. Shows how to pick the right oracle strength per codec. |
| `fuzz_deposit_msg` | Fuzzes the real `DepositMsg` parse path (the attacker-controlled `ft_transfer_call` `msg`): raw bytes must never panic the parser, and constructed messages round-trip idempotently. |

Every target carries a `// MUTATION-CHECK:` note (P5) describing a one-line edit
to the real code that should make it fail — run it when you change a target.

## Disabled targets

None. All targets are enabled in `Cargo.toml`. (If you must disable one, it is
coverage debt — give it a tracked issue and a row here per P4/P6, don't leave
a bare `TODO`.)

## Removed targets

| Target | Reason | Tracking |
| --- | --- | --- |
| `fuzz_liquidator_logic` | Was a toy reimplementation testing only harness arithmetic (P1), not contract code. Its real counterpart is the off-chain `ProfitabilityCalculator` in `templar-liquidator`, which pulls a full async stack not worth dragging into a libFuzzer binary. On-chain liquidation amounts remain covered by `fuzz_liquidations`. | [ENG-344](https://linear.app/templar-protocol/issue/ENG-344) |

## Bugs found by fuzzing

Bugs are tracked in **Linear**, not here — this file only records what each
target currently *excludes* so coverage debt stays visible (see the table
below). Most open bugs have their crashing input preserved as a committed
regression seed under [`seeds/<target>/`](./seeds/README.md) (named after the
issue key), the triggering region documented inline in the target, and a paired
`#[should_panic]` unit test where a boundary is involved.

| Issue | Bug | Found by |
| --- | --- | --- |
| [ENG-341](https://linear.app/templar-protocol/issue/ENG-341) | `Piecewise::new` underflows on unsigned `Decimal` subtraction when `base > optimal·(rate_2−rate_1)` | `fuzz_decimals` |
| [ENG-342](https://linear.app/templar-protocol/issue/ENG-342) | `liquidatable_collateral` denominator underflows when `mcr·(1−liquidator_spread) ≤ 1` | `fuzz_liquidations` |
| [ENG-343](https://linear.app/templar-protocol/issue/ENG-343) | `Valuation::ratio` divides by zero via `pow2_int(384)` overflow on extreme exponent gaps | `fuzz_price` |
| [ENG-345](https://linear.app/templar-protocol/issue/ENG-345) | Soroban storage decoders over-allocate on an unbounded length prefix (raw-decode disabled, no seed) | `fuzz_soroban_storage_codecs` |

## Targeted input bounds (P2) — all backstopped

These narrowings dodge a *specific* intentional `u128`/`Decimal` overflow abort, or
a *tracked* open bug, so the fuzzer explores logic instead of rediscovering the
same crash. Each is backstopped so the excluded region is still covered:

| Target | Bound | Backstop |
| --- | --- | --- |
| `fuzz_borrow`, `fuzz_borrow_invariants` | amounts `u64` (liability sum can't overflow `u128`) | `fuzz_borrow_overflow` + `borrow::tests::liability_overflow_aborts_on_principal_plus_in_flight` |
| `fuzz_supply` | amounts `u64` (`Deposit::total` sum can't overflow) | `fuzz_supply_overflow` + `supply::tests::deposit_total_overflow_aborts` |
| `fuzz_liquidations` | prices within ~6 orders of magnitude; skips **only** the `1 < cr < mcr` band when `mcr·(1−spread) ≤ 1` (healthy/underwater positions still fuzzed) | `fuzz_decimal_arithmetic` covers Decimal overflow guards; underflow region is [ENG-342](https://linear.app/templar-protocol/issue/ENG-342), abort pinned by `borrow::tests::liquidatable_collateral_denominator_underflow_aborts` |
| `fuzz_price` | `decimals` in `[0, 30]` (extreme exponent gap excluded) | excluded region is [ENG-343](https://linear.app/templar-protocol/issue/ENG-343) |
| `fuzz_price_calculations` | `decimals` in `[0, 18]`, `expo` in `[-12, -4]` (fuzzed), prices in `[1e5, ~1e9]` — combined gap kept inside `ratio`'s exact path | extreme-gap region is [ENG-343](https://linear.app/templar-protocol/issue/ENG-343) (owned by `fuzz_price`) |
| `fuzz_decimals` | skips `base > optimal·(rate_2−rate_1)` for `Piecewise` (constructor underflow — whole region aborts, nothing to recover) | excluded region is [ENG-341](https://linear.app/templar-protocol/issue/ENG-341), abort pinned by `interest_rate_strategy::tests::piecewise_new_underflows_when_base_exceeds_cross_term` |
