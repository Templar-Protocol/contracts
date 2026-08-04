# Proxy Oracle

The proxy oracle stores per-price proxy definitions on NEAR, resolves underlying Pyth/RedStone sources asynchronously, applies freshness filters, aggregates the surviving prices, gates the result through per-proxy circuit breakers, and caches the latest per-price update result.

## Directory Structure

Two no-std kernels hold the runtime-agnostic logic; each chain (`near/`, `soroban/`) wires them up with its own DTOs, storage, RBAC, and deployable contracts.

- `kernel`: shared no-std proxy, aggregation, freshness, and circuit-breaker logic.
- `governance-kernel`: shared no-std proposal-lifecycle ledger (create/cancel/execute, per-operation TTLs, pending cap). Storage- and authorization-agnostic; each runtime supplies its own RBAC and proposal-body storage.
- `near/common`: NEAR DTOs, source/request types, and versioned state.
- `near/governance-common`: NEAR governance operation/role/TTL types, events, and the contract-interface macro, built on `governance-kernel`.
- `near/contract`: deployable proxy oracle contract and callback implementation.
- `near/governance-contract`: deployable NEAR governance contract (RBAC + proposal dispatch).
- `near/lst-contract`: (legacy) LST adapter contract for transformed price feeds.
- `soroban/common`: Soroban DTOs and shared helpers (no governance types).
- `soroban/governance-common`: Soroban governance operation/role/TTL types and validation, built on `governance-kernel`.
- `soroban/contract`: deployable Soroban proxy oracle contract.
- `soroban/governance-contract`: deployable Soroban governance contract (stellar-access RBAC + dispatch engine).
- `soroban/sep40-adapter-contract`: per-feed SEP-40 adapter that re-exposes the proxy oracle's normalized prices.

## Configuration

Each proxied price should be configured with independent sources, a freshness filter, and a circuit-breaker set. Prefer multiple independent sources for important feeds; configure `min_sources` so one compromised or stale source cannot determine the price alone.

Freshness filters are mandatory for production feeds. Circuit breakers only compare accepted observations; they do not protect against stale or future-dated source prices.

Circuit breaker accepted history must be large enough for every installed rule. A zero or too-small history is effectively disabled protection, even if breakers are installed, armed, and enforced.

Use complementary breaker types: `StepwiseChange` catches sudden jumps, while `MonotonicRun` and `WindowedChangeDelta` help catch staged ramps. Avoid inert parameters such as zero streaks, windows shorter than two observations, and zero lookback windows.

History length can be configured up to 32 entries, and at most 16 breakers may be configured per proxy. Recalibrate gas and storage before raising either bound.

## Operations

Proxy and circuit-breaker configuration changes are proposal-governed. Configure the proxy and breaker history before installing breakers, then add breakers with explicit monotonic IDs. Governance proposals may be created, executed, or cancelled by an account holding the role required for that operation; `Role::Admin` is the global governance role and may act on any proposal.

An `Operation` is either **reflexive** — mutating the governance contract's own state (`SetReflexiveTtl`, `SetTargetDefault`, `SetMethodPolicy`, `SetRole`, `SelfUpgrade`) — or a **`TargetFunctionCall`**: a generic `(method_name, args, attached_deposit, gas)` call dispatched to the governed proxy oracle. Every proxy/circuit-breaker action is a target call to the matching `admin_*` method; adding a new one needs **no governance-contract upgrade** — an unlisted method is dispatchable immediately under `default_target`, and a `SetMethodPolicy` entry is needed only to grant it a non-default timelock or role. Governance treats the target payload as opaque bytes — semantic validation of the payload happens at the proxy oracle's `admin_*` method (and so fails only at execution, after the timelock), not at proposal-create time. Only this operation shape is accepted on the wire: pre-restructure JSON is rejected rather than converted, because the old typed variants carried their own authorization (`AdminFunctionCall` was Admin-only whatever it called) and reinterpreting them under the method-driven policy would create proposals with different privileges than the sender intended. Upgrade clients alongside the contract. The manager CLI keeps typed subcommands (`set-proxy`, `add-circuit-breaker`, …) that build the generic form client-side and still catch malformed input locally.

