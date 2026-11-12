# PR #699 review fixes

Fixes for the 10 `/code-review` findings on `fix/proxy-oracle-audit-remediation`. Nothing here is committed.

## Finding 1: removed feeds without a breaker tombstone (documented blocker)

Feeds that older code removed have a cache epoch but no breaker-allocator tombstone. Re-adding one makes `admin_set_proxy` panic with `Invalid circuit breaker set`. You chose to keep this as a blocker and document it, so there is no contract change.

- `tools/manager/src/dispatch/upgrade_preflight.rs`: new `upgrade.removed_feed_allocators` check. It reads the oracle's `CacheEpochs` and `CircuitBreakers` key indexes through `view_state` (`oracle_map_ids`, prefix `[map byte, b'i']`). It reports **Warned** with the price ids that have an epoch but no breaker set. If the inventory can't be read, it reports **Failed** so the gap isn't silently skipped. The status logic is the pure `removed_feed_allocator_status`.
- Test: `removed_feed_without_breaker_set_is_reported`.
- `contract/proxy-oracle/README.md`: the tombstone paragraph now names the panic and the preflight check, and the check is listed with the other preflight checks.
- Not verified against a live node. RPC `view_state` may refuse large accounts, and in that case the check fails closed.

## Findings 2 and 3: pending-proposal byte accounting on a legacy queue (NEAR governance)

The byte totals are new in this PR, so every deployed queue has none. Before this fix, any non-empty legacy queue had to be drained before new proposals could be admitted.

- `contract/proxy-oracle/near/governance-contract/src/lib.rs`:
  - When no totals are stored (every queue left by a pre-PR release), `measured_pending_usage()` sums the active proposals and only accepts the result if it fits the working-set and cancellation-log budgets. Stored totals are trusted as is: no rollback is planned, so they can't go stale.
  - Proposal creation uses the measured usage. If the legacy queue is too large it panics with `Drain legacy proposals before activating pending byte accounting`.
  - Role revocation measures the same way when other proposals are queued, and panics with `Drain legacy proposals before revoking roles` if they don't fit.
  - `release` panics with `Corrupt pending proposal accounting` instead of wrapping on underflow.
  - Cancel and execute release the proposal's bytes from stored totals; with no stored totals they leave the key absent.
- `governance-contract/src/tests.rs`:
  - Replaced `legacy_queue_must_drain_before_byte_accounting_activates` with two tests:
    - `unaccounted_legacy_queue_is_measured_on_admission_and_revocation`
    - `oversized_legacy_queue_still_blocks_admission_and_revocation`
  - Added the helpers `legacy_create`, `queue_usage` and `stored_usage`.
- README: the aggregate-limits paragraph says a legacy queue is measured on first use and that upgrades are forward-only, since a rollback with queued proposals leaves the totals wrong.

## Finding 4: HAL-7 fix (no change)

You chose to keep it as is.

## Finding 5: preflight rejected operations on a breaker a queued add installs

`pending_breaker_lifecycles` flagged later queued operations on a breaker that an earlier valid queued `AddCircuitBreaker` would install.

- `upgrade_preflight.rs`: `breaker_lifecycle_problem` now takes the set of queued installs. Checks against existing breakers are deferred and resolved by the new pure `queued_lifecycle_problems`, which also keeps `pending_breaker_lifecycles` under clippy's `too_many_lines`.
- Test: `queued_add_satisfies_later_operations_on_the_breaker_it_installs`.
- README: notes that a target installed by a valid queued add counts as present.

## Finding 6: missing period in CLI help

- `tools/manager/src/commands/proxy_oracle/governance/create_proposal.rs`: added the missing period to the `--id` help text.

## Finding 7: inaccurate migration doc

- `governance-contract/src/state/migration.rs`: the doc said the wrong role could cancel the migrated proposal. It now says it is executable by `ManualTripper` and only its creator or an Admin can cancel it.

## Finding 8: misleading aggregator doc

- `contract/proxy-oracle/kernel/src/proxy/aggregator/mod.rs`: the doc now says these are structural admission errors and that decoding a stored configuration doesn't apply them.

## Finding 9: ad-hoc `json!` in preflight tests

- `upgrade_preflight.rs` tests: replaced the `json!` payloads with the typed `LegacyOperation::AddCircuitBreaker { .. }` and a module-level `AbsoluteRearmArgs { id, breaker_id, armed_after_ns }` test struct. This follows the CLAUDE.md rule against ad-hoc JSON.

## Finding 10: Soroban governance loaded a proposal group twice

Cancel and execute loaded the proposal, then `remove_proposal` loaded its group again.

- `contract/proxy-oracle/soroban/governance-contract/src/storage.rs`: new `StoredProposal { id, slot, group, proposal }` and `find_proposal`. `load_proposal` wraps `find_proposal`, and `remove_proposal` takes a `StoredProposal` and reuses its cached group.
- `soroban/governance-contract/src/lib.rs`: cancel and execute use `find_proposal` and pass the result to `remove_proposal`.
- `soroban/governance-contract/src/tests.rs`: updated the two `remove_proposal` call sites.

## Verification

- `cargo fmt` is clean. `clippy -D warnings` is clean on the touched crates.
- `just test-fast`: 852 passed (manager, kernel, NEAR governance), before the ledger mark was removed. After removing it, NEAR governance passed 50/50. Soroban governance: 44 passed.
- NEAR governance contract passes `cargo check` for wasm32.
- `just test-sandbox` for the NEAR governance and oracle contracts passed 57/57 twice at CI's block time (`NEAR_SANDBOX_BLOCK_MS=120`).
- At the local default of 40ms blocks, the sandbox runs fail some tests: patched access keys never reach finality, or a `patch_state` doesn't take effect. This happens even with the machine idle. CI passes on the branch head. I haven't run the head locally at 40ms, so these changes aren't formally ruled out.
