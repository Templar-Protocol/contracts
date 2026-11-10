# HOT-to-Omni tooling verification

## Claim scope

Controlled local NEAR execution only. The production converter ABI, real
settlement semantics and safe intermediate-state admission are unavailable;
production submissions remain disabled. No production deployment, conversion,
release or PR merge was performed. Deterministic genuine Ed25519 signatures do
not establish distributed-MPC availability.

Controlled-local acceptance is complete on `feat/hot-omni-4-orchestration`.
[Published-head CI](https://github.com/Templar-Protocol/contracts/actions?query=branch%3Afeat%2Fhot-omni-4-orchestration)
and human review are separate from this local evidence; no merge approval is claimed.

The approved full-matrix environment is the repository's CI-standard
`NEAR_SANDBOX_BLOCK_MS=120`, with serialized node cases. Actual chain validity and
expiry checks are unchanged. Default-40-ms raw-workflow liveness is not claimed.

## Exercised evidence

| Surface | Command / scenario | Observed result |
|---|---|---|
| Gateway DAO and freshness | `just test-sandbox --stale -p templar-gateway-core --test dao --test proxy_source_readiness --test-threads 1` | Four real-neard cases passed: native policy/bond/threshold behavior, role semantics, budgeted DAO→governance→target execution, and all-source readiness. |
| Live transaction validity | Throwaway real-owned-sandbox helper plus actual compiled `tmplrmgr read chain.getTransactionValidityPeriod --json null` | Typed SDK and CLI both returned the actual 100-block window. Helper removed. No serialized `ProtocolLimits`/PatchPlan schema change. |
| Gateway and sandbox fast/catalog | `just test-fast -p templar-sandbox -p templar-gateway-testing -p templar-gateway-core --lib`; earlier spec/dispatch fast gate; catalog normal gate | Current 60 shared tests passed; earlier 101 spec/dispatch and four normal catalog tests passed. |
| Native ordered read batching | Actual compiled CLI with a heterogeneous two-read batch against real owned neard, plus write-containing rejection without partial stdout/account mutation | Passed and matched typed SDK values. All 610 manager fast tests on that isolated parent baseline passed. Root integration and final current-root fast gate remain separate. |
| Committed sandbox patches | Startup plus 16 single-submission account/code/raw-key create/reset cycles on nearcore 2.12.0 | Passed. Exact `near-sandbox = 0.3.11` selects the upstream committed-patch repair; finalized-state fences remain enabled. |
| Configuration patch | `just test-sandbox --stale -p templar-manager requires_sandbox_config_patch --test-threads 1` in PR2 | Five tests passed. The receipt-time guard failure runs under local, global-hash and global-account restoration; raw state/configuration/effective code/linkage roll back. Native gas balance and nonce are not rollback invariants. |
| Manager fast and parent node gate | `just test-fast -p templar-manager`; `just test-sandbox --stale -p templar-manager --test-threads 1` in PR3 `fc049c59` | 628 fast tests and all 19 parent sandbox cases passed after finalized-fixture propagation (496.571 s node gate). Both real-Sputnik stamped-patch transports, live drift, actual-period expiry and atomic rollback passed. This is not final current-root evidence. |
| Current root manager fast gate | `just test-fast -p templar-manager` | All 639 passed with the pinned RPC correction and metadata/finality repairs. Full-node acceptance remains separate. |
| Converter | `just test-sandbox --stale -p templar-manager --test asset_converter --test-threads 1` | All 11 passed: four FT/MT pairings, exact raw-unit settlement, failure refunds, real DepositBack credit/storage divergence, invalid routes and unauthorized/zero-input behavior. |
| Complete script workflow | Full manager sandbox gate below, through actual compiled CLI, shell and Python | All five complete/restart variants passed: borrow FT, collateral MT, mixed inventories, shared-feed cohort and original pause, including Sputnik approvals/timelock, MPC, reconciliation and accepted-cache refresh. |
| Current root compilation/lint | `cargo check --workspace`; manager/converter/artifact/testing/sandbox/core/spec/types all-target Clippy `-D warnings`; formatting | Passed, including the latest manager fixture corrections. Existing workspace unknown `clippy::ignore_without_reason` warnings remain; no change-specific Clippy finding remains. |
| Artifact catalog | `just test-fast -p templar-contract-artifacts --features fetch,workspace-loader,clap --run-ignored all`; `./script/check-artifact-drift.sh` | All 63 and the drift entrypoint passed. The entrypoint runs Rust checks and is not build-free. Immutable historical release files were not hand-edited. |
| Executable entrypoints | All eight directly executable stage `--help` commands; actual runner and native read-batch/predict-manual-trip/MPC-show help; ten Python syntax parses and Bash syntax | Passed; not a substitute for chain-runtime acceptance. |
| Offline ingress and journal boundaries | Current Python implementation: complete-intent publication/reopen; missing/unknown intent fields; domain key ending `:mpc` versus explicit financial scope; authored→normalized→frozen campaign parsing | Passed. Boolean schema, unknown campaign fields/transport, zero wait and raw explicit validity refused; frozen null validity retained. Empty artifact set: no chain, snapshot-resolution or campaign-acceptance claim. |
| Predictor reconstruction regression | Current scripts against preserved pre-fix CLI, then current CLI on the same mixed workflow scenario | Before: `02-pause` refused with `complete state accounting mismatch: account reports 1055321 bytes, fetched records account for 528900`. After: the full scenario passed. Shared staging preserves source local/global identity; temporary key removal and original metadata restoration retain strict complete-state accounting. |
| Four safety regressions before repair | Preserved original indexed Python scripts with actual current CLI and neard | Dependency metadata drift prepared a swap instead of refusing; a foreign pause ABA was accepted; a nonexecuting vote recovered a completed conversion; effective provider-code drift reached an approval wait. Each failed the required refusal assertion. No unrelated fixture failure is counted as this proof. |
| Canonical delegated-global RPC decoder | Fork `f96c0441`: two consumer regressions, generator equivalence, captured successful response and actual originally-paused workflow | Before: both current RPC identifier spellings failed. After: both passed with canonical action JSON unchanged; the full real response decoded and the delegated-global workflow passed in 95.443 s. Upstream HEAD and patch share the same 148 generated-code Clippy findings and unchanged formatting failures. |
| Historical receipt retention | Transparent read-only lookup trace, then the same compiled cohort workflow with sufficient node history | Before: the original successful `03-patch` relay became `UnknownTransaction`, and an earlier block became `UnknownBlock` while the current head remained readable. After the dedicated fixture's flat `gc_num_epochs_to_keep=1000` setting: complete/restart cohort passed in 330.164 s. Normal node defaults and code-30 refusal are unchanged; diagnostic hook removed. |
| Real new manual incident | `requires_sandbox_scripts_new_manual_pause_after_partial_resume_blocks_cohort` | Passed in 176.006 s. An authorized owner changes the first resumed feed from false to true without consuming the pending campaign governance ID; restart rejects without new proposals/writes or altering the incident. |
| Full manager sandbox matrix | `NEAR_SANDBOX_BLOCK_MS=120 just test-sandbox --stale -p templar-manager --test-threads 1` | **109/109 passed**, 25 slow, 643 fast/other cases excluded by the sandbox partition; 9511.335 s. Complete current uninjected gate, including 79 script, 11 converter and 19 original manager node cases. |
| Current catalog and documentation | `just test-fast -p templar-gateway-catalog`; `just docs-check`; selected repository-relative Markdown targets | Four catalog tests, risk-document self-test and generated-document check passed; relative targets exist. External URLs, fragments and site-root routes were not link-checked. |

Current converter test-WASM SHA-256:
`d96bb1c056009e19b476a1e2ad6317651a2ad4d6ea7fe396ad9ac7f55cad4b2e`.

Pinned real Sputnik revision:
`5434a59dec23ee0c49df8c8c4350c208a7e75b0b`; WASM SHA-256:
`389494bc38a43140edc2717f364c8bb8bcf05a9dcc5ace3dfdb956d574a6bbb3`.

## Review and operational limits

- Local acceptance is complete. CI and review must be assessed against the
  exact published head; local proof is not merge approval.
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
- Current-head PR2 CI at `f1346cd7` passed: 6,062 fast and 344 NEAR tests,
  including the formerly failing configuration fixture and unchanged
  `many_users_same_snapshot`. One configuration case was marked LEAK but counted
  passed in the successful summary; this does not explain prior cancellations.
- PR3 current-head CI attempt 2 at unchanged `fc049c59` passed: 6,076 fast and
  all 349 NEAR tests, with Full Required Gate successful. One configuration case
  was marked LEAK but counted passed; its cause remains unestablished. The first
  attempt's 45-minute cancellation remains an incomplete run, not a proven
  product defect or infrastructure attribution.
- Observed CI for parent PRs #713–#715 is green. Required human/security
  review remains separate; PR1's checklist is still open. PR observations are
  bounded snapshots, not a permanent background service or merge readiness.
- PR4's expanded CI job ceiling is 120 minutes for cold builds plus 79 script
  and 11 converter cases. Stock cadence and Nextest's per-case termination are
  unchanged. Workflow YAML/configuration parsing passed; actionlint is unavailable.
- The RPC compatibility correction is pinned through the workspace-root Cargo
  patch. Registry consumers do not inherit it automatically; external Rust
  applications need the same root patch until an upstream release includes it.
  No release or published-SDK compatibility claim is made.

Oracle pause is not maintenance mode. Observed raw-state/inventory checkpoints do
not prevent a later user action or old callback from racing a submitted receipt.
A failed refresh after unpause must remain visible as unhealthy/open; historical
completion is not a perpetual freshness certificate.
