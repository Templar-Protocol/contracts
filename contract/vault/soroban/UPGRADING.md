# Upgrading a Soroban vault to soroban-v1.3.0-rc.1

Operator runbook for moving an existing vault stack to the `soroban-v1.3.0-rc.1` release
artifacts. Read it end to end before starting: one step (the governance cutover) cannot be
undone.

Use the `tmplr-soroban-vault` CLI built from the same release. It deploys only the pinned,
SHA-256-verified release WASMs, defaults upgrade proposals to the pinned runtime hash, and guards
the governance cutover. Put global flags (`--json`, `--yes`, `--network*`, `--source-account`)
before the subcommand.

## Supported sources

| Running | Path |
|---|---|
| `soroban-v1.1.1` release assets (commit `28374b86`) | Upgrade in place with this runbook (rehearsed end to end on a local network). |
| A vault built from tag `templar-soroban-runtime-v1.2.0` | Same runbook; runtime storage is unchanged between v1.1.1, v1.2.0 and this release. |
| Anything older (e.g. pre-v1.1.0 testnet demo bytes) | Not supported. Deploy a fresh stack and move funds; do not attempt Upgrade + Migrate. |

## What changes per contract

| Contract | Changed since v1.1.1 | Existing instance | Why |
|---|---|---|---|
| Vault runtime | Yes | **Upgrade + Migrate** through governance | All FIND fixes; two new storage keys (below). |
| Governance | Yes | **Replace**: deploy new, cut over with `SetGovernance` | Governance cannot replace its own WASM. FIND-022/024/026 only apply to a new instance. |
| Curator proxy | Yes | **Replace** after the cutover | Pins its governance at `initialize` and cannot be upgraded. It holds no vault role, so nothing else needs re-granting. |
| Blend adapter | Yes | Keeps working; **replace to claim BLND** | No upgrade entrypoint. Old adapters cannot claim BLND emissions (FIND-027); emissions accrued to an old adapter's position stay unclaimable. To capture them going forward, deploy a new adapter and move the allocation. |
| Custodial adapter | No | Keep | Source identical. |
| Share token | No | Keep | Source identical. |
| 4626 proxy | No (own source) | Keep | Uses only shared wire types; the only shared-types change is a new constant. |

The runtime calls adapters through `supply`, `progress_withdrawal` and `total_assets`, which are
unchanged, so existing adapters keep working with the new runtime.

## State migration

No persisted layout changes: `VaultState`, the fee spec codec, policy state, restrictions and
adapter bindings decode unchanged. Two instance keys are new:

- `maxnav` (MaxNavAge): absent means **24 h**.
- `navrfsh` (last full NAV refresh): absent means **stale**. While the vault holds external
  assets, deposits fail `InvalidState` until a `RefreshMarkets` covering every market with
  principal runs. Withdrawals are not gated.

`migrate` re-anchors fee accrual at current assets and time, re-validates and rewrites stored
state, extends TTLs, and then (new) rejects a stored fee recipient that is the vault, its asset
token or its share token. It fails `InvalidState` if virtual assets are configured while the
withdrawal queue is non-empty.

From `Upgrade` acceptance until `Migrate` acceptance the vault is frozen: every operation fails
`InvalidState`. The freeze lasts at least the governance migration timelock.

## Behaviour changes after the upgrade

- **Stale-NAV deposit gate (FIND-001):** run `RefreshMarkets` over every market with principal at
  least once per MaxNavAge (default 24 h), or deposits stop. Governance can change the window
  (`submit-set-max-nav-age`; `18446744073709551615` disables the gate) once the new governance is
  live.
- **Fee changes (FIND-008):** settle fees accrued under the old spec first and require an idle vault.
- **Fee recipients (FIND-009):** never the vault, the asset token or the share token.
- **Supply queue (FIND-019):** an adapter already bound to another market is rejected. Existing
  duplicate bindings are not rewritten, but any later `SetSupplyQueue` touching them fails.
- **Failed payouts (FIND-015):** a withdrawal whose receiver cannot hold the asset completes with
  zero assets out and refunds the escrow instead of wedging the queue.
- **Accounting (FIND-013, FIND-020):** negative adapter valuations count as zero; a market exit may
  realize more than its recorded principal, and the gain is recognized for the performance fee.
- **Governance (new instance only):** matured proposals expire 7 days after their ready time
  (FIND-026); `Other` approvals expire after 30 days and can be revoked (FIND-024); lowering a
  timelock waits for the longer of the kind's delay and the config delay (FIND-022).

## 1. Preflight (required)

```sh
tmplr-soroban-vault upgrade preflight --vault <VAULT>
```

It must exit 0. It checks the migration flag, stored fee recipients, virtual assets against the
withdrawal queue, idleness, duplicate adapter bindings, external assets, and the governance queue.
Fix every blocking finding **on the current runtime, before upgrading**:

- **Fee recipient is the vault/asset/share token:** `governance submit-set-fees` with valid
  recipients, then accept. Otherwise `migrate` fails and recovery costs three more proposals.
