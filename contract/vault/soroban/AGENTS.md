# Soroban Vault Agent Guide

This file contains Soroban-specific guidance for future agents working in `contract/vault/soroban`.

## Scope

This directory is the Soroban executor layer for the shared vault kernel plus the closely related
Soroban-side companion contracts:

- `templar-soroban-runtime` in `contract/vault/soroban`
- `templar-soroban-governance` in `contract/vault/soroban/governance`
- `templar-soroban-share-token` in `contract/vault/soroban/share-token`
- `templar-soroban-blend-adapter` in `contract/vault/soroban/blend-adapter`
- shared ABI/types in `contract/vault/soroban/shared-types`

Read these first before making non-trivial changes:

- `contract/vault/soroban/README.md`
- `contract/vault/soroban/STRIDE.md`
- `contract/vault/soroban/SIZE_BUDGET.md`
- `contract/vault/soroban/src/contract/entrypoints.rs`
- `contract/vault/soroban/src/effects/mod.rs`
- `contract/vault/soroban/src/storage/mod.rs`
- `contract/vault/soroban/governance/src/lib.rs`
- `contract/vault/soroban/share-token/src/lib.rs`
- `contract/vault/soroban/blend-adapter/src/lib.rs`
- `contract/vault/soroban/shared-types/src/lib.rs`

## Why This Area Is High Risk

- The runtime is the Soroban execution boundary for the canonical vault kernel state machine.
- It owns custody, storage serialization, address mapping, auth enforcement, and effect execution.
- Soroban artifact size is a shipping constraint, not a cleanup task.
- Governance is split across contracts: timelock/orchestration lives in the governance contract,
  but the runtime still applies the canonical state changes.

## Canonical Invariants

Preserve these unless the architecture is being changed deliberately and the change is documented,
measured, and re-verified:

- The runtime is the sole canonical owner of custody, `total_assets`, `total_shares`, fee anchor,
  payout settlement, and accepted external-asset accounting state.
- Governance proposal submission, timelock maturity, approval/revocation, and abdication live in
  the governance contract, but vault-bound mutations are still applied by the runtime via
  `execute_governance(env, caller, payload)`.
- The share token only allows vault-authorized `mint()` and `burn()`. User transfers still require
  `from.require_auth()`.
- Read-only preview and getter surfaces must stay non-authoritative.
- Serialized `VaultState` remains a practical resource boundary because Soroban persists a single
  `StateBlob`. Pending withdrawals are the main long-lived growth vector.
- `extend_ttl()` is permissionless and is part of the operational liveness model.

## Binary Size Gate

The Soroban runtime deploy artifact must stay at or below `128 KiB` (`131072` bytes).

Use these commands:

- `just -f contract/vault/soroban/justfile build`
- `just -f contract/vault/soroban/justfile size-budget-check`
- `just -f contract/vault/soroban/justfile wasm-analyze 250 120`
- `just -f contract/vault/soroban/justfile wasm-analyze-print all 120`

Important details:

- The size gate checks `target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm`.
- `build` emits the optimized runtime WASM and uses that same artifact for deployment. It keeps
  contractspec metadata so standard Stellar CLI invocation and explorer source-attestation tooling
  can inspect the deployed WASM.
- Recent local evidence records the unstripped deploy artifact at `128955` bytes (`125.9 KiB`),
  leaving roughly `2.1 KiB` of headroom under the `131072` byte gate.

Common growth pitfalls:

- Duplicate normalization logic at runtime boundaries
- Extra allocation-heavy helper layers in hot or state-heavy paths
- Generic-heavy shared helpers and command surfaces that increase monomorphization
- Public ABI/event/spec changes that pull in more generated metadata

When the size gate fails:

1. Rebuild with `just -f contract/vault/soroban/justfile build`.
2. Run `just -f contract/vault/soroban/justfile size-budget-check`.
3. Run `just -f contract/vault/soroban/justfile wasm-analyze 250 120`.
4. Prefer deleting duplicated logic or narrowing compile surface before introducing new helper
   layers or architectural churn.
5. Treat low-delta slimming as the default path. Escalate to a topology split only if measured
   evidence still leaves the runtime above `131072` bytes.

