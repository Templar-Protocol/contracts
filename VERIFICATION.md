# HOT-to-Omni tooling verification

## Claim scope

Controlled local NEAR execution only. The production converter ABI, real
settlement semantics and safe intermediate-state admission are unavailable;
production submissions remain disabled. No production deployment, conversion,
release or PR merge was performed. Deterministic genuine Ed25519 signatures do
not establish distributed-MPC availability.

Controlled-local code is on `feat/hot-omni-4-orchestration`. The current PR713
review corrections and simplification are recorded first. Earlier checkpoints
remain explicitly historical; other PR review findings remain outside this repair.
[Published-head CI](https://github.com/Templar-Protocol/contracts/actions?query=branch%3Afeat%2Fhot-omni-4-orchestration)
and human review are separate from this local evidence; no merge approval is claimed.

The approved full-matrix environment is the repository's CI-standard
`NEAR_SANDBOX_BLOCK_MS=120`, with serialized node cases. Actual chain validity and
expiry checks are unchanged. Default-40-ms raw-workflow liveness is not claimed.

## PR713 review remediation and simplification

Addressed [royalf00l's review](https://github.com/Templar-Protocol/contracts/pull/713#pullrequestreview-5861606420),
which assessed `4158e70c`. The selected remote head was
`671fcbe1346dd0426143c3e9ce7576cbe9f6a3dc`, already containing the standalone
deployment-planning repair. Corrections were committed as `a97e3dac`; the six
simplification reviewers then inspected the frozen 66-file comparison
`40b5c157a5fcf4d87b8aa201636c7aef486372a0` →
`a97e3dac2703df4b36c5da12ed1a50f87109ce24`.
Completed simplifications and lint follow-ups are in
`d7b00d8080fcc61fbf7446ca2cf98bdf29f56642`.

This wave reviewed PR713, not the other three PRs. Normal merges propagate its
repairs through the existing stack. The unrelated untracked
`contract/proxy-oracle/soroban/halborn-re-review-request.md` is preserved; owned
`target` cache symlinks are not source changes.

### Review dispositions

| Claim | Disposition |
|---|---|
| Fresh `market plan` reads undeployed governance | Already repaired in PR713 by `671fcbe1`, not deferred to PR714. `requires_sandbox_manager_provider_preflight_redstone_only` is gated and exercises planning before target accounts exist, then actual application. The claim that only an ungated test covers this is incorrect. |
| Inline self-upgrade cannot override conservative execution gas | Added `execute-proposal --call-gas` and `create-proposal --execute-when-ready --execute-call-gas`. The latter requires `--execute-when-ready`. Omission still uses the gateway model; exact overrides retain live-limit/child-reservation checks and do not change approved bytes. Both actual CLI paths succeeded with the 355,727-byte governance WASM and 300 Tgas. |
| CI repeatedly depends on mutable current RPC code | Confirmed that the [pinned Rust-cache cleanup](https://github.com/Swatinem/rust-cache/blob/e18b497796c12c097a38f9edb9d0641fb99eee32/src/cleanup.ts) prunes this non-Cargo fixture directory. Added a separate immutable-manifest-keyed `actions/cache` entry, saved before Rust-cache cleanup, and documented the remaining cold/evicted-cache risk. The user-selected canonical current-final RPC source, both hashes and refusal to repin automatically remain; no release asset or historical-block dependency was introduced. An actual GitHub cache hit has not yet been observed. |
| Canonical token weights above `u64` fail | Raw JSON counts now parse exactly through the full `u128` range. Previously supported numeric counts remain numeric; newly supported counts above `u64::MAX` normalize to the existing decimal-string form. Modern strings remain strings. Signed zero, fractions and overflow still reject. A real canonical DAO delegated `u128::MAX` and returned an exact native numeric count; the typed gateway returned its exact decimal string. |
| Full-size test accepts any error; version coverage lost | The JSON refusal now asserts typed `RequestPreconditionFailed`, while retaining the unchanged full-size operation, same proposal ID, successful Borsh submission and complete readback equality. The existing core negative-version guard asserts `UnsupportedFeature`; codec/version coverage remains. The deleted offline-query wiring test and incidental HTTP-413 wording were not restored. |
| Production planner uses `unreachable!` | Replaced the trusted-codec shape panic with `GatewayError::Internal`. No artificial mock-only branch test was added. |
| Exact proposal bond is untested | Inspect finalized original signed transactions and assert `deposit == bond`, both for the initial 100 milliNEAR bond and after a live policy update to 200 milliNEAR. Contract overpayment acceptance cannot satisfy these assertions. |
| PR scope/publishable sandbox pin are understated | Keep the approved four-PR stack and use the retitle/description option to enumerate all nine new methods, extended `tx.get`, and snapshot-bound market reads. `get_configuration_at` has a concrete PR714 consumer. The exact sandbox dependency deliberately reaches publishable `templar-sandbox`, retaining the upstream committed-patch repair and finalized-state fences. |

Global `serde_json/arbitrary_precision` was evaluated and rejected, not shipped.
Against the same captured live-neard protocol response, an isolated executable
decoded the SDK envelope successfully without that feature and failed with it;
direct result decoding succeeded. The raw DAO boundary avoids changing shared
SDK number decoding, including the prior fail-closed signed-zero boundary.

### Simplification result

All six reports passed the supplied strict JSON Schema. The Task transport has
no native `outputSchema` parameter; validation was enforced by the main agent.

| DRY | YAGNI | State | Flow | Signal | Tests |
|---:|---:|---:|---:|---:|---:|
| 0 | 0 | 0 | 2 | 0 | 1 |

- `SIM-001 ← FLOW-002`: inline the single-use `execution_budget` calculation
  into `execute_gas`, retaining checked-arithmetic order and shared `bounded_gas`.
- `SIM-002 ← FLOW-001`: keep the governance test's receiver and `FunctionCall`
  typed until Sputnik's private-field JSON boundary; remove whole-transaction
  JSON serialization/indexing. Stored-kind equality, both threshold votes,
  explicit-gas assertions and detached-child-failure coverage remain.
- `SIM-003 ← TEST-001`: remove only the assertion about the locally built
  fixture's missing `proposal` key. Complete serialized-value comparisons still
  enforce flattened output and history semantics.

All three accepted items are complete. Deduplicated: 0; rejected: 0; subsumed: 0.
Final Clippy exposed two test-only follow-ups, also completed: `SIM-004` borrows
fixture counts instead of taking unused ownership; `SIM-005` declares the local
canonical-count DTO before executable statements. No test or lint was suppressed.
No second reviewer wave was run.

### Standalone PR713 verification

Commands ran in `/tmp/hot-omni-fullstack-pr713`. The broad fast gate passed
1,194 tests, with 69 node/ignored cases excluded. After the test-only lint
follow-ups, all eight DAO spec tests and the real canonical full-range case
were rechecked successfully.

```bash
just test-fast -p templar-gateway-methods-spec -p templar-gateway-types \
  -p templar-gateway-client -p templar-gateway-core -p templar-gateway-catalog \
  -p templar-manager -p templar-common -p templar-sandbox \
  -p templar-gateway-methods-dispatch -p templar-gateway-testing \
  -p templar-gateway-service -p templar-proxy-oracle-near-governance-common \
  --no-fail-fast
just test-fast -p templar-gateway-methods-dispatch --lib proxy_oracle_governance_impl::tests
just test-fast -p templar-gateway-methods-spec --lib dao::tests
```

The focused gas gate passed all five cases; the DAO spec gate passed all eight.
Ten real-node cases passed across the following six targets, covering native
full-range token weight, exact live bonds, policy/threshold behavior, default
and overridden gas, frozen calls, failed child receipts, cache/freshness,
spendable storage backing, opaque MT identity and original/delegate inspection:

```bash
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale \
  -p templar-gateway-core --test dao --test proxy_cached_price \
  --test proxy_source_readiness --test spendable_balance \
  --test token_metadata --test transaction_inspection --test-threads 1
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale \
  -p templar-gateway-core --test dao \
  canonical_token_weighted_votes_preserve_the_full_u128_count --test-threads 1
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale \
  -p templar-gateway-service borsh_encoding_carries_a_proposal_json_cannot \
  --test-threads 1
```

The last two commands each passed their one selected case. The Borsh case
retains the 1.25 MB operation and typed JSON rejection.

Actual CLI smoke: built with `cargo build -p templar-manager --bin tmplrmgr`,
then ran the following one-time instrumented provider scenario. It invoked the
compiled binary for both large inline self-upgrade routes, finalized creation
before final-state reads, inspected the original 300 Tgas execution transaction,
and observed successful final receipts, consumed proposals and unchanged code,
policy and oracle identity. The complete provider scenario passed (31.314 s).

```bash
env HOT_OMNI_ROYAL_CLI=/tmp/hot-omni-fullstack-pr713/target/debug/tmplrmgr \
  NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager \
  requires_sandbox_manager_provider_preflight_redstone_only \
  --test-threads 1 --nocapture
```

The instrumentation and disposable decoder probes were removed. That environment
variable is not a retained test interface. Provider-fixture SHA-256 returned to
`3f6fd51c736eb60aa27e410a87ccae78536a8034becea380edb427de3aba1101`.

Workspace compilation, all-target Clippy for all thirteen affected packages,
formatting and whitespace checks passed:

```bash
cargo check --workspace
cargo clippy -p templar-gateway-methods-spec -p templar-gateway-types \
  -p templar-gateway-client -p templar-gateway-core -p templar-gateway-catalog \
  -p templar-manager -p templar-sandbox -p templar-gateway-methods-dispatch \
  -p templar-gateway-testing -p templar-gateway-service \
  -p templar-proxy-oracle-near-governance-common \
  -p templar-proxy-oracle-near-governance-contract -p mock-mt \
  --all-targets -- -D warnings
cargo fmt --all --check
git diff --check
```

Clippy exited successfully with only the known unknown
`clippy::ignore_without_reason` warning; its workspace setting is unchanged
between the frozen base and head. No exercised local gate remains failing.
Warm fixture checks verified SHA-256
`3b4a563dcbfb10f004bd1f912a35df0c83a6eaade8cd8f70bc626d9ab92f19b9`.
YAML parsing verified the separate cache's immutable action pin, exact file/key,
step registration order and unchanged 45-minute timeout. Cold or evicted caches still
need working current RPC access and the canonical account to retain the pinned
code; this cache is not a durable historical release archive.

Published-head CI, human/security approval, the full one-off campaign matrix and
production migration readiness are not established by these local checks.

### Merged-stack verification

Normal merges carried the verified parent through the existing chain, without
rebasing or force-pushing. The following are code checkpoints; a later report-only
commit records this evidence.

| PR | Verified code checkpoint |
|---|---|
| #713 | `d7b00d8080fcc61fbf7446ca2cf98bdf29f56642` |
| #714 | `d6d6c52f038d412883bbd40476ab3d66c79b3385` |
| #715 | `74bb10c65cacdebd4aaf07a8141bb611e11bf26c` |
| #717 | `a278842f7006ff4b6301418bec7232c28f4eefaa` |

The middle layers were also checked independently in their own worktrees,
without PR717's delegated-global RPC patch. Both workspace checks and formatter
checks passed.

| Worktree | Manager library gate |
|---|---|
| `/tmp/hot-omni-fullstack-pr714` | 619 passed; 18 node/ignored cases excluded |
| `/tmp/hot-omni-fullstack-pr715` | 632 passed; 19 node/ignored cases excluded |

```bash
just test-fast -p templar-manager --lib
cargo check --workspace
cargo fmt --all --check
```

In the root PR717 worktree, all 857 selected fast tests passed, with 20
node/ignored cases excluded. Workspace compilation and the affected all-target
Clippy gate, including compilation of local opt-in acceptance targets, passed;
only the same known unknown-lint warning remains. Formatting and whitespace
checks passed.

```bash
just test-fast -p templar-manager -p templar-gateway-methods-spec \
  -p templar-gateway-methods-dispatch -p templar-gateway-core \
  -p templar-gateway-catalog -p templar-gateway-client \
  -p templar-gateway-types -p templar-sandbox --no-fail-fast
cargo check --workspace
cargo clippy -p templar-manager -p templar-gateway-core \
  -p templar-gateway-methods-spec -p templar-gateway-methods-dispatch \
  -p templar-gateway-service --all-targets \
  --features templar-manager/asset-migration-acceptance -- -D warnings
cargo fmt --all --check
git diff --check
```

All four real-Sputnik stamped-patch cases passed (164.217 s): delegate and raw
transaction transport, live-state drift refusal and actual-period expiry.
The unmodified provider scenario passed (24.317 s), after disposable CLI
instrumentation was removed. These are focused regressions, not a rerun of the
full one-off campaign matrix.

```bash
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale \
  -p templar-manager --test dao_mpc_patch --test-threads 1
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale \
  -p templar-manager requires_sandbox_manager_provider_preflight_redstone_only \
  --test-threads 1
cargo build -p templar-manager --bin tmplrmgr
target/debug/tmplrmgr proxy-oracle governance create-proposal --help
target/debug/tmplrmgr proxy-oracle governance execute-proposal --help
```

The rebuilt CLI exposes the new auto-execution gas override alongside the
existing creation override, and retains one direct-execution override.
The delegated-global RPC type fix remains pinned at `f96c0441`; no one-off test
target was added to CI, and the NEAR job timeout remains 45 minutes.

## Earlier governance-planning CI regressions

The automatic-gas follow-up exposed two real regressions in the existing node
gate: an incidental HTTP-413 assertion, and fresh market planning trying to read
a governance account that the plan had not deployed yet. PR713's
[Test run 42967818815](https://github.com/Templar-Protocol/contracts/actions/runs/42967818815)
finished with 336 passed and those two failures before its later 45-minute
cleanup timeout. PR714 and PR715 independently reported the same failures.
These are not classified as transient infrastructure failures.

SDK repair: `4ac3611bf192223b00cb2426600a146d7ca46ca6`.
Manager repair: `bd317f3a8a3ffde7bd7c3ebdfcadb867174149a3`.
`Client::plan_governance_execution(request, &operation)` plans a known immutable
operation before deployment, using the same authoritative gas calculation,
live caps, override checks and one-yocto deposit as ordinary execution.
Ordinary `executeProposal` still reads the stored operation. The deployment
planner borrows the exact operation for execution sizing, then moves it into
the preceding create step; no fixed-gas fallback or fictitious account is used.

| Surface | Exact command / scenario | Observed result |
|---|---|---|
| Full-size codec boundary | `env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-gateway-service borsh_encoding_carries_a_proposal_json_cannot --test-threads 1` in PR713 | Passed (6.108 s). The unchanged 1.25 MB operation is rejected as JSON, stored through Borsh with automatic gas, and read back equal. Removed only the incidental HTTP-status assertion. |
| Stored-operation execution | `env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-gateway-core --test dao governance_budgets_execute_target_through_real_dao_threshold --test-threads 1` in PR713 | Passed (17.346 s), through canonical Sputnik and real NEAR receipts. |
| SDK fast/compile/lint | `just test-fast -p templar-gateway-client -p templar-gateway-methods-dispatch -p templar-gateway-core -p templar-gateway-catalog --lib`; `cargo check --workspace`; `cargo clippy -p templar-gateway-client -p templar-gateway-methods-dispatch -p templar-gateway-service --all-targets -- -D warnings`; formatting and whitespace checks | All 141 fast tests passed; one ignored generator excluded. Compilation and Clippy passed with the known unknown `clippy::ignore_without_reason` warning only. |
| Actual deployment-planning CLI | `cargo build -p templar-manager --bin tmplrmgr`, then the provider scenario with temporary fixture instrumentation invoking the compiled `tmplrmgr market plan` subprocess against real neard | CLI exited 0 and wrote the nine-step plan before target accounts existed. The scenario then applied that plan and passed its provider/feed assertions (25.993 s). Instrumentation was removed; provider-fixture bytes returned to their original SHA-256. |
| Unmodified provider regression | `env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager requires_sandbox_manager_provider_preflight_redstone_only --test-threads 1` in PR714, after removing instrumentation | Passed (24.026 s). Original fixture SHA-256 `3f6fd51c736eb60aa27e410a87ccae78536a8034becea380edb427de3aba1101` preserved; no account was predeployed to mask the failure. |
| PR714 manager gates | `just test-fast -p templar-manager`; `cargo clippy -p templar-manager --all-targets -- -D warnings`; `cargo fmt --all --check`; `git diff --check` | All 614 fast tests passed; 18 node/ignored cases excluded. Clippy and formatting/whitespace passed with the known unknown lint warning only. |

All four branches then incorporated current `dev`
`40b5c157a5fcf4d87b8aa201636c7aef486372a0` through normal merges. Documentation
conflicts preserved both the delegated-RPC/MT notes and the new planning notes.
The upstream RedStone contract was rebuilt with
`./script/prebuild-test-contracts.sh --artifact redstone-adapter`; test-WASM
SHA-256: `de2322fae574db4dc307e0f85e33076913edbe8b9162398f069e99f2c9a2d774`.

Post-merge checks:

- `just test-fast -p templar-common -p templar-manager -p templar-gateway-client -p templar-gateway-core -p templar-gateway-methods-dispatch -p templar-gateway-catalog --lib`
  — all 1,089 passed; 20 node/ignored cases excluded.
- `cargo check --workspace` and
  `cargo clippy -p templar-manager -p templar-gateway-client -p templar-gateway-methods-dispatch -p templar-gateway-service --all-targets --features templar-manager/asset-migration-acceptance -- -D warnings`
  — passed with the existing unknown `clippy::ignore_without_reason` warning.
  Formatting and whitespace checks passed.
- The following gate selected **89** normal node cases because its additional
  filter expression is additive with the recipe's sandbox filter; all passed
  (799.176 s). This includes both formerly failing cases, stored-proposal gas,
  RedStone administration/keyless upgrades, configuration rollback and all four
  retained MPC patch scenarios. No one-off acceptance feature was enabled.

```bash
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager -p templar-gateway-core -p templar-gateway-service -p templar-gateway-testing -E 'test(borsh_encoding_carries_a_proposal_json_cannot) | test(requires_sandbox_manager_provider_preflight_redstone_only) | test(governance_budgets_execute_target_through_real_dao_threshold) | binary(redstone_admin)' --test-threads 1
```

No one-off acceptance target was added to CI, no CI rerun was dispatched, and the
NEAR timeout remains 45 minutes. New-head CI and outstanding review are separate
from these local observations.

## Structured MT identity review follow-up

The edited PR717 review summary identified a real ingress defect: structured MT
components were concatenated into delimiter-separated text and parsed again.
Before repair, `contract_id = "tokens.near:prefix"` with `token_id = "suffix"`
silently became account `tokens.near` and token `prefix:suffix`; numeric components
were coerced into strings. Structured components now remain separate, reject
non-string/empty fields and account separators, and preserve opaque token IDs.
Full account validation remains in the native gateway.

- Regression below failed before the fix and passed after it. It covers native
  and reference MT objects, malformed components and valid colon-bearing IDs.
- Actual `00-prepare.py` refused four malformed authored campaigns with code 20
  before RPC, frozen intent or journal publication.
- Preserved pre-fix versus post-fix comparison found identical canonical bytes for
  12 valid FT/MT forms, including nested colons and opaque token whitespace.
- The complete/restart collateral-MT raw-transaction workflow passed (195.032 s).
  This is one existing scenario plus the ingress regression, not the full matrix.
- `cargo fmt --all`, Python syntax, whitespace and
  `cargo clippy -p templar-manager --test asset_migration --features asset-migration-acceptance -- -D warnings`
  passed. Only the known workspace unknown-lint warning remains. CI selection and
  its 45-minute limit are unchanged; this test target remains local opt-in.

```bash
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --features asset-migration-acceptance --test asset_migration structured_mt_components_are_not_reinterpreted_or_coerced --test-threads 1
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --features asset-migration-acceptance --test asset_migration complete_and_restart_without_duplicate_financial_writes::case_2_collateral_mt --test-threads 1
```

## Earlier canonical DAO fixture and governance gas checkpoint

PR #713 implementation: `e4795353df81ddb7108cf0da597f9b27e777bfb6`.
The fixture is now current finalized `view_code` from mainnet
`templar.sputnik-dao.near` (observed version `2.3.1`), not an upstream source build.
RPC code hash: `4zSoHkLjJWZm34Psd4Eq2WUXHELt6LxtKmUXpsZWAECp`.
WASM SHA-256: `3b4a563dcbfb10f004bd1f912a35df0c83a6eaade8cd8f70bc626d9ab92f19b9`.
The downloader never specifies a historical block or automatically changes pins.
A warm check verifies the pinned bytes offline; it does not prove the live account
has remained unchanged since the fetch.

| Surface | Command / scenario | Observed result |
|---|---|---|
| Canonical download | `just fetch-sputnik-fixture --refetch`; `just fetch-sputnik-fixture --check` in PR1 | Live finalized download and offline checksum check passed. Executable help, Python AST parse and wrapper Bash syntax passed. |
| Downloader integrity | Throwaway local HTTP-server smoke exercising the actual downloader entrypoint | Current/final query, cold fetch and offline warm/check passed. Wrong code hash, wrong decoded SHA-256, invalid base64, invalid WASM and RPC errors each refused without replacing verified cache; missing offline cache refused without networking. |
| DTO, schema and gas policy | `just test-fast -p templar-gateway-methods-spec -p templar-gateway-methods-dispatch -p templar-gateway-core -p templar-gateway-catalog -p templar-proxy-oracle-near-governance-common --lib` | All 178 passed; one ignored generator skipped. Canonical missing history/numeric votes and modern present history/decimal strings retain their wire forms. Numeric votes above `u64` or fractional values fail closed instead of being rounded; decimal strings support full `u128`. |
| Real DAO/governance | Rebuild with `./script/prebuild-test-contracts.sh --artifact proxy-governance`, then `env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-gateway-core -p templar-proxy-oracle-near-governance-contract --test dao --test governed_operations --test-threads 1` | All seven passed: policy/bond/threshold behavior, default and exact override budgets, frozen action values/argument bytes, detached target failure visibility, target builders, Borsh entrypoints, oracle upgrade and governance self-upgrade. |
| PR1 compile/lint | `cargo check --workspace`; `cargo clippy -p templar-gateway-methods-spec -p templar-gateway-methods-dispatch -p templar-gateway-core -p templar-gateway-testing -p templar-proxy-oracle-near-governance-common -p templar-proxy-oracle-near-governance-contract --all-targets -- -D warnings`; final `cargo clippy -p templar-gateway-core --test dao -- -D warnings` | Passed. The known workspace unknown `clippy::ignore_without_reason` warning remains; no change-specific finding remains. Formatting and whitespace checks passed. |
| PR3 typed consumers | `just test-fast -p templar-manager`; `env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --test dao_mpc_patch --test-threads 1`; manager all-target Clippy and final `--test dao_mpc_patch` Clippy | All 627 fast tests and four real-Sputnik stamped-patch cases passed with the canonical fixture. Both transports, live-state drift refusal and actual-period expiry passed. PR3's finalized-state assertion now waits for transaction `FINAL`, not an arbitrary height jump; complete raw-state/code/nonce assertions remain intact. |
| Integrated fast/compile/lint | `just test-fast -p templar-manager -p templar-gateway-core -p templar-gateway-methods-spec -p templar-gateway-methods-dispatch -p templar-gateway-catalog -p templar-gateway-testing -p templar-sandbox`; `cargo check --workspace`; `cargo clippy -p templar-manager -p templar-gateway-core --all-targets --features templar-manager/asset-migration-acceptance -- -D warnings` | All 808 fast tests passed; 20 node/ignored tests excluded. Compilation and Clippy passed with only the known unknown-lint warning. No new one-off CI target or timeout increase. |
| Automatic-gas script consumers | Two complete/restart opt-in commands below, through real CLI/shell/Python/neard | Both passed with canonical Sputnik and no script-authored outer gas override: FT/delegate/local restore and MT/raw/global-hash restore. These are two focused cases, not a rerun of all 79 script cases. Existing plans are loaded unchanged rather than replanned. |
| Actual CLI/config/docs surface | `target/debug/tmplrmgr proxy-oracle governance create-proposal --help`; corresponding `execute-proposal --help`; changed Python AST parse; opt-in manifest and fixture-guide link inspection | Passed. Help describes automatic budgets and optional overrides. Both one-off targets still require `asset-migration-acceptance`; NEAR CI remains 45 minutes. |

Current complete/restart commands:

```bash
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --features asset-migration-acceptance --test asset_migration complete_and_restart_without_duplicate_financial_writes::case_1_borrow_ft --test-threads 1
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --features asset-migration-acceptance --test asset_migration complete_and_restart_without_duplicate_financial_writes::case_2_collateral_mt --test-threads 1
```

Receipt calibration used a throwaway real-neard program at 120 ms cadence, the
canonical DAO and explicit 200 Tgas outer calls. Values below are governance
receipt gas burnt, not child reservations or total DAO execution cost.

| Operation | Create (Tgas) | Execute (Tgas) |
|---|---:|---:|
| Role edit | 1.911 | 2.268 |
| Target call with 100 Tgas reserved | 1.967 | 2.220 |
| Artificial 32 KiB method policy | 6.733 | 3.165 |

All six measured calls succeeded. An artificial 128 KiB policy sample subsequently
failed in a DAO receipt despite a governance-created event; its exact cause was
not diagnosed. It is not counted as success. These samples support conservative
allowances, not a population-level 99% guarantee for arbitrary payloads, DAO
versions, policies or multi-action proposals. The throwaway program was removed.

The following scope and evidence are earlier checkpoints, not proof against the
new canonical fixture.


## Historical simplification target

The selected target is all four PRs, each against its immediate base: 121 distinct
changed files across these frozen comparisons.

| PR | Frozen base | Frozen head |
|---|---|---|
| #713 | `415a68f547b047df69f61cd8e128bbb1f60dbd64` | `eb1fbb57a380deddb842cc4752e6ea7496c1a8e8` |
| #714 | `eb1fbb57a380deddb842cc4752e6ea7496c1a8e8` | `f1346cd71eb6910d316946e41be626508787e0a5` |
| #715 | `f1346cd71eb6910d316946e41be626508787e0a5` | `fc049c59148aa38430c6d6da361f4354327d0648` |
| #717 | `fc049c59148aa38430c6d6da361f4354327d0648` | `bbf0ae392f4c6f94e79d0fcab8ea5530bc6e4f75` |

## Earlier checkpoint evidence

| Surface | Command / scenario | Observed result |
|---|---|---|
| Gateway DAO and freshness | `just test-sandbox --stale -p templar-gateway-core --test dao --test proxy_source_readiness --test-threads 1` | Four real-neard cases passed: native policy/bond/threshold behavior, role semantics, budgeted DAO→governance→target execution, and all-source readiness. |
| Live transaction validity | Throwaway real-owned-sandbox helper plus actual compiled `tmplrmgr read chain.getTransactionValidityPeriod --json null` | Typed SDK and CLI both returned the actual 100-block window. Helper removed. No serialized `ProtocolLimits`/PatchPlan schema change. |
| Gateway and sandbox fast/catalog | `just test-fast -p templar-sandbox -p templar-gateway-testing -p templar-gateway-core --lib`; earlier spec/dispatch fast gate; catalog normal gate | Current 60 shared tests passed; earlier 101 spec/dispatch and four normal catalog tests passed. |
| Native ordered read batching | Actual compiled CLI with a heterogeneous two-read batch against real owned neard, plus write-containing rejection without partial stdout/account mutation | Passed and matched typed SDK values. All 610 manager fast tests on that isolated parent baseline passed. Root integration and final current-root fast gate remain separate. |
| Committed sandbox patches | Startup plus 16 single-submission account/code/raw-key create/reset cycles on nearcore 2.12.0 | Passed. Exact `near-sandbox = 0.3.11` selects the upstream committed-patch repair; finalized-state fences remain enabled. |
| Configuration patch | `just test-sandbox --stale -p templar-manager requires_sandbox_config_patch --test-threads 1` in PR2 | Five tests passed. The receipt-time guard failure runs under local, global-hash and global-account restoration; raw state/configuration/effective code/linkage roll back. Native gas balance and nonce are not rollback invariants. |
| Manager fast and parent node gate | `just test-fast -p templar-manager`; `just test-sandbox --stale -p templar-manager --test-threads 1` in PR3 `fc049c59` | 628 fast tests and all 19 parent sandbox cases passed after finalized-fixture propagation (496.571 s node gate). Both real-Sputnik stamped-patch transports, live drift, actual-period expiry and atomic rollback passed. This is not final current-root evidence. |
| Current integrated fast gate | `just test-fast -p templar-manager -p templar-gateway-core -p templar-gateway-methods-spec -p templar-gateway-methods-dispatch -p templar-gateway-catalog -p templar-gateway-testing -p templar-sandbox` | All 803 passed after the full-stack simplifications; 20 node/ignored tests excluded. One duplicated source-plan boundary test was removed; the complete strict-boundary and digest/binding tests remain. |
| Converter | `./script/prebuild-test-contracts.sh --artifact mock-asset-converter`, then `NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --features asset-migration-acceptance --test asset_converter --test-threads 1` | All 11 passed against the rebuilt WASM below: four FT/MT pairings, exact raw-unit settlement, failure refunds, real DepositBack credit/storage divergence, invalid routes and unauthorized/zero-input behavior. |
| Complete script workflow | Full manager sandbox gate below, through actual compiled CLI, shell and Python | All five complete/restart variants passed: borrow FT, collateral MT, mixed inventories, shared-feed cohort and original pause, including Sputnik approvals/timelock, MPC, reconciliation and accepted-cache refresh. |
| Current root compilation/lint | `cargo check --workspace`; `cargo clippy -p templar-manager -p templar-gateway-core -p mock-asset-converter --all-targets --features templar-manager/asset-migration-acceptance -- -D warnings`; `cargo fmt --all --check` | Passed. Existing workspace unknown `clippy::ignore_without_reason` warnings remain; no change-specific Clippy finding remains. |
| Artifact catalog | `just test-fast -p templar-contract-artifacts --features fetch,workspace-loader,clap --run-ignored all`; `./script/check-artifact-drift.sh` | All 63 and the drift entrypoint passed. The entrypoint runs Rust checks and is not build-free. Immutable historical release files were not hand-edited. |
| Executable entrypoints | All eight directly executable stage `--help` commands; actual runner and native read-batch/predict-manual-trip/MPC-show help; ten Python syntax parses and Bash syntax | Passed; not a substitute for chain-runtime acceptance. |
| Offline ingress and journal boundaries | Current Python implementation: complete-intent publication/reopen; missing/unknown intent fields; domain key ending `:mpc` versus explicit financial scope; authored→normalized→frozen campaign parsing | Passed. Boolean schema, unknown campaign fields/transport, zero wait and raw explicit validity refused; frozen null validity retained. Empty artifact set: no chain, snapshot-resolution or campaign-acceptance claim. |
| Predictor reconstruction regression | Current scripts against preserved pre-fix CLI, then current CLI on the same mixed workflow scenario | Before: `02-pause` refused with `complete state accounting mismatch: account reports 1055321 bytes, fetched records account for 528900`. After: the full scenario passed. Shared staging preserves source local/global identity; temporary key removal and original metadata restoration retain strict complete-state accounting. |
| Four safety regressions before repair | Preserved original indexed Python scripts with actual current CLI and neard | Dependency metadata drift prepared a swap instead of refusing; a foreign pause ABA was accepted; a nonexecuting vote recovered a completed conversion; effective provider-code drift reached an approval wait. Each failed the required refusal assertion. No unrelated fixture failure is counted as this proof. |
| Canonical delegated-global RPC decoder | Fork `f96c0441`: two consumer regressions, generator equivalence, captured successful response and actual originally-paused workflow | Before: both current RPC identifier spellings failed. After: both passed with canonical action JSON unchanged; the full real response decoded and the delegated-global workflow passed in 95.443 s. Upstream HEAD and patch share the same 148 generated-code Clippy findings and unchanged formatting failures. |
| Historical receipt retention | Transparent read-only lookup trace, then the same compiled cohort workflow with sufficient node history | Before: the original successful `03-patch` relay became `UnknownTransaction`, and an earlier block became `UnknownBlock` while the current head remained readable. After the dedicated fixture's flat `gc_num_epochs_to_keep=1000` setting: complete/restart cohort passed in 330.164 s. Normal node defaults and code-30 refusal are unchanged; diagnostic hook removed. |
| Real new manual incident | `requires_sandbox_scripts_new_manual_pause_after_partial_resume_blocks_cohort` | Passed in 176.006 s. An authorized owner changes the first resumed feed from false to true without consuming the pending campaign governance ID; restart rejects without new proposals/writes or altering the incident. |
| Historical full manager sandbox matrix (`c481ccc5`, before opt-in gating) | `NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --test-threads 1` | **109/109 passed**, 25 slow, 643 fast/other cases excluded by the sandbox partition; 9511.335 s. Uninjected local gate, including 79 script, 11 converter and 19 original manager node cases. Use `--features asset-migration-acceptance` to include the one-off targets now. |
| Current catalog and documentation | `just test-fast -p templar-gateway-catalog`; `just docs-check`; selected repository-relative Markdown targets | Four catalog tests, risk-document self-test and generated-document check passed; relative targets exist. External URLs, fragments and site-root routes were not link-checked. |
| One-off acceptance opt-in | Cargo metadata; default script-name filter through `just test-sandbox`; retained MPC delegate smoke; `--features asset-migration-acceptance` with both one-off targets | Default script filter selected zero cases; retained MPC delegate passed. Both opt-in binaries compiled and the FT→FT converter settlement case passed, with the other 89 cases filtered out. Workflow parsing confirms the 45-minute job limit and removal of the script-only node trigger. |
| Current retained node paths | `env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --test dao_mpc_patch --test-threads 1`; `env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager requires_sandbox_config_patch --test-threads 1`; `env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-gateway-core --test spendable_balance --test-threads 1` | All four real-Sputnik MPC cases, five configuration-patch cases and one live storage-backed spendable-balance case passed. Both transports, live drift, actual-period expiry and receipt-time atomic rollback remain enforced. |
| Configuration and digest compatibility | Isolated parent `just test-fast -p templar-manager market_config_patch`, `source_plan` and `ordinary_proposal`; temporary owned/borrowed decoder equivalence and pre-streaming digest capture | Configuration boundaries passed; temporary checks compared canonical JSON, Borsh and exact errors, then were removed. Two existing source-plan tests and three ordinary-proposal tests passed; permanent digest vectors pin the original mixed-action and 16 KiB deployment encodings. |
| Current focused scripts | Seven separate opt-in invocations below, through actual compiled CLI, shell, Python and neard | All 11 selected scenarios passed: both complete/restart transports, all four actual unpriced mutations, dependency metadata before swap/resume, both effective-code providers and a new manual incident after partial resume. This is focused current-tree evidence, not a rerun of all 79 script cases. |

Current converter test-WASM SHA-256:
`816ca6854413c9e8cdaf1263b3a9c3cc8f4f1505ec7163fc7b8b5dcbd8cfa4dc`.

Historical source-built Sputnik fixture used by the earlier evidence:
`5434a59dec23ee0c49df8c8c4350c208a7e75b0b`; WASM SHA-256:
`389494bc38a43140edc2717f364c8bb8bcf05a9dcc5ace3dfdb956d574a6bbb3`.

### Earlier focused script commands

Each invocation passed; together they selected 11 distinct script cases.

```sh
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --features asset-migration-acceptance --test asset_migration requires_sandbox_scripts_complete_and_restart_without_duplicate_financial_writes::case_1_borrow_ft --test-threads 1
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --features asset-migration-acceptance --test asset_migration requires_sandbox_scripts_complete_and_restart_without_duplicate_financial_writes::case_2_collateral_mt --test-threads 1
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --features asset-migration-acceptance --test asset_migration requires_sandbox_scripts_detect_actual_unpriced_mutations --test-threads 1
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --features asset-migration-acceptance --test asset_migration requires_sandbox_scripts_frozen_full_token_metadata::case_2_dependency_name_before_swap --test-threads 1
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --features asset-migration-acceptance --test asset_migration requires_sandbox_scripts_frozen_full_token_metadata::case_4_dependency_before_resume --test-threads 1
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --features asset-migration-acceptance --test asset_migration requires_sandbox_scripts_effective_provider_code_after_patch --test-threads 1
env NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --features asset-migration-acceptance --test asset_migration requires_sandbox_scripts_new_manual_pause_after_partial_resume_blocks_cohort --test-threads 1
```

## Review and operational limits

- Fresh review findings outside this PR713-specific follow-up remain unresolved:
  [decimal edit admission in #714](https://github.com/Templar-Protocol/contracts/pull/714#discussion_r4533461930),
  [ambiguous proposal-submission identity in #715](https://github.com/Templar-Protocol/contracts/pull/715#discussion_r4533480537),
  and [interrupted journal-anchor publication in #717](https://github.com/Templar-Protocol/contracts/pull/717#discussion_r4533499090).
  They are not claimed fixed by this batch; the stack is not merge-ready.
- Historical full-matrix and current focused acceptance are separate. CI and
  human review must assess the exact published head; local proof is not approval.
- Native read batching and concurrent independent native observations are
  implemented. No protocol-validity override, per-case timeout or
  financial/finality assertion was relaxed; cadence choice is explicit above.
- Provider drift, frozen metadata, recovered MPC provenance and both pending
  pause-ABA regressions passed. Repeating `set_manual_trip(true)` on an already
  tripped feed changes no state and emits no event; that invalid row was replaced
  with the real new-manual incident after partial resume. Unnecessary validation
  added for the no-op row was removed; original write guards remain intact.
- The two stale fixtures now use 600-second freshness and 2,000 explicit blocks;
  both passed. Earlier 20,000-block advances failed at transport, not proven node
  crashes. Production freshness, protocol validity and per-case timeouts are unchanged.
- Earlier PR2 CI at `f1346cd7` passed: 6,062 fast and 344 NEAR tests,
  including the formerly failing configuration fixture and unchanged
  `many_users_same_snapshot`. One configuration case was marked LEAK but counted
  passed in the successful summary; this does not explain prior cancellations.
- Earlier PR3 CI attempt 2 at `fc049c59` passed: 6,076 fast and
  all 349 NEAR tests, with Full Required Gate successful. One configuration case
  was marked LEAK but counted passed; its cause remains unestablished. The first
  attempt's 45-minute cancellation remains an incomplete run, not a proven
  product defect or infrastructure attribution.
- The separate [manual Test run at `c481ccc5`](https://github.com/Templar-Protocol/contracts/actions/runs/42858146508)
  failed: 101 of 439 NEAR cases passed, eight `asset_migration` cases failed, and
  330 were not run after fail-fast. Those eight stopped in `03-patch` on payload
  expiry or unknown transaction reconciliation (including HTTP 408), before
  their intended conversion/replay assertions. All 11 converter cases passed.
  This is neither a completed reusable regression gate nor proof of a purely
  infrastructure failure. The one-off suites are local opt-in at the user's
  direction; this historical CI failure is not presented as green.
- Frozen-parent CI snapshots were green; they do not validate the cleanup commits.
  Required human/security review remains separate; PR1's checklist is still open.
  PR observations are bounded snapshots, not a permanent background service or
  merge readiness.
- The NEAR CI job limit is restored to 45 minutes. One-off script/converter
  acceptance requires the explicit `asset-migration-acceptance` feature and is
  excluded from normal test targets; reusable CLI/gateway regressions remain.
  Stock cadence and Nextest's per-case termination are unchanged. Workflow
  YAML/configuration parsing passed; actionlint is unavailable.
- The RPC compatibility correction is pinned through the workspace-root Cargo
  patch. Registry consumers do not inherit it automatically; external Rust
  applications need the same root patch until an upstream release includes it.
  No release or published-SDK compatibility claim is made.
- Structured MT normalization still joins and re-splits field strings: a colon in
  a structured `contract_id` can shift into `token_id`, and non-string fields are
  coerced. STATE-001 was excluded because correcting acceptance/error behavior is
  a separate correctness change, not a behavior-preserving simplification.

Oracle pause is not maintenance mode. Observed raw-state/inventory checkpoints do
not prevent a later user action or old callback from racing a submitted receipt.
A failed refresh after unpause must remain visible as unhealthy/open; historical
completion is not a perpetual freshness certificate.
