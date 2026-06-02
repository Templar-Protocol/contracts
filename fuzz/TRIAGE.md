# What to do when a fuzzer crashes

A crash is a **discovery, not a failure** — the engine's contract is "if it
aborts, it's a bug." This is the step-by-step runbook for handling one. It
operationalizes [`PRINCIPLES.md`](./PRINCIPLES.md) §4 (the three outcomes) and
feeds into [`seeds/README.md`](./seeds/README.md) (durable regression seeds).

> **Never** make the crash quiet by deleting the target, commenting out its
> `[[bin]]`, narrowing its inputs to dodge the input, or weakening the assertion
> that fired. Each of those discards the finding. If you genuinely must defer,
> that is coverage debt: file a tracked issue and add a row to `README.md`'s
> status tables — see step 5b.

---

## 0. Get the crashing input

**Local run.** libFuzzer wrote the reproducer to
`fuzz/artifacts/<target>/crash-*` and printed its path. Copy it somewhere stable
so it survives rebuilds while you work:

```bash
cp fuzz/artifacts/<target>/crash-XXXX /tmp/crash      # keep a stable copy
```

**CI run (nightly `Fuzz` workflow).** CI does *not* commit anything. It:
- opens a GitHub issue labelled `fuzz-crash` (deduped per target), with the
  input embedded as base64 when ≤ 4 KiB — copy that block and `base64 -d` it
  into a file; and
- uploads the input as the `fuzz-crash-<target>` workflow artifact (90-day
  retention) for larger inputs — download and unzip it.

Either way you end up with a single crash file. The rest of this runbook is the
same for local and CI crashes.

## 1. Reproduce it deterministically

```bash
cargo +nightly fuzz run <target> /tmp/crash
```

This runs the target on that one input. You should see the **same panic every
time** — note the panic message and the source location it points to. If it does
*not* reproduce, the target is non-deterministic (wall-clock, RNG, global
state); that is itself a bug to fix (P6) before you can triage anything.

## 2. Minimize it (optional but recommended)

```bash
cargo +nightly fuzz tmin <target> /tmp/crash
```

`tmin` shrinks the input to the smallest one that still crashes, which usually
makes the cause obvious. Use the minimized file for the rest of the steps.

## 3. Read the panic and classify the crash

Look at the panic message + location and decide which of the **three outcomes**
(PRINCIPLES §4) it is. This is the only judgement call in the runbook:

| Outcome | What it looks like | Example from this repo |
| --- | --- | --- |
| **1. Real bug** | An assertion in the target fired, or the **production code** panicked somewhere it shouldn't. The panic points into `common/` / `primitives/` and the input is one production would accept. | Removing `.min(collateral)` from `liquidatable_collateral` → invariant #1 fires: *"returned N > total_collateral M"*. ENG-341/342/343 were all this. |
| **2. Intentional abort** | The production code hit a **deliberate** safety check — a checked-arithmetic overflow / underflow that is *supposed* to abort on out-of-range input. The panic is `attempt to add with overflow` / `arithmetic operation overflow` at a known boundary. | `get_total_borrow_asset_liability` aborting when `principal + in_flight > u128::MAX`. |
| **3. Harness bug** | The *target* is wrong: it built a type bypassing its real constructor's invariants, called a removed/renamed API, or asserted something the function never promised. The panic points into `fuzz_targets/` and the "bug" disappears under correct expectations. | The `cr == mcr == 1` invariant overlap: invariant #4 asserted "result == 0" but the function's underwater branch (checked first) correctly returns full collateral. |

**How to tell #1 from #3:** ask *"is the production function actually wrong, or
is my oracle wrong?"* Re-derive the expected result by hand for the minimized
input. If the function's output is defensible and the assertion is what's
overreaching, it's a harness bug (#3). If the output violates a property the
contract genuinely must hold, it's real (#1).

