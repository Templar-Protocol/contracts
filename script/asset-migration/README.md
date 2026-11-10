# Controlled local asset migration

These scripts exercise a fixed DAO-controlled migration procedure against a local
NEAR sandbox. They are not an executable HOT-to-Omni production migration.
Production submission is disabled: the real converter ABI/settlement rules and
safe intermediate-state admission remain unavailable.

Oracle pause is **not market isolation**. Deposits, accrual, debt-free collateral
withdrawals, static-yield withdrawals, and already-scheduled callbacks may change
state while a price is unavailable. Checkpoints detect observed divergence; they
do not close the interval between a precheck and a later receipt.

Verification and claim scope live in [VERIFICATION.md](../../VERIFICATION.md).

## Prerequisites

- A matching compiled `tmplrmgr`, Python 3.11+, Bash, and POSIX file locking.
- A controlled loopback sandbox containing the pinned real Sputnik fixture,
  proxy oracle/governance, supported released Market 1.6.1 state, an installed
  derived full-access MPC key, and the test-only converter.
- Transaction/outcome and block history retained for the entire campaign and
  all restarts. Configure retention before execution; a pruned known outcome
  remains unresolved and never authorizes resubmission.
- A real DAO approval threshold and governance policy/timelock. These scripts
  never vote, bypass the threshold, or advance chain time.
- Exact chain ID/genesis, converter code hash, routes, account ownership/roles,
  metadata/decimals, registrations, funding and explicit dependency scope.

The local converter consumes one exact FT/MT input and emits a plain output
transfer under a one-to-one **raw-unit** rule. It must not create a new market
deposit. This rule is test data, not a statement about HOT/Omni economics.

## Campaign input

[campaign.example.toml](campaign.example.toml) is a complete schema example
captured from a passing local run. Its fixture has stopped; its endpoint,
accounts, genesis and routes are not a live deployment. Obtain fresh bindings
together rather than copying a hash or account from another chain.

A campaign selects one or more `[[markets]]` with `account` and at least one of
`borrow_asset` / `collateral_asset`. Optional `mpc_path` overrides the native
derivation path. Asset text is `nep141:CONTRACT` or
`nep245:CONTRACT:OPAQUE_TOKEN_ID`; colons inside an MT token ID remain part of that
ID. `[[conversions]]` maps exact `old_asset` to `new_asset` identities. Both roles
can change in one guarded configuration patch.

`dependency_scope_accounts` is the explicit inspected consumer scope. Optional
`registry_accounts` add registry discovery; `external_consumers` name additional
consumers requiring inspection. A scoped registry scan is not proof that every
consumer of a public feed has been discovered. Shared feeds are handled once;
unchanged consumers still constrain resume freshness and incident checks.

`mpc_transport` is `delegate-action` or `transaction`. Only delegate actions accept
`mpc_valid_for_blocks`. Raw transaction lifetime comes from the live chain's
protocol configuration; approval and execution must fit that fixed window.
Approved bytes are never refreshed to extend it.

## Preparation and submission

```bash
export ASSET_MIGRATION_TMPLRMGR="$PWD/target/debug/tmplrmgr"
python3 script/asset-migration/00-prepare.py \
  --campaign campaign.toml --run-dir runs/local-migration

bash script/asset-migration/run.sh \
  --run-dir runs/local-migration --submit
```

Preparation is read-only on chain. It freezes normalized intent, complete raw
snapshots, code/linkage, configuration, all access keys/nonces, changed and
unchanged asset inventories, exact token metadata, oracle/governance context,
dependency scope and immutable review artifacts. Unknown layout, no-op or coupled
configuration changes, mismatched precision, unfunded storage/bonds and wrong
roles/keys fail before the prohibited write.

Independent reads use native `read-batch` with bounded concurrency. The batch
preserves each method's validation and result ordering, not a shared block or an
atomic snapshot; the scripts still recheck every financial precondition.

The initial market, dependency and oracle snapshots freeze resolved effective
code hashes as well as account linkage. A global-account provider changing code
after a configuration patch is still drift; the confirmed patch does not reset
that baseline. Full token metadata is rechecked for changed roles, unchanged
roles and scoped dependencies before preparing swap or resume proposals, at
financial write boundaries, during reconciliation and on completed-run checks.

For one market, preparation also accepts `--market`, `--borrow-asset` and
`--collateral-asset` with the shared campaign file. They conflict with an authored
`markets` list; later invocations cannot silently replace frozen input.

`--submit` authorizes the next permitted submission, not automatic approval of
DAO proposals. Without it, submitting stages stop before a write. A runner exits
at the earliest external wait. Review the persisted native source plan and exact
DAO proposal before voting, then supply the executing transaction reference and
resume the same run directory. `--wait` polls only within the campaign's finite
wait bounds; it does not cast votes or authorize retries.

Signing uses the existing manager backend. The submitter and relayer can use
keychain entries, or `ASSET_MIGRATION_SUBMITTER_SECRET_KEY` and
`ASSET_MIGRATION_RELAYER_SECRET_KEY`. Credentials are passed to the native child
through its environment, never embedded in campaign/artifact files or secret
command-line arguments. Keep the run directory and state directory private.

