# Proxy-oracle audit remediation ledger

Baseline: `a3c8a9653a6a460731881e327253a811aa2c07e9` (clean). Cached audit targets `fde891da949ccdc615198175847de72cfdaef828`; engagement `f3bc6599-6681-4e93-902b-813bd0e2c93d`. HAL numbers use **halIndex**, not findingIndex. Original report status is evidence metadata, not proof of a current fix. No live transactions or upgrades are authorized.

New Soroban aggregation/pin XDR is **fresh-stack only**: incompatible runtime/governance in-place migration is skipped. Existing live identities were not established. NEAR upgrades require exact deployed-artifact/state/pending-operation inventory; deleted allocator history cannot be reconstructed.

## HAL-1: Single Role Holder Can Fill Pending Proposal Queue

- Original UUID: `9d21d325-84c0-4ff5-bc7d-8fea19323dc2`. Audit severity: **low**. Report status: **Pending**.
- Chain: Both. Precondition: Delegated role holder fills shared queue.
- Current assessment: Fixed; delegated availability attack no longer consumes Admin's reserved slot. Scheduling fairness is not guaranteed.
- Disposition: Reserve slot 64 for Admin; retain cap and expected IDs.
- Audit source locations: `contract/proxy-oracle/governance-kernel/src/lib.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`.
- Scenario/evidence: Both governance entrypoints reject delegate admission at 63 pending and accept Admin slot 64. NEAR governance regressions, Soroban `reserved_admin_slot_and_original_expected_ids_are_never_reused`, and real host queue/revocation smoke passed. Admission lives in each governance `src/lib.rs`; shared creation IDs remain unchanged.

## HAL-2: Unpinned Source Decimals Allow Silent Price Rescaling

- Original UUID: `3071300a-fabf-4dc9-bf55-337bc20fffc1`. Audit severity: **low**. Report status: **Pending**.
- Chain: Soroban. Precondition: Admitted upstream changes decimals after admission.
- Current assessment: Fixed for admitted-source decimal drift; it still presupposes upstream upgrade/misconfiguration rather than an unauthenticated local write.
- Disposition: Pin expected decimals at admission and refresh; requires source upgrade/misconfiguration.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/conversion.rs`; `contract/proxy-oracle/soroban/contract/src/refresh.rs`.
- Scenario/evidence: `soroban/common/src/lib.rs::parse_proxy_config` validates explicit pins; runtime admission queries decimals and refresh filters drift. Decimal 8→9 invalidates quorum; explicit repin resets aggregation and retains next_id. Common/runtime/integration gates and actual host smoke passed.

## HAL-3: History Keeps Serving Prices After A Halt

- Original UUID: `f185765f-2667-4e6b-9a8a-f2e071c92d82`. Audit severity: **low**. Report status: **Solved**.
- Chain: Soroban. Precondition: Manual or automatic halt with historical samples.
- Current assessment: Existing fix retained and verified.
- Disposition: Historical reads consult breaker state; history remains freshness-independent.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/lib.rs`; `contract/proxy-oracle/soroban/sep40-adapter-contract/src/lib.rs`; `contract/proxy-oracle/soroban/contract/src/refresh.rs`.
- Scenario/evidence: Runtime and adapter historical reads fail closed under manual/automatic halt, while ordinary history remains independent of cache freshness. Runtime/adapter/history integration regressions passed; no ring-buffer or freshness policy change was introduced.

## HAL-4: Circuit Breaker Ring Buffer Accepts Duplicate Samples

- Original UUID: `5995c3ab-1135-4396-be70-261809879bba`. Audit severity: **informational**. Report status: **Solved**.
- Chain: Both. Precondition: Duplicate/older publication with zero sampling interval.
- Current assessment: Existing fix retained and verified at zero sampling interval.
- Disposition: Persist only increasing source publication times; generic ring buffer unchanged.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/ring_buffer.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: Kernel `equal_publish_time_trips_without_advancing_history` and `regressing_price_trips_without_advancing_history`, plus runtime `duplicate_and_older_samples_do_not_pad_zero_interval_histories_while_blocked`, passed. Generic RingBuffer behavior is unchanged.

## HAL-5: Empty Breaker History Skips First Price Check

- Original UUID: `5387c56f-c683-4b21-83dd-436515226088`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Relative rule has no trusted first sample.
- Current assessment: Intentional bootstrap limitation.
- Disposition: Preserve initial baseline semantics and explicit cumulative baseline.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/stepwise_change.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`.
- Scenario/evidence: Kernel first-baseline behavior remains unchanged: relative rules require a reference; cumulative rules retain their explicit installation baseline. Production-config/baseline recovery tests and fresh host first-refresh smoke passed. This is an acknowledged bootstrap trust requirement, not a new fail-closed policy.

## HAL-6: Admin Grant Accepts An Address That Cannot Sign

- Original UUID: `892faed0-d266-4647-a238-e30e9c81999d`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Admin grants unusable account then revokes usable Admin.
- Current assessment: Fixed; Admin activation now proves grantee authorization without restricting valid contract-account authentication.
- Disposition: Require grantee co-authorization; reject governance-self Admin; retain last-Admin guard.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/roles.rs`; `contract/proxy-oracle/soroban/governance-contract/src/engine.rs`.
- Scenario/evidence: `soroban/governance-contract/src/roles.rs` requires new-Admin coauthorization and rejects self-Admin; explicit auth-tree tests reject missing grantee/bootstrap auth and retain last-Admin protection. Constructor failure also leaves Header/ProxyOracle absent. Governance and fresh-stack integration gates passed.

## HAL-7: Role Mapping Lets Lower Roles Veto Admin Proposals

- Original UUID: `2608f406-c729-447f-b73f-7da5caf2caf1`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Peer with same operation role cancels another creator proposal.
- Current assessment: Fixed; peer execution remains intentionally separate from cancellation authority.
- Disposition: Creator-or-Admin cancellation; current authorized peer may still execute.
- Audit source locations: `contract/proxy-oracle/soroban/governance-common/src/lib.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`.
- Scenario/evidence: Both chains require creator or current Admin to cancel. Regressions cover unauthorized same-role peers, authorized peer execution, creator cancellation after role loss, and Admin override; Soroban uses explicit caller auth. NEAR/Soroban governance gates passed.

## HAL-8: Circuit Breaker Rules Ignore Sample Time Gaps

- Original UUID: `c4d8b7b2-9170-4810-a8af-6c1c08eddc60`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Keeper samples have long time gaps.
- Current assessment: Intentional sample-count semantics.
- Disposition: Document cadence/recalibration; do not change mathematics.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/observation.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/stepwise_change.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/windowed_change_delta.rs`.
- Scenario/evidence: `kernel/.../rule/{stepwise_change,windowed_change_delta,monotonic_run}.rs` retains observation-count mathematics; sampling only controls persistence. Window-mean/monotonic/sampling tests passed, with existing window/monotonic Kani assertions and reachability proved. Keeper cadence remains an operator calibration assumption.

## HAL-9: Opposite-Sign Window Changes Sum Instead Of Subtract

- Original UUID: `43c53da8-628e-4aa6-a3c0-51fb5a7f58c9`. Audit severity: **informational**. Report status: **Solved**.
- Chain: Both. Precondition: Opposite-direction windows.
- Current assessment: Existing arithmetic fix retained and verified.
- Disposition: Current window means/sums replace historical sign error.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/windowed_change_delta.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/math.rs`.
- Scenario/evidence: Kernel `windowed_change_delta_compares_window_means`, strict window-sum and overflow cases passed. Both windowed Kani harnesses proved their assertions and all covers. No signed-sum formula was reintroduced.