- **Withdrawal queue:** no view exposes it. If virtual assets are configured, drain the queue
  (`user execute-withdraw`; each request executes only after the vault's withdrawal cooldown) and
  only then rerun with `--attest-empty-withdrawal-queue`.
- **Governance queue:** accept or revoke every pending proposal (`governance queue`,
  `governance accept --proposal-id`, `governance revoke --proposal-id`). Pending proposals do not
  move to the new governance.

## 2. Upgrade the runtime (existing governance)

```sh
tmplr-soroban-vault deploy wasm vault
tmplr-soroban-vault governance submit-upgrade --admin <ADMIN>
# wait for the upgrade timelock
tmplr-soroban-vault governance accept --admin <ADMIN> --proposal-id <UPGRADE_ID>
```

`deploy wasm vault` uploads (or reuses) the pinned release runtime and prints its hash, which must
equal the release's `templar_soroban_runtime.wasm` SHA-256. `submit-upgrade` defaults to that hash,
refuses any other without `--allow-unpinned-wasm`, and refuses if the bytes are not installed on the
network. After acceptance the vault reports `version()` = `["1.3.0-rc.1", <flags>]` and is frozen.

## 3. Migrate (immediately)

```sh
tmplr-soroban-vault governance submit-migrate --admin <ADMIN>
# wait for the migration timelock
tmplr-soroban-vault governance accept --admin <ADMIN> --proposal-id <MIGRATE_ID>
```

Accept proposals by id. `governance accept-ready` stops at the first proposal that fails, so a
failing migrate silently blocks every later proposal behind it.

## 4. Re-open deposits

```sh
tmplr-soroban-vault curator refresh-markets --caller <ALLOCATOR> --markets <EVERY_MARKET_WITH_PRINCIPAL>
```

Deposits stay rejected until a refresh covers every market with principal. Schedule it at least
once per MaxNavAge from now on.

## 5. Replace governance and the curator proxy

```sh
tmplr-soroban-vault deploy governance --vault <VAULT> --timelock-ns <NS>
tmplr-soroban-vault governance submit-set-governance --admin <ADMIN> --new-governance <NEW_GOVERNANCE>
# wait for the governance timelock
tmplr-soroban-vault governance accept --admin <ADMIN> --proposal-id <SET_GOVERNANCE_ID>
tmplr-soroban-vault upgrade confirm-governance-cutover
tmplr-soroban-vault deploy curator-proxy --vault <VAULT> --governance <NEW_GOVERNANCE> --force-new
```

- `deploy governance` deploys the pinned release governance bound to this vault (admin defaults to
  the current governance admin) and records it as pending; the active governance is untouched.
- `submit-set-governance` refuses unless the target is bound to this vault, has the expected admin
  and runs the pinned governance WASM. **Never pass `--allow-unverified-governance` on a live
  vault**: `SetGovernance` on-chain only checks that the target is some other contract, and a wrong
  target leaves the vault with no governance that can ever act on it.
- `confirm-governance-cutover` re-verifies and promotes the new governance in the manifest.
- Then re-create on the new governance what the old one held: per-kind timelocks
  (`governance submit-set-timelock --kind <KIND> --timelock-ns <NS>`), any `Other` approvals, and
  MaxNavAge if 24 h is wrong for this vault. Point integrations at the new curator proxy.
- Choose a non-zero `--timelock-ns` for a live vault. The CLI refuses a zero timelock unless the
  global `--allow-zero-timelock` flag precedes the subcommand; use that only on test networks.

## Recovery

| Situation | Recovery |
|---|---|
| `migrate` fails `InvalidInput` (bad fee recipient) | Revoke the failed migrate proposal; `submit-cancel-migration` + accept (clears the flag; the vault runs the new runtime); fix fees with `submit-set-fees` + accept; **submit `Upgrade` again with the same hash** + accept (sets the flag again); `submit-migrate` + accept. A bare re-migrate after cancelling fails `InvalidState`. |
| `migrate` fails `InvalidState` (virtual assets + queued withdrawal) | Same as above, but drain the withdrawal queue instead of fixing fees. |
| Deposits fail `InvalidState` after the upgrade | Run `curator refresh-markets` over every market with principal. |
| `governance accept-ready` stops on a failing proposal | Accept by `--proposal-id`, or revoke the failing one. |
| Upgrade proposal for bytes not installed on the network | `submit-upgrade` refuses it. With the override, the accept fails and the vault stays on the old runtime; revoke it. |
| `SetGovernance` executed to a wrong target | Unrecoverable on-chain. Prevented only by the guarded `submit-set-governance`. |

## Mixed versions

| Runtime | Governance | Result |
|---|---|---|
| old | old | Works (starting point). |
| new, flag set | old | Frozen until Migrate or CancelMigration; old governance can do both. |
| new | old | Works, but MaxNavAge cannot be changed and FIND-022/024/026 are absent. |
| new | new | Target state. |
| old | new | Works except `SetMaxNavAge`, which fails and stays queued. This runbook never creates this state. |

## Do not

- Do not upgrade to a hash that is not the pinned release runtime.
- Do not leave a vault between Upgrade and Migrate: it is frozen.
- Do not bypass the cutover guard or cut over to an address you did not just deploy and verify.
- Do not cut over while the old governance still has pending proposals.
- Do not keep using the old curator proxy after the cutover: it still talks to the old governance.
- Do not build release WASMs on a host; the bytes embed the toolchain. Use the release assets
  (the CLI does) or the operator image.