Proposals can be submitted through either of two entrypoints, which differ only in how their arguments are encoded: `create_proposal(id, operation, requested_ttl)` takes JSON and returns the stored `Proposal`, while `create_proposal_borsh` takes the same three arguments borsh-encoded and returns nothing (read the body back with `get_proposal`). Authorization, TTL resolution, create-time validation and the `Created` event are shared, so the two are interchangeable. Borsh arguments are `CreateProposalArgs`, exported from `governance-common` so a client encodes the wire shape as a type rather than reproducing an argument order by hand. Both take exactly 1 yoctoNEAR, so a proposal cannot fund its own state: the governance account stakes the stored payload itself, and a wasm-carrying proposal needs roughly 1 NEAR of free balance per 100 KB — pre-fund it or the call panics with `LackBalanceForState`.

**Borsh is opt-in, and JSON is the default on purpose.** A JSON proposal shows its whole `Operation` in explorers and indexers; a borsh one is opaque base64, so choosing it trades away the readability of the single most audit-sensitive action the protocol has. Reach for it when the payload actually needs it — anything carrying wasm, meaning a `SelfUpgrade` or an `admin_upgrade` target call. There the saving is real: JSON base64 inflates a blob by 4/3, and twice over for a target upgrade whose payload is itself JSON, which costs gas to parse (measured at 102.8 vs 39.5 Tgas over 352 KB of wasm) and eats into NEAR's 1,572,864-byte `max_transaction_size`, capping a JSON-submitted target upgrade at roughly 880 KB of wasm against borsh's ~1.17 MB. Select it with `tmplrmgr proxy-oracle governance create-proposal --encoding borsh`; the gateway refuses the request against a governance contract older than `0.3.0` rather than quietly falling back to JSON.

Per-method timelock and role come from a table-driven `GovernancePolicy`: independent reflexive timelocks, a conservative `default_target` (a long TTL and `Role::Admin`), and a `method_policies` whitelist of `{ ttl, role }` overrides for known cheap/low-privilege methods. Resolution: a listed method uses its override; any unlisted method — including one introduced by a future target upgrade — falls back to `default_target`, so it can never buy a shorter timelock or a lower role than the default. Every override's TTL is held `<= default_target.ttl` on every write, and a policy supplied from outside (init args, `--policy-file`) is parsed into `GovernancePolicy` from its wire form — rejected unless it also stays within the 180-day `MAX_PROPOSAL_TTL` ceiling and the override count cap — so a deployment cannot seed a policy that governance is unable to edit its way out of. Shortening any lock — a reflexive bucket, the target default, or a method override — is itself gated by that lock: the edit matures under at least the lock it shortens (raising or holding a lock needs only the policy-edit lock). That gate is applied when the proposal is **created**, against the policy in force at that moment, and the resulting TTL is then fixed for the life of the proposal — the same create-time binding OpenZeppelin's `TimelockController` and Compound's `Timelock` use, and what `get_effective_proposal_ttl` previews. A consequence worth knowing: a lock edit queued while a lock was short still executes on its original clock even if the lock is raised afterwards, so raising a lock does not retroactively delay proposals already queued against it. Because `SelfUpgrade` can replace the contract with arbitrary code, its reflexive timelock is the effective ceiling on all others and should be deployed as the longest lock.

The NEAR governance contract is initialized as its own contract account with `new(proxy_oracle_id, admin_id, policy)`. It seeds `admin_id` into RBAC as the initial `Role::Admin`. Legacy *embedded* proxy-oracle governance state is not migrated into it — that is not an in-place migration target. An existing standalone governance contract on v0 state is a different case and does migrate in place: `migrate({"from_version":"v0"})` seeds the policy from the old TTL table and reshapes pending proposal bodies into the generic form. Because v0 has no `SelfUpgrade` operation, that migration has to be driven with the governance account's full-access key rather than through a proposal.

`update_prices(price_ids)` performs oracle IO, aggregation, circuit-breaker evaluation, event emission, breaker-state persistence, and cache writes. `list_ema_prices_no_older_than(price_ids, age)` is a cached read only: it returns `None` when a cached result is missing, blocked, resolve-failed, or stale under the caller-provided `age`.