## HAL-10: Ownership Transfer Always Uses Maximum Acceptance Window

- Original UUID: `9592a6f2-161e-489b-a7ca-39dcd6381bd0`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Executed ownership transfer remains pending.
- Current assessment: Fixed by cancellability; maximum acceptance-window bootstrap remains intentional.
- Disposition: Append cancellation action sharing Admin authorization and transfer TTL.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/engine.rs`.
- Scenario/evidence: Appended `CancelOwnershipTransfer` in Soroban governance-common; engine dispatches existing transfer cancellation. Matching/wrong/missing/expired stored pending-owner tests passed, with explicit auth, the existing transfer TTL, and no new OperationKind/TtlConfig field or transfer-start event.

## HAL-11: Arbitrary Tie-Break Picks Aggregate Timestamp And Exponent

- Original UUID: `6ae3db6a-c3a8-4465-a172-8eb5f53dac25`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Numerically equal endpoints have different metadata.
- Current assessment: Fixed; metadata selection is deterministic without changing numeric price equality.
- Disposition: Median-local total order; preserve numeric SpecificPrice equality and exact exponents.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/aggregator/method/median/specific_price.rs`; `contract/proxy-oracle/kernel/src/proxy/aggregator/method/median/mod.rs`.
- Scenario/evidence: Failing-before reversed-source timestamp 110/100 regression now selects Low 100/High 110 in either order. `equal_price_metadata_is_permutation_independent` and scaled exponent/weight regressions passed; median comparator orders numeric value, timestamp, exponent, raw value, weight. SpecificPrice comparison remains numeric.

## HAL-12: Constructor Allows Zero Timelock, Unverified Proxy Oracle

- Original UUID: `24f405bf-20e3-4727-ae08-ec42ef45e26c`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Zero bootstrap timelocks/nonreciprocal constructor.
- Current assessment: Intentional bootstrap facility.
- Disposition: Verify reciprocal wiring and hardened timelocks as deployment postconditions.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`.
- Scenario/evidence: Fresh-stack integration proves governance/runtime reciprocal wiring, two-step handoff, explicit new-Admin auth, adapter binding and all 14 TTLs hardened to 24h with SetActionTtl last. Script gate and interrupted-hardening model smoke passed; constructors still permit deliberate zero-TTL bootstrap, not production approval.

## HAL-13: Rearmed Circuit Breakers Skip Trip Checks During Cool-Down

- Original UUID: `d1dc13cd-d7e5-443f-a4ca-270b1b78e3a9`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Privileged rearm enters cooldown.
- Current assessment: Intentional recovery authority.
- Disposition: Preserve ArmedAfter and histories; no fail-closed cooldown.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: Kernel `future_armed_breaker_records_history_without_tripping` and `hal_20_rearm_preserves_both_shared_histories` passed. Relative rearm arithmetic and both runtime overflow regressions passed. Privileged cooldown bypass is retained and documented, not silently converted to blocking.

## HAL-14: Proxy Reconfiguration Leaves Stale History And Breakers

- Original UUID: `91a93fd5-087d-4365-8faf-168bfc0e5690`. Audit severity: **informational**. Report status: **Partially Solved**.
- Chain: Both. Precondition: Aggregator/source/quorum replacement or destructive removal.
- Current assessment: Fixed; full reset clears configuration/rules/trip/history/cache while retaining allocator identity.
- Disposition: Retain cache/history clearing and only preserve next breaker ID on full reset.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/lib.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`.
- Scenario/evidence: `reset_preserving_next_id` is used for destructive removal and Soroban aggregator replacement. Method/order/pin/quorum changes reset full state; freshness-only changes preserve breakers; identical config is a no-op. Both-chain lifecycle and Soroban replacement regressions passed.

## HAL-15: Governance Keeper Never Refreshes Role Storage TTLs

- Original UUID: `1d75c83b-c8be-407d-8eb7-eeff7476b396`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Dormant role count/index/membership/ExistingRoles archive.
- Current assessment: Fixed with bounded maintenance; live-network restoration remains an operator responsibility.
- Disposition: Bounded role TTL pages plus normal keeper ExistingRoles renewal; no membership cap.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/roles.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`.
- Scenario/evidence: `roles.rs::extend_page` renews count/index/membership for all four roles; ordinary keeper renews ExistingRoles/groups. Multi-page near-archival tests verify TTLs/default resource limits, count 0/17 rejection and past-end None. Archived role/group host-restoration regression passed; this is SDK recording-mode evidence, not network restoration proof.

## HAL-16: Stale Breaker Id Silently Voids NEAR Proposal

- Original UUID: `5d4a4f15-0fb7-479b-86c7-c5b0970c0806`. Audit severity: **informational**. Report status: **Pending**.
- Chain: NEAR. Precondition: Two adds captured same expected breaker ID.
- Current assessment: Guard retained; operator-facing target-failure ambiguity fixed.
- Disposition: Preserve one-shot dispatch and explicit IDs; reject failed target receipts in manager.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/error.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`.
- Scenario/evidence: Competing guarded adds install exactly one rule. `context::check_governed_execution_status` is shared by explicit and combined execution, requiring terminal outcome/no failed receipts. Actual tmplrmgr sandbox executions exited 1 with failed oracle receipt and transaction evidence for proposals 5/7; consumed proposals were not requeued.

## HAL-17: Breaker Added Event Drops Configured Parameters

- Original UUID: `3c1199a0-0376-4f29-a6c2-9d8564b774b2`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Breaker added event omits parameters.
- Current assessment: Fixed through an additive companion event.
- Disposition: Keep old event; add typed config/baseline detail from existing DTO.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/events.rs`; `contract/proxy-oracle/soroban/contract/src/codes.rs`.
- Scenario/evidence: `CircuitBreakerAddedDetail` projects existing CircuitBreakerConfig from the installed kernel rule; baseline exists iff cumulative. All rule parameters/baseline and old Added payload regressions passed; compiled ABI gate pins companion fields/topics. Existing event remains unchanged.

## HAL-18: Constructor Accepts Owner With No Reciprocity Check

- Original UUID: `1fc1450f-8b20-4d5a-8346-0dc6628abcf1`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Bootstrap owner not governance; adapter base differs.
- Current assessment: Existing base guard retained; circular constructor restrictions rejected in favor of verified deployment postconditions.
- Disposition: Test ownership/wiring postconditions; avoid circular constructors.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/lib.rs`; `contract/proxy-oracle/soroban/sep40-adapter-contract/src/lib.rs`.
- Scenario/evidence: Fresh host Bootstrap and integration assertions verify intended runtime, governance ownership after ordinary handoff, authenticated Admin, adapter parent/asset/base and runtime source_base. Adapter base-mismatch rejection passed. Script verifies the same bindings; no live stack identity or cutover is claimed.