## Release WASM Inspection Workflow

Do not stop at `size-budget-check`. When the artifact grows, inspect the built release artifacts
directly and keep the commands/results in the task notes or PR description.

Release artifact to inspect after `just -f contract/vault/soroban/justfile build`:

- `target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm`

Recommended workflow:

1. Confirm exact byte sizes.
2. Inspect section layout with `wasm-objdump`.
3. Use `twiggy` to find the biggest retained items and dominator chains.
4. If needed, dump WAT and inspect suspicious symbols, long match arms, repeated helpers, or
   unexpectedly large data/custom sections.
5. Compare before/after outputs when evaluating a refactor. Do not rely on intuition.

Commands:

- `stat -c '%s %n' target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm`
- `wasm-objdump -h target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm`
- `wasm-objdump -x target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm > /tmp/templar_soroban_runtime.objdump.txt`
- `twiggy top target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm -n 80`
- `twiggy dominators target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm`
- `twiggy monos target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm`
- `wasm2wat target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm -o /tmp/templar_soroban_runtime.wat`

What to look for:

- Large `code` or `data` sections in `wasm-objdump -h`
- Unexpected custom sections that survived stripping
- Large monomorphized functions in `twiggy monos`
- Dominator chains caused by generic-heavy helpers, large enum dispatch, or duplicated conversion
  paths
- ABI/spec/event-related growth after adding command variants, event payload fields, or public
  methods
- Helper layers that look small in Rust source but retain large downstream call trees in `twiggy`

Interpretation guidance:

- If `templar_soroban_runtime.wasm` grew unexpectedly, inspect retained code shape with `twiggy`.
- If section growth is concentrated in custom/data sections, inspect serialization payloads,
  event/spec metadata, and embedded strings before changing control flow.
- If code growth is concentrated in a few dominators, attack those first. Small deletions there
  can collapse a large retained subtree.
- Prefer comparing `twiggy top`, `twiggy dominators`, and `wasm-objdump -h` before and after a
  patch. Size debugging should be evidence-driven.

## Codegen Reduction Tips

Primary reference:

- Rust and WebAssembly code-size guide: <https://rustwasm.github.io/docs/book/reference/code-size.html>

This workspace already uses strong size-oriented release settings in the root `Cargo.toml`:

- `codegen-units = 1`
- `opt-level = "z"`
- `lto = "fat"`
- `panic = "abort"`
- `strip = "symbols"`
- Soroban release profile with `overflow-checks = false`

Do not casually undo those. Also remember that Cargo only reads profile settings from the
workspace-root `Cargo.toml`, not from member crates.

When the runtime bloats, check these levers before reaching for a bigger refactor:

- Measure `opt-level = "s"` versus `opt-level = "z"` for `release-soroban`. Rust’s official docs
  note that `"s"` can sometimes produce a smaller binary than `"z"`. Do not assume `"z"` wins.
- Keep release builds non-incremental and size-oriented. Incremental compilation trades away some
  optimizations and is not the right mode for artifact measurement.
- Confirm debug info and symbol/name sections are actually gone before changing source. Cargo’s
  profile docs and the Rust/Wasm guide both note that debug info and the wasm `names` section can
  be major size contributors. In this repo, `debug = false`, `strip = "symbols"`, and the
  optimizer/strip pipeline should already remove them, so unexpected growth here usually means a
  build-path regression.
- Check for duplicate crate versions with `cargo tree -d`. Cargo’s docs note that avoiding multiple
  versions of the same package can help executable size as well as build time.
- Inspect feature flow with `cargo tree -e features` and `cargo tree -e features -i <crate>`.
  Cargo feature unification can silently turn on defaults or widen a dependency’s surface far
  beyond what this crate asked for directly.
- Prune dependency features aggressively. In size-critical crates, prefer `default-features = false`
  when viable, and do not enable convenience features in the runtime unless they are required in
  shipping WASM.
- Treat every new public method, event field, command variant, and shared-type variant as a size
  decision. Public ABI growth often pulls both more code and more spec metadata into the artifact.
