# Committed fuzz seeds

This directory holds a **small, curated, version-controlled** set of inputs:
`seeds/<target>/` is fed to `<target>` alongside its working corpus on every
run. Unlike `corpus/` (the large, evolving, *gitignored* corpus that lives in
the CI cache), everything here is committed and durable.

It exists for two reasons:

1. **Regression seeds for known bugs.** Each file below is a reproducer for a
   tracked bug. The harness currently guards the triggering region, so replaying
   the seed does **not** crash today — but it *will* start crashing again if the
   guard is removed without fixing the bug, or if a fix regresses. That is the
   point: a committed regression test that rides along with the fuzzer.
2. **Coverage bootstrap.** Seeds get a fresh checkout (or a cache-evicted CI
   run, or a brand-new target) past shallow exploration immediately.

New coverage-expanding inputs are written to `corpus/<target>` (the first
positional dir), never here — so this directory only grows when someone
deliberately commits a seed.

## Current regression seeds

| File | Bug |
| --- | --- |
| `fuzz_decimals/ENG-341-piecewise-underflow-{1,2}` | [ENG-341](https://linear.app/templar-protocol/issue/ENG-341) — `Piecewise::new` unsigned underflow |
| `fuzz_liquidations/ENG-342-denominator-underflow` | [ENG-342](https://linear.app/templar-protocol/issue/ENG-342) — `liquidatable_collateral` denominator underflow |
| `fuzz_price/ENG-343-ratio-div0-{1,2}` | [ENG-343](https://linear.app/templar-protocol/issue/ENG-343) — `Valuation::ratio` div-by-zero via `pow2_int(384)` |

## Promoting a CI-found crash to a regression seed

When the nightly **Fuzz** workflow finds a crash it does **not** commit anything
— it uploads the crashing input as the `fuzz-crash-<target>` workflow artifact
(90-day retention) and opens a `fuzz-crash` issue. To make it durable:

```bash
# 1. Download the `fuzz-crash-<target>` artifact from the failed run and unzip.
# 2. Reproduce locally:
cargo +nightly fuzz run <target> <path-to-crash-file>
# 3. (optional) minimize it:
cargo +nightly fuzz tmin <target> <path-to-crash-file>
# 4. Commit it as a regression seed (name it after the tracking issue):
cp <crash-file> fuzz/seeds/<target>/ENG-NNN-<short-description>
git add fuzz/seeds/<target>/
```

Do this once the bug is filed; the seed then verifies the eventual fix and
guards against regression.