## HAL-19: Matured Governance Proposals Remain Executable Indefinitely

- Original UUID: `b444f04e-eb41-4e7b-86d9-79e51be7157c`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Matured proposal persists indefinitely.
- Current assessment: Intentional minimum delay.
- Disposition: No expiry/grace period; current authorization and explicit cancellation remain.
- Audit source locations: `contract/proxy-oracle/governance-kernel/src/lib.rs`.
- Scenario/evidence: Governance kernel retains minimum-delay semantics and current execution authorization; create/execute TTL tests passed. Cancellation/revocation regressions provide explicit removal paths. No expiry/grace period was added; indefinite maturity remains a deliberate policy.

## HAL-20: Aggregation Error Codes Collapse To One Value

- Original UUID: `44d398e9-bf01-4b10-8f39-b8119f0df191`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Aggregation errors share code.
- Current assessment: Fixed; non-aggregation public codes remain unchanged.
- Disposition: Keep unrelated codes; append 101/102 with expected/actual detail.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/aggregator/method/mod.rs`; `contract/proxy-oracle/soroban/contract/src/codes.rs`.
- Scenario/evidence: `soroban/contract/src/{codes,refresh}.rs` projects intact errors to 101/102 and checked expected/actual counts. Length mismatch boundary, insufficient quorum, generic-no-detail tests and real host code-102 companion smoke passed. ABI gate verifies the additive event schema.

## HAL-21: Blocked Price Event Omits Which Breaker Tripped

- Original UUID: `8e7760b8-232e-48ce-830b-95ad21e15fa6`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Already blocked refresh including no-source path.
- Current assessment: Fixed for all blocked refresh paths.
- Disposition: Publish exact blocking IDs on every blocked refresh; manual IDs empty.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/events.rs`; `contract/proxy-oracle/soroban/contract/src/codes.rs`; `contract/proxy-oracle/soroban/contract/src/refresh.rs`.
- Scenario/evidence: Typed Blocked(PriceBlockedReason) reaches one projection match. `blocked_refresh_details_retain_all_ids_with_sources_without_sources_and_manual_trip` passed: automatic IDs remain complete, manual IDs empty, old CacheBlocked payload unchanged. Host blocked-event smoke and ABI policy passed.

## HAL-22: Breaker History Resize Skips Clear Or Rearm

- Original UUID: `0cf73c69-56df-47bd-becc-6615214bebe5`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: History capacity/cadence changed with installed rules.
- Current assessment: Existing atomic rejection/history-preservation behavior retained and verified.
- Disposition: Reject insufficient capacity atomically and retain valid history.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/ring_buffer.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/windowed_change_delta.rs`.
- Scenario/evidence: Kernel `set_config_resizes_history_in_place` and Soroban history resize/cadence regression passed: invalid capacity/sampling rejects without mutation; valid resizing retains appropriate observations and installed breaker state. No unconditional clear/rearm was introduced.

## HAL-23: Governance Create Requires Predicting Shared Proposal Id

- Original UUID: `71b13c1e-240c-40d0-9ad0-4e922d9bc88e`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Contending or replayed explicit proposal ID.
- Current assessment: Intentional at-most-once guard; contention remains.
- Disposition: Retain expected IDs; unknown outcome stops, never reselect or claim ownership.
- Audit source locations: `contract/proxy-oracle/governance-kernel/src/lib.rs`.
- Scenario/evidence: Kernel replay/contention cases passed; actual CLI sandbox covered discarded-success response, contention and replay after cancel/execute without ID advancement or body adoption. Script unknown-send gates and exact-ID hardening evidence tests passed. Contention remains intentional; transport-level loss was not injected.

## HAL-24: Price Decline Measurement Caps Change At 1.0

- Original UUID: `f430a208-0be7-4b13-a6f4-e3efca776cf7`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Decline threshold at or above 1.
- Current assessment: Intentional conventional relative math.
- Disposition: Validate 0 < threshold <= 1; exact 1 allowed with strict comparison.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/math.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/stepwise_change.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/monotonic_run.rs`; `contract/proxy-oracle/soroban/contract/src/conversion.rs`.
- Scenario/evidence: `is_valid_relative_change` preserves 0 < x <= 1. Exact threshold-1 cases passed: full/positive declines and growth exactly 100% do not exceed strict 1; growth above 100% does. Existing relative-comparison Kani proof passed. No symmetric/log-return substitute was introduced.

## HAL-25: Sep40Adapter Returns Price History Oldest First

- Original UUID: `093e6a69-09a5-49a7-972d-a385f8e8fcb7`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: SEP-40 historical bucket order/query alignment.
- Current assessment: Fixed; compatible adapter storage is separately exercised through compiled-Wasm upgrade.
- Disposition: Newest-first distinct buckets; bucket query time and choose newest convertible entry.
- Audit source locations: `contract/proxy-oracle/soroban/sep40-adapter-contract/src/lib.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: Adapter tests yield distinct newest buckets [110,100] from [100,101,110], bucket query 109→100 and skip unconvertible candidates. Actual released 0.1.4→current Wasm swap passed with preserved Config/owner/decommission and corrected history. Parent history stays chronological/breaker-gated.

## HAL-26: Remove Proxy Skips Existence Check And Emits Event

- Original UUID: `4ba68345-7b17-4cab-bc2b-f3520b9ff022`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Remove unknown asset.
- Current assessment: Fixed; unknown removal fails before mutation/events.
- Disposition: Existence guard before all mutation/events.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: `soroban/contract/src/lib.rs::remove_proxy` requires an existing proxy before resetting/deleting/emitting. Unknown-asset regression passed with no success event/state mutation; removed-proxy views hide retained allocator tombstones.

## HAL-27: Runtime Failure Mapping In Governance Call Is Dead Code

- Original UUID: `c5aa2669-eca2-4b7d-b349-ba1eb6c7ebc8`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Governed runtime invocation returns failure.
- Current assessment: Fixed with typed nested invocation outcome handling.
- Disposition: try_invoke_contract; RuntimeFailed with whole-transaction rollback/proposal retention.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/engine.rs`.
- Scenario/evidence: Engine uses try_invoke_contract and accepts only Ok(Ok). `runtime_error_rolls_back_effects_and_retains_proposal_through_real_invocation` passed: returned runtime error maps to RuntimeFailed, child effects roll back and body/header remain unchanged.

## HAL-28: Queue-Full Governance Error Aliases As Invalid Input

