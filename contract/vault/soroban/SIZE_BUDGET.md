# Soroban Runtime Size Budget

The Soroban vault runtime deploy artifact must remain at or below `128 KiB` (`131072` bytes).

## Enforcement

- Local check:
  - `just -f contract/vault/soroban/justfile size-budget-check`
- CI check:
  - GitHub Actions runs the same recipe on each PR/push.

## Why this exists

Recent regressions pushed the runtime artifact significantly above budget. We now treat code size as a release gate, not an afterthought.

## Common growth pitfalls

- Adding duplicate normalization paths at runtime boundaries
- Introducing additional allocation-heavy helper layers in hot/state paths
- Expanding generic-heavy helpers that increase monomorphized code

## When the check fails

1. Measure current artifact size from CI output.
2. Use commit-level or hunk-level size bisection in clean worktrees.
3. Prefer simplifying control flow or removing duplicated runtime work before adding new optimizations.