- Reduce monomorphization pressure. Be suspicious of generic-heavy helper layers, duplicated
  conversion helpers, broad enums, and wrappers that instantiate the same logic across many type
  combinations.
- Audit inlining decisions explicitly. `#[inline]` and especially `#[inline(always)]` can improve
  speed while making wasm larger by cloning helper bodies into many call sites. In size-critical
  code, do not add inline attributes casually, and re-check existing ones when a function starts
  dominating `twiggy` output.
- For hot generic helpers used with many concrete types, consider whether a trait-object or
  non-generic boundary would be smaller. The Rust and WebAssembly guidance explicitly calls out
  generic monomorphization as a common source of wasm bloat. Do not change dispatch style blindly;
  measure the tradeoff.
- Dynamic dispatch is a size lever, not a default style rule. If the same generic helper is being
  instantiated across many concrete types, a trait object like `&dyn Trait` can collapse the code
  to one emitted implementation. The tradeoff is lost specialization and indirect-call overhead.
  Use it only where the call frequency and optimization loss are acceptable.
- Consider `#[inline(never)]` on cold, reused helpers if `twiggy` or LLVM IR suggests they are
  being inlined into many call sites and inflating the code section. Do this surgically and only
  after measurement; forcing no-inline on hot code can hurt performance and occasionally even block
  other size wins.
- Avoid string formatting in release-critical runtime paths when static strings or compact error
  codes are sufficient. The Rust and WebAssembly guidance calls out `format!`, `to_string`, and
  related formatting machinery as a common source of code growth.
- Avoid panicking paths in shipping runtime code when a normal error return will do. Panics and
  panic formatting can retain surprising amounts of code. If an invariant truly cannot fail, keep
  the path minimal and document why.
- Reuse the existing kernel abort helpers in
  [contract/vault/kernel/src/abort.rs](/data/projects/contracts/contract/vault/kernel/src/abort.rs)
  when you need a documented impossible-path trap on wasm32. In particular, prefer the existing
  `abort!`, `unwrap_abort!`, and `unwrap_abort_result!` macros over introducing fresh panic-heavy
  helpers. These already compile to `core::arch::wasm32::unreachable()` on wasm32 while preserving
  a normal panic in non-wasm test builds.
- Watch for implicit panics, not just `panic!()`: indexing, division, and `unwrap()` can all pull
  in panic machinery. Prefer `.get()`, checked arithmetic, and explicit error handling where that
  preserves the intended semantics.
- Prefer one shared implementation over many near-identical helpers, but only if the shared path
  does not introduce a larger generic surface. Measure both shapes if unsure.
- Be careful with derives and serialization surface area. Extra derived impls and broad serde/postcard
  reachability can retain code that looks cheap at the source level.
- If `twiggy` shows allocator symbols such as `dlmalloc`, `__rust_alloc`, or `__rust_realloc` high
  in the retained-size list, treat allocation reduction as a first-class size tactic. The Rust and
  WebAssembly guide notes that the default wasm allocator is roughly ten kilobytes. Avoiding
  allocation entirely is best; switching allocators is only worth considering after compatibility
  and runtime-cost review.
- Re-check whether the growth is in the Rust-generated code or in custom/data/spec sections before
  editing logic. If the bytes are not in `code`, logic changes may be the wrong fix.
- If the optimizer path changes, compare the raw release artifact against the optimizer output.
  The Rust and Wasm guidance notes that post-processing with `wasm-opt -Os` or `-Oz` can save
  additional size, and our Soroban build already relies on the Stellar optimizer path for this
  reason.
- `wasm-opt` is still worth considering when the Soroban optimizer path changes or when you need an
  independent measurement. Binaryen’s docs describe `wasm-opt` as the standard post-link optimizer
  for making wasm smaller and faster. It is not currently installed in this workspace, so do not
  add it as a required step without updating the tooling docs.
- `wasm-snip` is a last-resort tool, not a default optimization pass. The Rust and WebAssembly
  guide suggests it mainly for code such as panic infrastructure that provably cannot execute at
  runtime. For contracts, do not use it unless the removed path is demonstrably unreachable and
  the result is re-optimized and re-tested.