- Original UUID: `3c3238a9-14a2-436f-91fe-d30a5787bc8e`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Pending queue full.
- Current assessment: Fixed additively; existing error numbers retained.
- Disposition: Append TooManyPendingProposals=12; retain all existing codes.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`; `contract/proxy-oracle/soroban/governance-common/src/lib.rs`.
- Scenario/evidence: `TooManyPendingProposals = 12` is appended in governance-common and used for delegate reserve/global cap. Full-queue/error regressions and real host reserve smoke passed; compiled governance ABI policy passed without renumbering old errors.

## HAL-29: Proxy Config Lacks Age And Source Bounds

- Original UUID: `db38f61d-0b93-4cdd-a64d-831aabc6d4ec`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Missing freshness/source bounds or duplicate wrappers.
- Current assessment: Existing bounds strengthened through the shared parser; provider independence remains outside enforceable address identity.
- Disposition: Retain fail-closed and zero-age policy; no invented identity/stale fallback.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/freshness_filter.rs`; `contract/proxy-oracle/soroban/contract/src/conversion.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`; `contract/proxy-oracle/soroban/contract/src/refresh.rs`.
- Scenario/evidence: Common parser tests passed for 1–16 distinct oracle addresses, quorum, pins≤18, required age/drift bounds and zero-age policy. Runtime freshness/pin/fallback tests and integration smoke fail closed. Wrapper addresses/quorum are not represented as proof of economic independence.

## HAL-30: NEAR Proxy Oracle Lacks Aggregator Config Validation

- Original UUID: `eec1926b-3050-4162-ab28-818562fc739a`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Authorized config has duplicate descriptors/impossible quorum.
- Current assessment: Fixed at shared admission/preflight boundaries; historical decoding remains compatible.
- Disposition: Shared ingress shape validator; Priority first-valid fallback, no quorum.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/aggregator/mod.rs`; `contract/proxy-oracle/kernel/src/proxy/aggregator/method/median/mod.rs`; `contract/proxy-oracle/kernel/src/proxy/mod.rs`.
- Scenario/evidence: `Aggregator::validate` rejects empty/duplicate descriptors and median zero weights/impossible quorum; Priority remains first-surviving fallback. Five NEAR invalid-config no-mutation cases passed. Manager reports invalid configurations under unchanged upgrade.source_weights; actual CLI preflight passed.

## HAL-31: Unbounded Sample Interval Freezes Circuit Breaker History

- Original UUID: `68fe64c7-9b06-4df2-9c56-6c9c13604ac6`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Seconds conversion overflows nanoseconds.
- Current assessment: Fixed for overflow; long representable privileged intervals remain intentional.
- Disposition: Checked runtime/static parsing; zero and all representable intervals allowed.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: Shared parse_breaker_set_config and runtime configure_breakers use checked seconds→nanoseconds. Static/runtime overflow regressions passed with no reservation/state/events; zero remains accepted, including MonotonicRun. No arbitrary interval cap was added.

## HAL-32: Unbounded Rearm Timestamp Can Permanently Disable Breaker

- Original UUID: `1fde15dd-2c3a-48bf-b26b-fc9a230f8ba2`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Relative rearm delay overflows timestamp.
- Current assessment: Existing checked relative-delay behavior retained and verified.
- Disposition: Preserve relative delay and reject overflow; long intentional delay remains authorized.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/status.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: Both runtimes reject seconds/deadline overflow atomically. NEAR relative-rearm regression preserves breaker/cache/epoch on u64::MAX; Soroban overflow case passed. Preflight distinguishes compatible relative payloads from old absolute-time payloads. Very long representable recovery delays remain authorized.

## HAL-33: Governance Validation Admits Proposals The Runtime Rejects

- Original UUID: `8cd2a72b-1924-416c-90c6-a98f284a4f61`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Statically impossible breaker proposal queues.
- Current assessment: Fixed for statically decidable shape, representability and feasibility; dynamic execution remains state-dependent.
- Disposition: Share pure shape/representability/feasibility parsing; execution retains dynamic checks.
- Audit source locations: `contract/proxy-oracle/soroban/governance-common/src/lib.rs`; `contract/proxy-oracle/soroban/governance-contract/src/engine.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`; `contract/proxy-oracle/soroban/contract/src/conversion.rs`.
- Scenario/evidence: Governance-common reuses common proxy/breaker parsers and kernel feasibility predicate without fabricated cumulative baseline. Static impossible-history/threshold/interval cases reject before queue reservation; dynamic runtime-failure regression retains proposal and rolls back child state. Common/governance gates passed.

## HAL-34: Role Revocation Leaves Queued Proposals Executable

- Original UUID: `c2bc2295-ef24-46b1-a586-e103eac7b97b`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Revoked creator queued proposals survive.
- Current assessment: Fixed with live policy checks and atomic body-dependent purge.
- Disposition: Purge only now-unauthorized creator operations after successful role removal.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/roles.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`; `contract/proxy-oracle/soroban/governance-contract/src/engine.rs`.
- Scenario/evidence: Both governance engines purge only the revoked creator's now-unauthorized operations after actual removal; retained roles/Admin policy and no-op revocations are covered. Missing/corrupt body node tests roll back membership, header, prior purges and storage. Soroban full/sparse grouped queue default-resource and host smoke passed.

## HAL-35: Proposal Maturity Ignores Later Timelock Increases

- Original UUID: `9859b5f4-75a8-4161-b0f4-94f275ac1ce9`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Policy TTL increases after creation.
- Current assessment: Intentional snapshotted maturity.
- Disposition: Preserve queued maturity; document no retroactive delay.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/events.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`; `contract/proxy-oracle/soroban/governance-contract/src/engine.rs`.
- Scenario/evidence: Kernel stores effective TTL in each Proposal and executes against that snapshot; TTL creation/maturity tests passed. Fresh hardening and interrupted script model rely on SetActionTtl last rather than retroactively changing queued maturity. Policy increases intentionally do not rewrite existing bodies.

## HAL-36: Decoupled Proposal TTL Policies Risk Liveness Friction

- Original UUID: `6adf5dc6-5bfe-4090-8531-9bf24d1367d3`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Proposal body archived while ID persists.
- Current assessment: Retained restoration/body-required semantics, with bounded group maintenance verified.
- Disposition: Maintain body-required cancellation and keeper; never silently drop missing body IDs.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/storage.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`.
- Scenario/evidence: Missing-body cancellation/revocation fails instead of dropping IDs. Ordinary keeper renews bounded proposal groups; archived proposal-group/role recording-mode restoration test executes/cancels retained original IDs without allocator reset. No claim of automatic live-network restoration or irreversible orphaning.

## HAL-37: Reused Breaker Id Matches Wrong Breaker State

- Original UUID: `2d6d1a28-0d6d-4ab5-a272-3c03ddd6ea2c`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Asset removed/recreated resets breaker allocator.
- Current assessment: Fixed for future lifecycles; already-deleted allocator history remains unrecoverable automatically.
- Disposition: Reset bounded tombstone preserving next_id; hide absent-asset tombstones.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/error.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: Both chains retain reset tombstones; replacement IDs advance 0/1→2 and stale remove/rearm/enforce/add cannot change replacement/cache. Exhaustion never wraps; corrupt/missing prior allocator rejects. Soroban archival restoration passed. Actual CLI preflight identifies missing lifecycles without rejecting compatible adds wholesale.

## HAL-38: Unversioned Breaker Status Encoding Risks Silent Misdecode

