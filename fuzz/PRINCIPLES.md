# Principles of effective fuzzing

This is the canonical guidance for writing fuzz targets in this repository, for
humans and AI agents alike. It is **normative**: new and edited targets are
expected to follow it, and reviews cite it.

The aim is to keep our fuzzers *capable of failing when the code is wrong*. That
sounds obvious, but most ways of quieting a noisy fuzzer — narrowing its inputs,
weakening its assertions, catching its panics, disabling it — also destroy its
ability to find bugs. The six principles below explain how to avoid that. Each
ends with an **In practice** note grounding it in this codebase, drawn from the
fuzzing literature (see [Further reading](#further-reading)).

---

## 1. A fuzzer is a generator plus an oracle — and the oracle must be independent of the code under test

A fuzz target has two parts: the engine that *generates* inputs (libFuzzer does
this for you) and the *oracle* that decides whether an output is wrong. The
oracle is the part with all the leverage, and the part that's easy to get wrong.

The cardinal mistake is an oracle that re-derives the system's own logic and then
checks the system against it: it shares the system's faults and proves nothing.
A good oracle is **independent** of the implementation. In rough order of
strength:

- **Round-trip / inverse** — `decode(encode(x)) == x`, `parse(print(x)) == x`.
- **Differential** — compare the real function against a trusted reference, a
  prior version, or an alternate implementation.
- **Metamorphic** — relate the outputs of related inputs (e.g. monotonic:
  `x ≤ y ⇒ f(x) ≤ f(y)`; idempotent; commutative) without knowing the exact
  expected value.
- **Explicit invariants / post-conditions** — properties that must always hold.
- **The implicit oracle** — any panic, abort, or sanitizer trip. Free, and the
  baseline every target gets.

**In practice.** Drive the *real* contract functions, never a copy of them: call
`BorrowPosition::liquidatable_collateral` and the real `Price`/`Valuation`
conversions rather than hand-rolling liquidation or price math in the harness
(the kind of toy reimplementation that left `fuzz_liquidations` and
`fuzz_price_calculations` testing arithmetic the harness wrote itself). If the
real function isn't reachable from the fuzz crate, expose a thin shim in the
source crate — like `templar_soroban_runtime::test_utils::fuzz_api` — instead of
duplicating logic. And an oracle is only as good as it is *complete*: a target
that asserts "if my pre-checks pass, `validate()` must succeed" gives false
positives unless it enumerates *every* rule `validate()` enforces. When you can't
guarantee completeness, downgrade the claim to the always-safe one — "the
function does not panic and returns a well-formed `Result`."

---

## 2. Bug-finding power ≈ oracle strength × reachable input domain

Anything that shrinks either factor shrinks the product. Weakening an assertion
and narrowing the input domain are therefore *the same mistake*, and both deserve
the same scrutiny: you are trading away bugs the target could have found.

This is the principle behind the most common "fix" that quietly guts a fuzzer:
restricting inputs so a crash can't occur. Sometimes a bound is legitimate — to
stop the fuzzer re-discovering one *intentional* abort instead of exploring
logic — but only when it is **targeted** (it dodges one specific, named check,
not "overflow in general"), **documented** (a comment says which check and why
it's not this target's job), and **backstopped** (the excluded region is covered
by another target that drives values *to* the limit and asserts the abort).

**In practice.** Capping borrow amounts at `u64` so `principal + in_flight +
interest + fees` can't overflow `u128` is only acceptable if a dedicated boundary
target still pushes that sum to `u128::MAX` and asserts the intended abort —
removing both at once erases the overflow property entirely. The good model is a
bound that caps *one* quantity and states what it excludes:
`fuzz_vault_state_machine`'s `truncate_plan` and the `MAX_RAW_DECODE_LEN` cap in
`fuzz_soroban_storage_codecs` both do this. On the oracle side, every assertion
must be able to fail: `assert!(total_collateral >= deposit)` where
`total_collateral` *is* the deposit field, or `assert!(sum >= addend)` for a sum
of non-negative terms, are decorations that test nothing — replace them with
round-trips, differentials, or invariants a buggy change could actually violate.

---

## 3. A fuzzer only tests the code it actually reaches

Iteration count is worthless if the inputs bounce off an early validation check.
Getting *past* validation into the deep logic is the harness's real job, and it's
won with structured input, not raw volume. An input filter that rejects values
the contract itself would accept silently amputates whole regions of behavior.

**In practice.** Use `#[derive(Arbitrary)]` to build well-formed structured
inputs rather than hoping random bytes parse. For stateful code, generate an
*arbitrary sequence* of real operations and re-check invariants after each step —
`fuzz_vault_state_machine` (an `Action` enum applied in a loop) is the reference
pattern, and it finds order- and accumulation-dependent bugs that one-shot
field-setting never will. Don't filter out inputs the contract would process in
production. When in doubt, measure: coverage tooling tells you whether a target
reaches the logic you think it does.

---

## 4. A crash is a discovery — triage it, never suppress it

When a target panics, the default assumption is *the fuzzer found something*. The
engine's contract is literally "if it aborts, it's a bug." There are exactly
three outcomes, and suppression is not one of them:

1. **Real bug** — file it, and keep the crashing input as a regression seed.
2. **Intentional abort** (e.g. a checked-arithmetic overflow that is a deliberate
   safety property) — assert the *boundary* so the target proves the abort fires
   exactly when it should; don't make the boundary unreachable.
3. **Harness bug** — constructing a type while bypassing its real constructor's
   invariants, or calling a removed/renamed API — fix the harness.

Disabling a target with a `TODO` is the last resort, never the first, and must
carry a tracked issue and an owner.

**In practice.** `fuzz_decimals` was switched off after `Exponential2::at`
panicked via `.pow2().unwrap()`. Since `Exponential2::new` bounds
`eccentricity ≤ 24` and `at` requires `usage_ratio ≤ 1`, the result should fit in
`Decimal` — so either the harness built the curve bypassing `new()`'s invariants
(harness bug) or there's a genuine in-bounds overflow (real bug). Disabling it
discarded the answer. The half-dozen commented-out `[[bin]]` entries are coverage
debt in exactly this way: each is a deferred triage, not a steady state.

---

## 5. Validate the fuzzer itself — an oracle that cannot fail is theater

A target that has never found anything is indistinguishable from a target that
*can't* find anything. Confidence comes from evidence that the oracle bites:
inject a known fault into the code under test and confirm the target catches it
(lightweight mutation testing), and watch coverage to confirm the target reaches
what you intend.

**In practice.** Keep a `// MUTATION-CHECK:` note in each target describing a
one-line edit to the real code that *should* make the target fail (e.g. "flip
`>=` to `>` in `get_total_borrow_asset_liability`" or "drop the overflow check").
Run that check whenever you materially change a target — if the mutation doesn't
trip it, the target isn't testing what you think.

---

## 6. Fuzzing is continuous and reproducible, not a one-shot exercise

Targets are first-class code. They live in the repo, build in CI, ship a seed
corpus, and every crash becomes a regression seed. Two properties make this work
and are easy to lose: **determinism** (a non-deterministic target can't be
triaged or reproduced — avoid wall-clock, RNG, and global state) and **speed** (a
slow target explores less of the space; aim for a tight, allocation-light body).

Keep one distinction sharp: a **narrow target *scope*** — one entry point or one
data format per target — is *good* (faster, cleaner triage; split a multi-format
target into several). A **narrow input *domain*** — clamping the values a target
will explore — is the *suspect* move from principle 2. They sound alike and are
opposites.

**In practice.** Keep targets compiling against current APIs — a target that
references a removed function rots and gets disabled, quietly dropping coverage;
and don't trust a stale `TODO` (one claims `SupplyPosition::can_be_removed` was
removed — it still exists at `common/src/supply.rs:77`). Run the suite via
`run_fuzzing.sh` / CI, and record *any* reduction in what's fuzzed — a disabled
target, a tightened domain, a dropped assertion — in `README.md`'s status tables.
A buried change reads as "this is fine" to the next person; an explicit one reads
as debt to pay down.

---

## Checklist for a new or edited target

- [ ] Calls the **real** production function, not a reimplementation (P1).
- [ ] Its oracle is **independent** of the code under test; if it can't be made
      complete, it asserts only "doesn't panic / well-formed `Result`" (P1).
- [ ] Every `assert!` can actually fail if the code is wrong — no tautologies (P2).
- [ ] Any input bound is **targeted + documented + backstopped** elsewhere (P2).
- [ ] Uses structured `Arbitrary` input and, for stateful code, an action
      **sequence** that re-checks invariants per step (P3).
- [ ] Any panic it can hit is a tracked bug, or an intentional abort it
      **asserts** on — never one dodged by input shaping (P4).
- [ ] Carries a `// MUTATION-CHECK:` note describing an edit that should make it
      fail (P5).
- [ ] Deterministic, fast, builds against today's APIs (P6).
- [ ] Any coverage reduction is recorded in `README.md` (P4, P6).

---

## Further reading

- Google fuzzing — [*What makes a good fuzz target*](https://github.com/google/fuzzing/blob/master/docs/good-fuzz-target.md) (determinism, speed, narrow scope, "if it aborts it's a bug", seed corpus)
- Klees, Ruef, Cooper, Wei, Hicks — [*Evaluating Fuzz Testing*](https://arxiv.org/abs/1808.09700), CCS 2018 (measure coverage *and* bugs; don't assume efficacy)
- Schloegel et al. — [*SoK: Prudent Evaluation Practices for Fuzzing*](https://arxiv.org/pdf/2405.10220), 2024
- Trail of Bits — [*The call for invariant-driven development*](https://blog.trailofbits.com/2025/02/12/the-call-for-invariant-driven-development/), 2025 (function- vs system-level invariants; Hoare-triple framing)
- SWEN90006 — [*Property-based Testing and Test Oracles*](https://swen90006.github.io/Property-based-testing.html) (oracle taxonomy: metamorphic, alternate-implementation, golden-program)
- OSS-Fuzz — [*Ideal integration*](https://google.github.io/oss-fuzz/advanced-topics/ideal-integration/)
- [Rust Fuzz Book](https://rust-fuzz.github.io/book/) — structure-aware fuzzing with `Arbitrary`
