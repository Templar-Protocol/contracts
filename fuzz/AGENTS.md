# Fuzzing — guidance for agents

**Before writing or editing any fuzz target, read [`PRINCIPLES.md`](./PRINCIPLES.md).**
It is the normative, shared guidance (for humans and agents) on what makes a fuzz
target effective — fuzz the real code, keep the oracle independent and able to
fail, treat crashes as findings, and never narrow inputs or weaken assertions to
make a noisy target quiet. The end of that file has a checklist; satisfy every box
before you consider a target done.

This file holds only the agent-specific operational notes that don't belong in the
shared doc.

## Operational notes

- **Register the target.** Every target needs a `[[bin]]` entry in `Cargo.toml`
  (`test = false`, `bench = false`). A target file with no `[[bin]]` is dead code.
- **Don't disable to make CI green.** Commenting out a `[[bin]]` removes coverage.
  If a target crashes, triage it — follow [`TRIAGE.md`](./TRIAGE.md) (reproduce →
  classify → fix → seed → verify), grounded in PRINCIPLES.md §4. When you must
  defer, open a tracked issue and add a row to the status tables in `README.md` —
  don't leave a bare `TODO`.
- **Verify the claims you write.** Don't trust an existing `TODO` at face value —
  check the API still exists, check the bound is actually needed. (A current
  `fuzz_supply` TODO claims `SupplyPosition::can_be_removed` was removed; it still
  exists at `common/src/supply.rs:77`.)
- **Reference targets to copy from:** `fuzz_vault_state_machine` (stateful,
  real-API, principled bounding) and `fuzz_soroban_storage_codecs` (round-trip +
  raw-decode, documented caps). Prefer these patterns over the legacy single-shot
  field-setting targets.
- **Build & smoke-test** a changed target before finishing:
  `cargo +nightly fuzz build <target>` then
  `cargo +nightly fuzz run <target> -- -max_total_time=60`.
- **Record coverage changes.** Any reduction in what's fuzzed goes in
  `README.md`'s status tables in the same change.