- Original UUID: `7e05397f-523a-4def-9468-c1e01ec989b7`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Code upgrade changes positional breaker bytes.
- Current assessment: Compatible bytes frozen/verified; incompatible Soroban runtime/governance migration explicitly skipped.
- Disposition: Freeze old bytes/load real fixtures; skip incompatible Soroban runtime/governance migration.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/status.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/soroban/contract/src/storage.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: Literal Borsh and postcard fixtures cover both status variants/full sets through native NEAR and Soroban runtime storage/public views. Historical NEAR code-only/migration/ordering fixtures and current-code keyless redeployment preserve owner/roles/pending bodies/IDs and guarded JSON/Borsh APIs. Actual compiled adapter swap passed; no live fleet coverage or old-config decoder is claimed.

## Baseline source assessment and verification

Locations below are relative to `contract/proxy-oracle/` unless prefixed with
`tools/`. These are baseline observations, not completed remediation claims.

| HAL | Current implementation evidence | Required scenario |
|---|---|---|
| 1, 7, 23, 34 | NEAR `near/governance-contract/src/lib.rs:83-113,170-184,232-242`; Soroban `soroban/governance-contract/src/lib.rs:94-168`, `roles.rs:22-67` | Delegate 63/Admin 64; creator/Admin vs peer cancellation; revoked-creator purge; explicit-ID replay after body deletion |
| 2, 29, 30 | `soroban/common/src/lib.rs:140-190`, `soroban/contract/src/conversion.rs:132-155`, `refresh.rs:152-161`; NEAR `near/contract/src/lib.rs:342-347` checks only zero weights | All methods reject invalid shape; admission and refresh pin mismatch fail atomically |
| 3, 4 | `soroban/contract/src/lib.rs:433-454`, `storage.rs:137-157`; integration `manual_trip.rs:33-56`, `breakers.rs:63-72` | Manual/automatic historical halt; duplicate/older publication with zero sampling |
| 5, 8, 9, 13, 22, 24, 32 | Shared `kernel/src/proxy/circuit_breaker/{set.rs,rule/}`; Soroban cumulative baseline `contract/src/lib.rs:241-273`, checked rearm `:294-303` | Preserve baseline/cooldown/window mathematics; exact threshold and overflow; history rejection/preservation |
| 6 | `soroban/governance-contract/src/lib.rs:37-64`, `roles.rs:26-49` | Explicit initial/grantee auth; self-Admin rejection; last-Admin protection |
| 10 | `soroban/governance-common/src/lib.rs:151-168`, `soroban/governance-contract/src/engine.rs:161-182` | Mature cancellation of matching pending owner, including expired transfer; reject wrong/missing |
| 11 | `kernel/src/proxy/aggregator/method/median/mod.rs:147` uses numeric-only tuple ordering; `specific_price.rs:43-58` defines numeric equality | All permutations, scaled-equal exponents, uneven weights; Low time 100/High time 110 |
| 12, 18 | `soroban/governance-contract/src/lib.rs:47-58`; adapter `sep40-adapter-contract/src/lib.rs:58-83,252-267` | Reciprocal governance/runtime wiring, adapter immutable metadata, ownership handoff and final TTL hardening |
| 14, 26, 37 | `soroban/contract/src/lib.rs:172-205` clears/reset/deletes; `:367-390` hides absent proxy views | Tombstone allocator survives method/pin/quorum replacement and removal; unknown removal has no effects |
| 15, 36 | `soroban/governance-contract/src/lib.rs:204-209`, `storage.rs:69-90` maintain proposal bodies only | Near-archive count/index/membership/ExistingRoles TTL plus all four role pages; missing body is not silently dropped |
| 16 | NEAR governance `near/governance-contract/src/lib.rs:249-264` detaches target dispatch | Actual failed target receipt produces manager error in both execution paths; no requeue/reselection |
| 17, 20, 21 | `soroban/contract/src/events.rs:45-70,173-226`, `codes.rs:28-31`, `refresh.rs:25-112` discard configs/counts/IDs | Compatible old events plus exact typed details; already-blocked/no-source paths included |
| 19, 35 | `governance-kernel/src/lib.rs:32-46`; Soroban `governance-contract/src/lib.rs:102-133` snapshots TTL | Long-lived maturity retained; later TTL increases are not retroactive |
| 25 | `soroban/sep40-adapter-contract/src/lib.rs:172-216` rejects unaligned query and traverses oldest first | Times 100/101/110 at resolution 10 yield 110/100; query 109 picks newest convertible 100 bucket |
| 27 | `soroban/governance-contract/src/engine.rs:223-246` invokes trapping call before error mapping | Actual invocation returns RuntimeFailed and rolls back runtime/header/proposal/events |
| 28 | `soroban/governance-common/src/lib.rs:36-48`; queue-full mapped at governance `lib.rs:211-220` | Append error 12 without changing existing assignments |
| 31, 33 | `soroban/contract/src/lib.rs:207-221` uses saturating seconds; `governance-common/src/lib.rs:243-264` omits static breaker checks | Checked interval/history/rule parsing rejects before proposal reservation or runtime mutation |
| 38 | `soroban/scripts/release_artifacts.py:382-540` freezes old XDR; integration `tests/upgrade.rs:11-23` explicitly lacks successful Wasm swap | Literal old bytes and actual NEAR fixtures; no claim of compatible Soroban runtime/governance swap |

Observed baseline checks:

- `just test-fast -p templar-proxy-oracle-kernel --features serde,borsh`:
  **178 passed, 0 skipped**.
- `just test-fast -p templar-proxy-oracle-governance-kernel --features serde,borsh`:
  **9 passed, 0 skipped**.
- Cache identity check: exactly 38 unique contiguous `halIndex` values, 1–38;
  original UUID is the `id` field. Status counts: 34 Pending, 3 Solved,
  1 Partially Solved. These statuses are preserved above, not trusted as fixes.
- Rust 1.86.0, Just 1.51.0, Nextest 0.9.138, Stellar CLI 25.2.0,
  Soroban toolchain 1.99.0, and Kani 0.68.0 are installed.

### Step 2 — shared aggregation verification

- HAL-11 reproduced before the fix: `equal_price_metadata_is_permutation_independent`
  failed with MedianLow selecting publication time 110 instead of 100 for the
  reversed source order. After the median-local comparator, the complete kernel
  gate passes **182/182**, including permutation, exponent, uneven-weight, existing
  numeric equality, property and numerical-reference tests.
- Runtime smoke (`cargo run -p templar-proxy-oracle-kernel --example
  remediation_aggregation --features serde,borsh`) exercised actual aggregate and
  proxy resolution: Low selected time 100, High selected time 110 for both source
  orders; duplicate Priority admission returned `DuplicateSource`. Temporary
  example removed after proof.
- HAL-30 shared `Aggregator::validate` checks nonempty/distinct descriptors,
  median quorum and positive weights without allocating a set. Historic decoding,
  public fields/constructors and `has_zero_weighted_source` remain unchanged.
