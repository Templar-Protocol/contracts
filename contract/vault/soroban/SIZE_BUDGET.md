# Soroban Runtime Size Budget

The Soroban vault runtime deploy artifact must remain at or below `128 KiB` (`131072` bytes).

## Enforcement

- Local check:
  - `just -f contract/vault/soroban/justfile size-budget-check`
- CI check:
  - GitHub Actions runs the same recipe on each PR/push.

The check measures:

- `target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.deploy.wasm`

The justfile also emits `templar_soroban_runtime.optimized.wasm`, but the `.deploy.wasm` file is
the contractspec-stripped artifact used for deployment and budget enforcement.

## Why this exists

The runtime includes the Soroban execution boundary, storage codecs, governance bridge, token
effect routing, and the shared vault kernel. Public ABI growth, helper abstractions, and generic
serialization paths can make small source changes retain disproportionately large WASM trees. Code
size is a release gate for every runtime-facing change.

## Common growth pitfalls

- Adding duplicate normalization paths at runtime boundaries
- Introducing additional allocation-heavy helper layers in hot/state paths
- Expanding generic-heavy helpers that increase monomorphized code
- Adding public methods, events, shared-type variants, or generated spec surface
- Pulling human-readable serde/formatting paths into `wasm32` code
- Accepting convenience dependency features that widen the runtime compile surface

## When the check fails

1. Measure current artifact size from CI output.
2. Run `just -f contract/vault/soroban/justfile build` and confirm raw, optimized, and deploy
   artifact sizes.
3. Run `just -f contract/vault/soroban/justfile wasm-analyze 250 120` when source-level cause is
   not obvious.
4. Use commit-level or hunk-level size bisection in clean worktrees.
5. Prefer simplifying control flow, reducing ABI/spec surface, or removing duplicated runtime work
   before adding new optimization layers.