**How to tell #1 from #2:** ask *"is this abort a designed safety property?"* An
intentional abort fires only at a documented boundary (e.g. a `u128` sum
overflowing) and is *correct* behavior. If the input that triggers it is one
production would reject upstream, it's #2; if production would happily process
that input, the abort is a real bug (#1).

## 4. Act on the classification

### Outcome 1 — Real bug
1. **File it** (Linear). Capture the minimized input, the panic, and the
   production location in the issue.
2. **Preserve the input as a regression seed** (step 5a) — named after the
   issue.
3. **Fix the production code**, or, if the fix is deferred, add a *targeted,
   documented, backstopped* guard in the target that skips **only** the exact
   triggering region (PRINCIPLES §2) plus a `#[should_panic]` unit test that
   pins the abort, and record it in `README.md`. (This is how ENG-341/342 are
   currently handled — see the "Targeted input bounds" table.)

### Outcome 2 — Intentional abort
The fuzzer is rediscovering a deliberate safety check. **Don't** make the
boundary unreachable. Instead:
1. Add/extend a **boundary backstop**: a target that drives the operand *to* the
   limit and asserts exactness up to it, plus a `#[should_panic(expected = …)]`
   unit test asserting the abort fires past it. (Pattern:
   `fuzz_borrow_overflow` + `borrow::tests::liability_overflow_aborts_*`.)
2. In the *exploring* target, predict-and-skip the overflowing input so the
   fuzzer spends its budget on logic, not on re-finding the same abort — with a
   comment naming the check and a `README.md` row.

### Outcome 3 — Harness bug
Fix the **target**, not the production code:
- Constructing a type directly? Use its real constructor so invariants hold.
- Asserting too much? Tighten the oracle to what the function actually promises
  (and prefer the always-safe claim — "doesn't panic / well-formed `Result`" —
  when you can't enumerate every rule, per PRINCIPLES §1).
- Calling a stale API? Update it.

A harness bug that was *masked* by an over-broad input filter is a signal the
filter was hiding real behavior — narrow the filter, don't widen it.

## 5. Make it durable

### 5a. Promote the input to a committed regression seed
For outcomes 1 and 2, the crashing input becomes a permanent regression test:

```bash
cp /tmp/crash fuzz/seeds/<target>/ENG-NNN-<short-description>
git add fuzz/seeds/<target>/
```

It rides along on every run (`run_fuzzing.sh` / CI feed `seeds/<target>` read-only).
While the guard/fix is in place it won't crash — but it *will* start crashing
again if the guard is removed or a fix regresses. See
[`seeds/README.md`](./seeds/README.md).

### 5b. Record any coverage reduction
If you guarded a region, skipped an input class, or (last resort) disabled a
target, add a row to the relevant table in `README.md` in the **same change**. A
buried reduction reads as "this is fine" to the next person; an explicit one
reads as debt to pay down (PRINCIPLES §4/§6).

## 6. Verify the fix against the same input

This is the step that closes the loop — prove the *exact* crashing input no
longer crashes:

```bash
cargo +nightly fuzz run <target> /tmp/crash      # exit 0, "Executed … in 0 ms"
```

For an intentional-abort boundary, also run the paired unit test:

```bash
cargo test -p templar-common --lib <abort_test_name>   # should_panic test passes
```

## 7. Re-run the target to confirm no *new* crash nearby

A fix can move the crash one step deeper. Run the target again (with the seed
corpus) for a while to confirm the region is clean:

```bash
cargo +nightly fuzz run <target> corpus/<target> seeds/<target> -- -max_total_time=120
```

If it survives, commit the fix, the new seed, and any `README.md` row together.

---

### Quick reference

```
crash → reproduce (step 1) → minimize (2) → classify (3)
  ├─ real bug (1)        → file + seed + fix/guard + should_panic test
  ├─ intentional abort(2)→ boundary backstop + predict-and-skip + seed
  └─ harness bug (3)     → fix the target's constructor/oracle/API
then → seed it (5a) → record coverage change (5b)
     → verify same input passes (6) → re-fuzz to confirm clean (7)
```