- NEAR owner ingress uses that validator before writes. Five regression cases
  (empty, duplicate, zero weight, zero/impossible quorum) passed and preserved the
  existing proxy and breaker history under rejected replacement. Command:
  `just test-sandbox -p templar-proxy-oracle-near-contract --test proxy_oracle
  admin_set_proxy_rejects_invalid_aggregators_without_mutation`.
- Preflight now reports each invalid aggregator and reason under the unchanged
  external key `upgrade.source_weights`; internal predicate is
  `aggregator_config_status`. Common/manager gate: **668 passed, 12 skipped**.
- Initial `--stale` sandbox attempt stopped before testing because mock signer/DAO
  Wasms were missing; ordinary sandbox recipe built the prerequisites and the
  scoped admission gate then passed. This is not yet the full node acceptance gate.
- Existing Kani obligations completed **9/9**, with all reachability covers satisfied; individual results are recorded below.

### Approved amendment — Soroban proposal body grouping

Actual governance host smoke reached delegate slot 63/Admin slot 64 and rejected
the original expected ID after cancellation. Revoking the delegated creator with
61 queued bodies then exceeded the host's transaction limits: **141 footprint
entries > 100**, **68 write entries > 50**. No test limits were increased and the
queue scenario was not reduced.

The user explicitly approved **Group proposal bodies** rather than requiring an
operator queue-drain precondition. The fresh Soroban stack groups unchanged
`Proposal` values into eight fixed persistent slots, up to eight bodies each,
preserving header fields, creation IDs/arguments/results, action/policy shapes
and checkpoint identity. This additional storage change is **fresh-stack only**;
no old decoder, fallback or in-place governance migration is provided.
Worst-case group bytes, sparse full-queue atomic revocation and keeper pages
passed their default host resource-limit assertions.

### Step 3 — governance verification

- HAL-1/7/23/34: NEAR/governance-kernel gate **54 passed**. The node-backed
  governance gate **14 passed** (43 non-node tests excluded), including missing
  and corrupt body rollback of role membership, ledger, prior purges, and storage.
  JSON/Borsh entrypoint, upgrade ordering and keyless upgrade tests passed in that
  scoped gate. NEAR header/body layouts and explicit creation signatures remain.
- HAL-6/7/10/15/27/28/34: Soroban governance gate passed 50 cases, followed by
  the corrected dormant-role TTL case passing separately. Cases exercise explicit
  constructor/grantee/caller authorization, last-Admin protection, matching/wrong/
  missing/expired ownership cancellation under the existing transfer TTL,
  actual runtime invocation failure with rollback, creator/Admin cancellation,
  revocation filtering, sparse full-queue default-resource bounds and group size.
  The initial TTL fixture used zero ledger TTL limits; it now explicitly installs
  realistic ledger limits before aging dormant keys, without raising resources.
- All four role count/index/membership families and ExistingRoles survived keeper
  maintenance near archival across multiple bounded pages. Each page asserts
  default footprint/write limits. Count 0/17 rejects; past-end offsets return None.
- Actual host smoke: delegate admission at expected ID 64 rejected with error 12
  at 63 pending; Admin ID 64 admitted as slot 64; cancelled original ID replay
  rejected; revocation purged the creator's queue, retaining only Admin proposal
  64. All four role pages and ordinary keeper completed. Temporary smoke removed.
- Fresh grouped-storage callers: Soroban integration **70 passed**. Keeper script
  suite **120 passed** before DTO cutover; role paging follows returned offsets
  for all four roles and rejects non-advancing offsets.
- Manager CLI help was exercised; it explains explicit-ID retries and emits
  selected proposal/breaker IDs before submission. Actual target-receipt failure
  smoke subsequently passed as recorded in Step 5.

### Existing Kani obligations after step 2

`cargo kani -p templar-proxy-oracle-kernel` completed: **9 verified, 0 failed**.
No mathematical proof inputs were changed. These are bounded existing obligations,
not a proof of all new runtime/governance behavior:

| Harness | Assertions | Reachability covers |
|---|---|---|
| armed_breaker_trips_once_and_stays_blocking | proved | 6/6 satisfied |
| windowed_lookback_uses_offset_reference | proved | 5/5 satisfied |
| windowed_matches_strict_mean_reference | proved | 6/6 satisfied |
| monotonic_matches_significant_suffix_reference | proved | 8/8 satisfied |
| stepwise_matches_strict_relative_reference | proved | 6/6 satisfied |
| freshness_matches_optional_inclusive_limits | proved | 8/8 satisfied |
| median_high_index_satisfies_weighted_rank | proved | 8/8 satisfied |
| median_low_index_satisfies_weighted_rank | proved | 8/8 satisfied |
| priority_resolve_selects_first_surviving_source | proved | 6/6 satisfied |

### Step 4 — unified Soroban boundary verification

- Common/kernel/static-admission gate: **212 passed**. Unified DTOs retain only
  MedianLow/MedianHigh with equal weights and ordered Priority, explicit pins,
  required freshness, and one shared parser/conversion boundary.
- Complete Soroban contract gate: **235 passed** (common/governance-common 27,
  runtime 71, governance 41, adapter 25, Lazer 63, batcher 8).
  Integration gate: **74 passed**. Script gate: **119 passed**; checkpoint schema
  2 rejects old aggregation checkpoints. Removed two mocked forwarding-only
  configuration tests; retained consumer-visible pin-drift/no-submission coverage.
- Actual `remediation_smoke` host run printed explicit create/execute IDs and
  events for MedianLow 100×10^8, MedianHigh 200×10^8, Priority ordered fallback,
  source freshness/quorum failure, mutable decimal pins and manual blocking.
  This was an in-process generated-client host run, not live Stellar evidence.
- Fresh-stack postcondition regression verifies reciprocal governance/runtime
  ownership, adapter parent/asset/base, explicit new-Admin authorization and
  hardening all 14 operation TTLs to 24 hours, with SetActionTtl hardened last.
- Full aggregation reset intentionally restores empty breaker configuration as
  well as clearing rules/history/cache, retaining only next_id. The deliberate
  repin regression therefore reconfigures breaker history before installing the
  replacement rule; replacement ID remains 2 rather than reusing 0.
- Removed runtime `Proxy<u32>` index/source-order translation and duplicate breaker
  conversion. Refresh now uses one kernel source vector and one aligned price
  vector. Admission checks decimal pins only; existing base filtering remains at
  refresh rather than adding an unrelated admission precondition.
- All 33 immutable released **NEAR** Wasm fixtures were fetched into the verified
  artifact cache. They do not establish current production deployment identities.

### Step 5 — lifecycle and diagnostics verification

- Breaker removal/recreation retains the allocator on both chains. Stale add,
  remove, rearm and enforcement operations cannot mutate a replacement rule;
  absent or corrupt historical allocators fail closed rather than restart at 0.
  Public absent-proxy views hide the retained tombstone.
- Manager/NEAR-common gate: **625 passed, 12 skipped**. NEAR runtime contract
  gate: **40 passed, 3 ignored fixture generators**. The added relative-rearm
  overflow case passed separately, preserving breaker state, cache and epoch.
  Kernel gate: **189 passed**, including strict relative threshold 1 boundaries.