`update_prices` does not accept a caller freshness age. Governed proxy `FreshnessFilter` settings control source freshness during updates; caller freshness is applied only when reading accepted cached prices.

Enforcement and lifecycle are separate. Unenforced breakers still evaluate and can trip while the set has no existing blocking trip. Re-arming requires an explicit accepted-history source: empty history or observed history collected during the incident.

`get_proxy_circuit_breaker_set` exposes both `accepted_history` and `observed_history`. Accepted history is the rule baseline and only records non-blocking evaluations. Observed history records valid sampled prices even while the set is tripped or manually blocked, and should be treated as recovery/audit data until governance explicitly seeds from it.

The role a method requires comes from its resolved policy, so what a role can do depends entirely on the deployed policy table. A **fresh** deployment via `tmplrmgr proxy-oracle governance create --ttl-default …` starts with **no per-method overrides**: every target method resolves to `default_target` and is therefore Admin-only, and granting `ManualTripper` or `CircuitBreakerOperator` on such a deployment does nothing until overrides exist. Pass `--policy-file` (see `governance-policy.example.json` in this directory) to deploy a real table, or add entries later with `SetMethodPolicy` proposals. The natural table below is what the **v0→v1 migration** seeds, and what the example file reproduces: manual trip/untrip (`admin_set_manual_trip`) requires `Role::ManualTripper`; circuit-breaker lifecycle methods (`admin_rearm`, `admin_set_enforced`) require `Role::CircuitBreakerOperator`; proxy definitions and circuit-breaker configuration/add/remove require `Role::ProxyConfigurationManager`. All reflexive operations — policy edits (`SetReflexiveTtl`, `SetTargetDefault`, `SetMethodPolicy`), `SetRole`, and `SelfUpgrade` — require `Role::Admin`, since they govern governance itself. Governance roles are multi-role memberships managed with targeted `SetRole { account_id, role, set }` operations: `set: true` grants the named role and `set: false` revokes only that named role. `Role::Admin` is the global governance superuser role and may act on any proposal; removing the final `Role::Admin` membership is rejected.

Proxy oracle contract upgrades are a `TargetFunctionCall` to the proxy's owner-gated `admin_upgrade` entrypoint (the CLI's `create-proposal oracle upgrade` subcommand attaches 280 Tgas). Any other proxy-admin action — e.g. accepting ownership after an owner transfer via `create-proposal oracle call --method own_accept_owner --deposit "1 yoctoNEAR"` — is likewise a plain target call. Attach whatever deposit the target method asserts (the ownership entrypoints require one yoctoNEAR); a call that under-attaches is a rejected receipt that still consumes the proposal. Gas is caller-supplied and baked into the stored call; if a target call is under-funded it simply fails at the proxy oracle with a rejected receipt (the proposal is still consumed), leaving nothing in a broken state. The manager CLI's `--gas` accepts either raw gas units or a formatted value such as `120 Tgas`.

### Bring-up and hardening