## Fixed stages

| Stage | Required result before progression |
|---|---|
| `00-prepare.py` | Freeze exact scoped intent and review artifacts; no chain mutation. |
| `01-register.py` | Fund only positive token-storage deficits through ordinary DAO proposals; prove the executing receipt. Already funded and zero-minimum targets retain explicit no-write evidence. |
| `02-pause.py` | DAO → governance create → effective timelock → DAO execute → exact manual-trip receipt; certify paused context. |
| `03-patch.py` | Native generic configuration generator → schema-3 plan/replay/stamp → source-bound MPC → atomic deploy/patch/restore; preserve opaque storage. |
| `04-swap.py` | One frozen transfer-call per nonzero inventory; prove actual resolver consumption, converter output, balances and unchanged market accounting. |
| `05-reconcile.py` | No unresolved attempt; exact configuration/code/linkage/raw state and inventory conservation; persist `05-reconcile/report.json`. |
| `06-resume.py` | Clear only campaign-owned manual trips after cohort reconciliation and fresh-source checks; preserve original manual/automatic incidents. |
| `07-finalize.py` | DAO refresh and actual accepted-cache receipt; positive fresh cached prices, restored identities and final reconciliation. |

Each stage listed above also accepts the same `--run-dir`, `--submit`, `--wait`
and `--execution-refs` options. A completed runner rechecks final health; old
completion is not a perpetual price freshness certificate. Failed refresh after
unpause is reported truthfully; the scripts do not automatically re-pause or
conceal the open state.

Before either manual-trip DAO approval, native `proxy-oracle predict-manual-trip`
replays the exact finalized oracle WASM and complete raw storage in an isolated
sandbox. The frozen predicted storage, including cache epochs and collection
bookkeeping for every feed, must match after the successful target receipt.
Late receipt recovery cannot replace that prediction with the current oracle
state. A foreign false→true manual-trip cycle during another feed's pending
approval therefore requires inspection, even when visible breaker values match.
Runs without this causal checkpoint cannot automatically clear manual trips.

## External execution references

Supply `--execution-refs PATH` with schema 1 JSON containing the frozen
`intent_digest` and an `operations` array. Each operation uses the exact journal
`stage`, `key`, `attempt` and `dao_proposal_id`, plus `tx_hash` and
`sender_account_id` for the transaction that **executed** the proposal. Do not
substitute a proposal-creation transaction, the first nonexecuting approval vote,
a receipt ID, or a later DAO proposal counter.

For an MPC execution whose native response was lost, additionally supply both
`execution_tx_hash` and `execution_sender_account_id` for the original signed
execution. The DAO threshold-vote reference is still required separately. Native
`dao mpc show --tx-hash HASH --tx-signer ACCOUNT` verifies that the supplied vote
actually emitted a cryptographically valid signature for the frozen payload;
a nonexecuting first vote is insufficient even with a correct execution hash.
Historical signature review does not require an unspent nonce or live payload.
Transaction/delegate inspection, source-plan/description binding and downstream
receipts must also agree before the journal can confirm execution.

Publish reference files as new files. Keep previous handoffs for review; do not
edit frozen intent, sealed plans/blobs, journal records or tip anchors.

| Exit | Meaning |
|---|---|
| `0` | This stage is confirmed, or the runner's final recheck succeeded. |
| `10` | External approval/timelock/reference wait or explicit submission authorization required. |
| `20` | Failed admission, drift, rejected operation or unsupported settlement; inspect retained evidence. |
| `30` | Unknown submission/settlement outcome; reconcile the original operation before any new write. |

Unknown outcomes retain the per-key lease. A lost response is not evidence that a
write did not happen. Do not delete the journal, release state or create another
run directory to force a retry. Successful earlier swaps remain recorded when a
later role fails; partial settlement needs inspection, not automatic resubmission.

Keep the original run directory in place: native plans and sealed blob references
bind to its files. `$XDG_STATE_HOME/templar-asset-migration` (defaulting to
`~/.local/state/templar-asset-migration`) holds cross-run key locks and durable
leases keyed by chain/genesis, account and public key. Preserve it across restarts
and RPC aliases.

## Local acceptance

```bash
NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox -p templar-manager \
  --test asset_converter --test asset_migration --test-threads 1
```

Acceptance uses the repository's CI-standard 120 ms block cadence. The native
manager still derives the actual chain validity period; there is no validity
override. A 100-block raw approval has roughly 12 seconds at that cadence,
versus roughly 4 seconds at the local 40 ms default. The complete raw workflow
has not passed at 40 ms: approvals, restarts and preserved guards can exhaust
that window, and expiry is rejected rather than silently refreshing signed bytes.

The harness supplies genuine deterministic signer responses, real Sputnik votes,
and explicit chain-time advances. Test fault injection is not implemented by
production-script votes or fake native return objects. Inspect the authoritative
verification report for exercised cases and remaining limitations.