- Final scoped Soroban gate: **197 passed** (runtime 81, governance 41,
  integration 75). It exercises checked aggregation counts, every typed
  diagnostic companion, blocked IDs with and without available sources, manual
  blocking, cumulative baseline disclosure, duplicate/older publication,
  history resize/cadence changes and host restoration of an archived tombstone.
- Adapter gate: **25 passed**. History selects newest convertible prices,
  deduplicates resolution buckets and preserves fail-closed/decommission behavior.
  Compiled-Wasm upgrade compatibility is separately verified in Step 6.
- Script gate: **120 passed**. Compiled Wasm ABI policy accepted the exact three
  new diagnostic event schemas, including field order and topic placement.
  Size gate passed: runtime 99,890/131,072 bytes; governance 46,221/131,072;
  adapter 16,729/32,768; Lazer 25,593/32,768; batcher 7,600/32,768.
- Actual `tmplrmgr` subprocesses against an ephemeral NEAR sandbox exercised
  create/execute/cancel, discarded-create-response retry with the original ID,
  counterparty contention, replay after cancellation/execution and stale breaker
  IDs. Rejected requests did not consume a replacement ID or execute another
  creator's body. This simulated response loss by discarding a successful
  response; it was not transport-level fault injection.
- Explicit execution of stale proposal 5 reported a successful governance dispatch
  **and** the failed oracle receipt, exiting 1 with transaction
  `8sUky3fZyrVaCrDtH686Q6FfzxHEVbpiKUWMMXeLchKF`.
  Combined create/execute proposal 7 likewise exited 1 with transaction
  `CXJfycKyDbhGpXRqQRZjyYCdteTayAJqMuHkNYuq6DVF`. Both hashes belong only to
  that ephemeral sandbox. Final state: next proposal 8, next breaker 1, no active
  proposals. Temporary smoke sources were removed after execution.
- NEAR/manager and affected Soroban all-target Clippy commands exited successfully
  with `-D warnings`; they emitted the workspace's unknown
  `clippy::ignore_without_reason` warning. Historical-baseline comparison is
  recorded with final verification rather than treating that warning as fixed.

### Step 6 — compatibility and complete callers

- NEAR fixtures freeze literal Borsh/postcard status/full-set bytes and load the
  Borsh bytes through the actual runtime map. Soroban loads literal postcard
  bytes through its production storage decoder/public view. Neither fixture is
  represented as a captured production breaker set.
- The historical oracle fixture is specifically
  `proxy-oracle-ixlmustry-ixlmusdc.v1.tmplr.near`, release 0.3.0,
  tag `templar-proxy-oracle-near-contract-v0.3.0`, SHA-256
  `d2e62c4566c98e55121a5aad32e0e5b8cfb911f82aca71dbaeaa83794fed9e8e`,
  with `tests/migration/mainnet_v1_proxy_oracle_ixlmustry_ixlmusdc.borsh`.
  Code-only replacement preserves captured owner/sets/allocator. Governance
  release 0.1.0 migration preserves pending legacy AddBreaker ID 7 and Admin;
  both upgrade orders pass. Current-code keyless redeployment separately
  preserves roles, pending JSON/Borsh bodies and proposal allocator.
- Actual Soroban adapter swap mounts verified release 0.1.4,
  tag `templar-proxy-oracle-soroban-sep40-adapter-contract-v0.1.4`,
  SHA-256 `e84da57949a1120d9deef9ced1d54d5436ec115efd05e214495f30cd262a2995`
  (16,818 bytes), and current optimized Wasm SHA-256
  `1b65cba38c4c815c73646faff85232d82b57ce36525713a5b9e55108af9b5662`
  (16,729 bytes). Executable hashes change, Config XDR/owner/decommission state
  survive, old historical defects reproduce before swap and corrected bucket
  reads pass afterward. This is host execution, not live upgrade approval.
- `test-integration` prepares the optimized adapter and enables the explicit
  `compiled-wasm-tests` feature. CI's Soroban job installs Nextest and runs this
  gate. Native-only fast integration remains 75 tests without Stellar tooling;
  the complete compiled-Wasm integration gate runs 76. No ignored or
  silently-successful missing-artifact fallback was added.
- New read-only `upgrade.pending_breaker_lifecycles` inspects current/legacy
  pending targets against loadable sets and IDs. Compatible adds remain queued;
  missing/undecodable lifecycles, stale expected add IDs and absent mutation
  targets fail. It cannot reconstruct deleted allocator history. Rearm preflight
  now retains compatible relative-delay payloads rather than rejecting every
  generic rearm call as legacy.
- Actual `tmplrmgr proxy-oracle preflight` subprocess smoke passed five checks
  with compatible pending add proposal 2, then exited 1 and named proposal 2
  after its proxy was removed. The body stayed unchanged; no execution, new ID
  or replacement proposal occurred. Temporary source was removed.
- Rehearsal `configure` hardens all 14 TTLs to 24 hours, SetActionTtl last.
  Resume requires matching confirmed create/execute evidence for each nonzero
  TTL. Script tests reject wrong-ID/unconfirmed evidence and repeated writes.
  A throwaway transaction/ledger-model smoke interrupted after six updates with
  pending ID 6, resumed that original ID, created IDs 0..13 once, hardened
  SetActionTtl last and submitted nothing on completed resume. This model
  exercises real rehearsal functions, not live Stellar or transport faults.
- Both read-only final review slices found no patch-introduced actionable
  defects in their assigned governance/NEAR/manager and Soroban/script scopes.
  They did not independently execute gates; the observed results below remain
  the verification evidence.

## Final acceptance evidence

| Command / gate | Observed result |
|---|---|
| `just test-fast -p templar-proxy-oracle-kernel -p templar-proxy-oracle-governance-kernel --features serde,borsh` | 200 passed (189 kernel + 11 governance-kernel) |
| `just test-fast -p templar-proxy-oracle-near-common -p templar-proxy-oracle-near-governance-common -p templar-gateway-core -p templar-gateway-methods-spec -p templar-gateway-methods-dispatch -p templar-manager` | 690 passed in the governance/manager partition plus 187 near-common/gateway; 12 node-backed cases excluded |
| `just test-fast -p templar-gateway-catalog` | 4 passed; ignored regeneration case excluded; METHODS.md current |
| `just test-sandbox --stale -p templar-proxy-oracle-near-contract -p templar-proxy-oracle-near-governance-contract` | 56 passed; 51 non-node/ignored cases excluded; Wasms rebuilt by ordinary sandbox recipe |
| Soroban `test` | 249 passed: common/governance-common 27, runtime 82, governance 44, adapter 25, Lazer 63, batcher 8 |
| Soroban `test-integration` | 76 passed, including actual compiled adapter swap; native-only gate separately 75 passed |
| Soroban `test-scripts` | 122 passed |
| Soroban `budget-check` | Runtime 82/82 and governance 44/44 passed; deterministic host scenarios, not full Stellar RPC resource simulation |
| Soroban `size-check` + compiled ABI policies | Runtime 99,749/131,072; governance 47,753/131,072; adapter 16,704/32,768; Lazer 25,593/32,768; batcher 7,600/32,768 bytes |
| `cargo check --workspace` | Passed |
| Existing `cargo kani -p templar-proxy-oracle-kernel` obligations | 9/9 proved; every listed reachability cover satisfied; inputs unchanged |