- If `twiggy` is not enough to explain a large function, inspect LLVM IR for the release wasm build:
  `cargo rustc --profile release-soroban --target wasm32-unknown-unknown -p templar-soroban-runtime -- --emit llvm-ir`
  Then inspect the generated `.ll` file in `target/wasm32-unknown-unknown/release-soroban/deps/`.
  The Rust and WebAssembly guide recommends this when you need to see what got inlined into a
  retained function.

Practical order of attack:

1. Measure section growth with `wasm-objdump -h`.
2. Attribute retained code with `twiggy top`, `dominators`, and `monos`.
3. Check `cargo tree -d` and `cargo tree -e features -i <crate>` before assuming the bloat is in
   your own code.
4. Trim features, duplicate versions, and ABI/spec surface before changing architecture.
5. Compare `"s"` vs `"z"` if code shape changed materially.
6. Check for accidental inlining via `#[inline]`, `#[inline(always)]`, or large cold helpers being
   cloned into many sites.
7. Use LLVM IR inspection if a retained giant still lacks a clear source-level cause.
8. Escalate to a larger split only after the measured low-delta levers are exhausted.

## Security Notes

- `initialize()` is highly sensitive because front-running it would seize governance/curator
  control. Keep deployment and initialization assumptions explicit.
- Review auth on every privileged Soroban entrypoint. Do not rely on outer routing alone.
- Soroban transactions are atomic, but adapter correctness, state ordering, and accepted external
  asset snapshots still matter for accounting safety.
- Changes to postcard serialization, versioning, migration, or storage keys are security-relevant.
- The kernel-to-Soroban address mapping is critical for effect routing. Treat changes there as
  high impact.
- `withdraw()` and `redeem()` include an idle-only atomic path that bypasses the queued withdrawal
  lifecycle. Do not change queue semantics without checking both paths.
- Governance abdication is irreversible. Any change to governance action kind mapping or timelock
  policy needs a high-suspicion review.
- `RemoveMarket`, skim recipient changes, and share-token authority changes can all become
  authority drift or asset-loss bugs if altered casually.

## Working Norms

- Prefer small, measurable patches over broad Soroban refactors.
- If you touch anything that can affect size, measure the artifact after the change.
- Preserve the existing contract split: runtime, governance, share-token, and blend adapter each
  have distinct authority boundaries.
- Avoid broad shared-type expansion. Shared ABI crates can silently increase runtime size.
- Be explicit about wire formats, storage formats, and event payloads.
- Keep tests out of runtime implementation modules. Do not add inline `#[cfg(test)] mod tests`
  blocks to files such as `src/contract/entrypoints.rs`; put runtime tests in `src/tests.rs` or
  integration tests under `tests/`.
- If behavior changes around governance bridging, withdrawal lifecycle, or share-token auth,
  update the docs in this directory in the same change.

## Verification

Minimum runtime verification:

- `cargo test -p templar-soroban-runtime -- --nocapture`
- `cargo test -p templar-soroban-runtime --test integration_tests -- --nocapture`
- `cargo test -p templar-soroban-runtime --test property_tests -- --nocapture`

Size verification:

- `just -f contract/vault/soroban/justfile build`
- `just -f contract/vault/soroban/justfile size-budget-check`

When relevant, also run:

- `cargo test -p templar-soroban-governance -- --nocapture`
- `cargo test -p templar-soroban-share-token -- --nocapture`
- `cargo test -p templar-soroban-blend-adapter -- --nocapture`

Use `wasm-analyze` when:

- the deploy artifact grows unexpectedly
- you add a new shared type, command variant, event shape, or generic helper
- you move code across runtime/governance/share-token/adapter boundaries

Use direct `twiggy` and `wasm-objdump` on the release artifacts when:

- a change regresses `size-budget-check`
- you need to know whether growth came from code, data, or custom sections
- you need to attribute bloat to a specific retained function tree rather than a crate-level guess

If an important verification step cannot run, say so explicitly.

## Documentation Maintenance

- Keep `README.md`, `STRIDE.md`, `SIZE_BUDGET.md`, and this file aligned.
- Update this file when Soroban build paths, size rules, authority boundaries, or verification
  commands change.