A newly deployed oracle needs configuration immediately — proxy definitions, breaker history, breakers — so bring-up runs under a deliberately open policy and is hardened afterwards. `--ttl-default 0s` (what a market deployment's `governance.ttl_default` sets) is already that open policy: every lock is zero and every target method is Admin-only, so `create-proposal … --execute-when-ready` configures the oracle in a single pass. Use `--policy-file governance-policy.bootstrap.example.json` instead if delegates rather than the Admin should do the bring-up — same role assignments as the steady-state table, all timelocks zero.

Neither is a production policy: with every lock at zero the timelocks provide no protection at all. Harden to something like `governance-policy.example.json` once the oracle is configured.

**Deploy with the bootstrap file if you intend to harden later**, because pre-declaring the methods is what keeps hardening cheap. Shortening a lock is gated by that lock, and an *unlisted* method's current lock is `default_target` — so on a policy with no overrides, the first override you add for a method is measured against the default. Raise `default_target` to 3d and then add a 1d override for an unlisted method, and that edit is a shortening: it takes 3 days to execute. You cannot dodge it by adding the override first, either, since `SetMethodPolicy` rejects an entry whose `ttl` exceeds `default_target.ttl`.

With every method already listed at zero, none of that applies — each hardening edit raises an existing lock, and a raise needs only the policy-edit lock:

1. Raise `default_target` to its target value. Existing overrides are all zero, so the ceiling is satisfied.
2. Raise each method override to its target TTL. Each is `0 → N`, a raise, and `N <= default_target` now holds.
3. Raise `set_role` and `self_upgrade`.
4. **Raise `set_policy` last.** A proposal's TTL is fixed when it is *created*, not when it executes — so create the whole batch (or execute this one last) while the policy-edit lock is still zero and every proposal in it stays immediately executable. Raise `set_policy` first and each remaining edit inherits the new delay.

Starting from a bare `--ttl-default 0s` deployment instead, the same end state needs the default staged upward through each distinct override value — add the zero-TTL overrides while the default is still zero, raise the default to 1d and add the 1d overrides, raise it to 3d and add the 3d ones — so that every edit is a hold or a raise rather than a shortening. Otherwise accept a one-time wait equal to the default you just installed.

### Migrating a legacy NEAR proxy oracle

`tmplrmgr proxy-oracle upgrade --migration v0` accepts only a v0.1.0 source. It requires the proxy-oracle account's full-access key because legacy `migrate` is private, then submits `DEPLOY_CONTRACT` and `migrate({"from_version":"v0"})` with 250 Tgas in one transaction.

Before mainnet execution, refresh and run the production-state fixture:

```text
cargo test -p templar-proxy-oracle-near-contract --test migrate_mainnet generate_mainnet_state_patch -- --ignored
cargo test -p templar-proxy-oracle-near-contract --test migrate_mainnet migrate_mainnet_patch_exactly
cargo test -p templar-proxy-oracle-near-contract --test migrate_mainnet failed_migration_reverts_contract_code
```

These tests need `near-workspaces`, network access, and local port binding; they do not run in restricted CI environments.

The two non-ignored tests must pass after the first downloads the current account state. `migrate_mainnet_patch_exactly` deploys the checked-in v0.3.0 release into a sandbox, invokes the same migration payload, and asserts the migrated state and proxy definitions. `failed_migration_reverts_contract_code` verifies a failed migration leaves the deployed code at v0.1.0.

Verify the source version, use an audited v0.3.0 WASM, then verify the destination version:

```text
tmplrmgr --network mainnet contract get-version --contract-id <oracle-id>
tmplrmgr --network mainnet proxy-oracle upgrade --oracle-id <oracle-id> --wasm <proxy-oracle-v0.3.0.wasm> --migration v0 --signer-id <oracle-id>
tmplrmgr --network mainnet contract get-version --contract-id <oracle-id>
```

Set `SECRET_KEY` in the execution environment instead of passing a private key on the command line.

Registry-backed WASM resolution is deferred to [ENG-482](https://linear.app/templar-protocol/issue/ENG-482/support-registry-sourced-wasm-for-proxy-oracle-upgrades). Do not use `--migration v0` for another source version or retry it after a successful upgrade.

Manual-trip metadata is event-only, encoded as `Base64VecU8`, capped at 1024 bytes, and not stored in contract state. Offline manual-trip events are emitted only when the manual-trip state changes. Governance-derived circuit-breaker configuration events are emitted for successful executions, except no-op manual-trip executions do not emit a manual-trip event.

Proxy and circuit-breaker changes clear the cached price and bump an internal per-price update epoch. In-flight update callbacks whose epoch no longer matches are ignored, so stale callbacks cannot repopulate cache or mutate breaker state after configuration changes.

Circuit-breaker events use the `templar-proxy-oracle` standard and names prefixed with `circuit_breaker_*`, including configuration, add/remove, enforcement, rearm, role, manual-trip, and automatic trip events. Automatic trip events include `is_enforced` so consumers can distinguish tripped-but-non-blocking breakers from blocking trips.

Off-chain services should use the proxy oracle path for protected feeds. Falling back to direct Pyth/Hermes reads bypasses proxy aggregation and circuit-breaker semantics. The relayer and liquidator update underlying oracle sources first, then call proxy `update_prices` for market-facing proxy price IDs before dependent actions. Operators running other flows must do the same on a cadence or before actions that require fresh proxy prices; cached reads fail closed until an accepted update is available.