Affected NEAR/kernel/manager and Soroban all-target Clippy gates use
`-D warnings`. The unknown `clippy::ignore_without_reason` warning also occurs
with successful Clippy on the original `a3c8a965` kernel in a temporary detached
baseline worktree; that worktree was removed after comparison.

Net simplification is structural, not a claim of fewer total lines: removed
runtime `Proxy<u32>`/source-order translation, the old private proxy/breaker
conversion paths and independent optional error fields. One common parser and
config conversion now serve admission/runtime/governance; event projections use
the existing breaker DTO. Remaining `config.sources/min_sources` references are
the intended MedianConfig conversion in common, not the removed flat config.
Public kernel constructors, existing zero-weight helper and Soroban aliases
remain intentionally compatible.

### Production limits

No live transactions, upgrades, ownership handoff or consumer cutover occurred.
Runtime/governance Soroban migration is intentionally skipped for this
incompatible release. Live identities and the complete pre-upgrade NEAR
Wasm/state/roles/pending-operation inventory were not established, so production
upgrade approval remains blocked. Current-state preflight is not proof that old
code never deleted/reused an allocator. Host restoration/resource assertions are
not live ledger restoration or full Stellar simulation evidence. External
documentation URLs were not checked; local relative paths/headings were checked.

Final hygiene: `cargo fmt --all --check` and `git diff --check` passed; script
gate remained 122/122. All 38 original titles/UUIDs/severities/report statuses
match the cached audit records, and every row has a final disposition/evidence.
Local Markdown checker verified five relative links/headings across eight
changed guides; CI YAML parsed and its explicit compiled-Wasm gate was checked.
No remediation smoke source remains in either examples directory.

### Step 7 — critical review, bounded queues, and simplification

The final critical review found two release-blocking liveness defects in the
HAL-34 remedy itself, which earlier green gates had missed:

- **NEAR (reproduced on owned sandbox).** 63 legal Borsh proposals carrying
  900,000 opaque argument bytes filled the non-Admin slots; the reserved Admin
  revocation failed at ~73 Tgas with
  `Size of the recorded trie storage proof has exceeded the allowed limit (4.0 MB)`
  and the delegate retained its role (artifact `artifact://227`). Cancellation
  logs from rolled-back receipts still appear in logs, so indexers must use
  receipt status. A second witness showed 63 proposals with a 256-byte method
  exceeding `max_total_log_length` at 16,582 bytes of cancellation logs
  (artifact `artifact://285`).
- **Soroban (real SDK-25 host, default 100M CPU).** A mixed-role queue with 55
  maximum-size 16-source `SetProxy` bodies and 8 small manual-trip bodies
  trapped `Error(Budget, ExceededLimit)` during revocation, because purging a
  few small bodies rewrote all surviving large groups (artifact `artifact://222`;
  Stellar SLP-0004 keeps the 132,096-byte write limit). A homogeneous purge had
  already consumed 96,643,282 CPU (artifact `artifact://212`).

The user explicitly selected **bound aggregate pending bytes** rather than
staged revocation. The remedy preserves atomic physical purge, all existing
event shapes, individual large payloads, explicit IDs, and layout-compatible
NEAR `State`/header/proposal/operation encodings:

- NEAR: combined serialized state + pending bodies is capped at 2 MiB with a
  1 KiB Admin recovery reserve, sized exactly via `proposal_body_size`
  (Borsh-length, fallible). Cancellation-log bytes are capped at 12 KiB with a
  512-byte reserve, sized exactly via `cancellation_event_size` (the real
  NEP-297 encoder with a counting writer). Policy edits also require the
  resulting policy to remain removable within the same working-set bound, so
  an admitted configuration cannot lock itself in. New auxiliary accounting
  keys (working-set and cancellation-log bytes) live outside the serialized
  state, so no state-version bump. Legacy unmarked queues still drain:
  ordinary cancel/execute continue; new admissions and non-noop revocations
  wait until the queue is empty — an explicit transition, not a wholesale
  cancellation of compatible pending work.
- Soroban (fresh stack): proposal XDR values are capped at 122,880 bytes with
  a 2,048-byte Admin reserve; the new error is appended as
  `PendingProposalBytesExceeded = 13` (all prior codes unchanged). Purge is a
  single pass with exact byte reconciliation; missing/inconsistent bodies fail
  the whole invocation. The eight-group storage shape is unchanged.
- New read-only preflight check `upgrade.pending_resources` evaluates legacy
  and current pending bodies plus policy recoverability against these exact
  limits and instructs operators to drain/reconfigure oversized legacy state
  on the old code before upgrading.

Verification of the remedy (actual execution, not extrapolation):

- NEAR sandbox, exact 2,097,152-byte working set with 64 entries (including a
  1,250,000-byte payload, an Admin survivor and the revocation proposal):
  revocation succeeded at 119.585 Tgas execution / 119.895 Tgas total; the
  delegate lost the role and the survivor remained (artifact `artifact://289`).
- NEAR event budget: 29 delegated proposals with a 256-byte method produced
  11,447 counted cancellation-log bytes; the next create was rejected with the
  byte-limit error, and the reserved Admin revocation then succeeded
  (artifact `artifact://287`).
- Soroban generated-client smoke: a mixed queue saturated at 45 proposals,
  after which Admin revocation succeeded at 26,733,295 CPU, 19 memory-read
  entries, 12 write entries, and 120,388 write bytes. Permanent regressions
  additionally assert event size, CPU, combined footprint, write entries and
  write bytes for mixed-role, Admin-revocation and 63-small-body cases.

Simplification pass (six reviewers, findings DRY 2, YAGNI 0, STATE 0, FLOW 3,
SIGNAL 2, TEST 1; all eight accepted, zero deduplicated): shared
`default_proxy_config` fixture and `ADAPTER_DECIMALS` pins; last-emitted-bucket
dedup in adapter history; borrowed JSON deserialization in preflight;
unchecked map removal only after presence is proven; corrected stale adapter
proof guidance in PARITY/PROOF; `has_incompatible_rearm_payload` naming and
accurate preflight wording; consolidated duplicate captured-state upgrade
rehearsal. No observable contract changed.

Final follow-up verification: actual `tmplrmgr proxy-oracle preflight` against a
fresh sandbox with three large-but-legal pending bodies passed all six checks,
reporting working set 1,200,457/2,097,152 bytes and policy recovery 1,184 bytes.
The temporary smoke source was removed. Permanent sandbox gate: 56 passed
(51 excluded), including `cancellation_log_budget_prevents_long_method_revocation_failure`.
Affected NEAR/manager and Soroban all-target Clippy gates exited successfully
with `-D warnings`; `cargo fmt --all` and `git diff --check` passed.
