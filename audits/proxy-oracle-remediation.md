# Proxy-oracle audit remediation ledger

Baseline: `a3c8a9653a6a460731881e327253a811aa2c07e9` (clean). Cached audit targets `fde891da949ccdc615198175847de72cfdaef828`; engagement `f3bc6599-6681-4e93-902b-813bd0e2c93d`. HAL numbers use **halIndex**, not findingIndex. Original report status is evidence metadata, not proof of a current fix. No live transactions or upgrades are authorized.

New Soroban aggregation/pin XDR is **fresh-stack only**: incompatible runtime/governance in-place migration is skipped. Existing live identities were not established. NEAR upgrades require exact deployed-artifact/state/pending-operation inventory; deleted allocator history cannot be reconstructed.

## HAL-1: Single Role Holder Can Fill Pending Proposal Queue

- Original UUID: `9d21d325-84c0-4ff5-bc7d-8fea19323dc2`. Audit severity: **low**. Report status: **Pending**.
- Chain: Both. Precondition: Delegated role holder fills shared queue.
- Current assessment: Valid unresolved.
- Disposition: Reserve slot 64 for Admin; retain cap and expected IDs.
- Audit source locations: `contract/proxy-oracle/governance-kernel/src/lib.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-2: Unpinned Source Decimals Allow Silent Price Rescaling

- Original UUID: `3071300a-fabf-4dc9-bf55-337bc20fffc1`. Audit severity: **low**. Report status: **Pending**.
- Chain: Soroban. Precondition: Admitted upstream changes decimals after admission.
- Current assessment: Valid unresolved; decimals already bounded.
- Disposition: Pin expected decimals at admission and refresh; requires source upgrade/misconfiguration.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/conversion.rs`; `contract/proxy-oracle/soroban/contract/src/refresh.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-3: History Keeps Serving Prices After A Halt

- Original UUID: `f185765f-2667-4e6b-9a8a-f2e071c92d82`. Audit severity: **low**. Report status: **Solved**.
- Chain: Soroban. Precondition: Manual or automatic halt with historical samples.
- Current assessment: Existing fix; regression required.
- Disposition: Historical reads consult breaker state; history remains freshness-independent.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/lib.rs`; `contract/proxy-oracle/soroban/sep40-adapter-contract/src/lib.rs`; `contract/proxy-oracle/soroban/contract/src/refresh.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-4: Circuit Breaker Ring Buffer Accepts Duplicate Samples

- Original UUID: `5995c3ab-1135-4396-be70-261809879bba`. Audit severity: **informational**. Report status: **Solved**.
- Chain: Both. Precondition: Duplicate/older publication with zero sampling interval.
- Current assessment: Existing fix; regression required.
- Disposition: Persist only increasing source publication times; generic ring buffer unchanged.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/ring_buffer.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-5: Empty Breaker History Skips First Price Check

- Original UUID: `5387c56f-c683-4b21-83dd-436515226088`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Relative rule has no trusted first sample.
- Current assessment: Intentional bootstrap limitation.
- Disposition: Preserve initial baseline semantics and explicit cumulative baseline.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/stepwise_change.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-6: Admin Grant Accepts An Address That Cannot Sign

- Original UUID: `892faed0-d266-4647-a238-e30e9c81999d`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Admin grants unusable account then revokes usable Admin.
- Current assessment: Valid unresolved.
- Disposition: Require grantee co-authorization; reject governance-self Admin; retain last-Admin guard.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/roles.rs`; `contract/proxy-oracle/soroban/governance-contract/src/engine.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-7: Role Mapping Lets Lower Roles Veto Admin Proposals

- Original UUID: `2608f406-c729-447f-b73f-7da5caf2caf1`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Peer with same operation role cancels another creator proposal.
- Current assessment: Valid unresolved.
- Disposition: Creator-or-Admin cancellation; current authorized peer may still execute.
- Audit source locations: `contract/proxy-oracle/soroban/governance-common/src/lib.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-8: Circuit Breaker Rules Ignore Sample Time Gaps

- Original UUID: `c4d8b7b2-9170-4810-a8af-6c1c08eddc60`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Keeper samples have long time gaps.
- Current assessment: Intentional sample-count semantics.
- Disposition: Document cadence/recalibration; do not change mathematics.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/observation.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/stepwise_change.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/windowed_change_delta.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-9: Opposite-Sign Window Changes Sum Instead Of Subtract

- Original UUID: `43c53da8-628e-4aa6-a3c0-51fb5a7f58c9`. Audit severity: **informational**. Report status: **Solved**.
- Chain: Both. Precondition: Opposite-direction windows.
- Current assessment: Existing fix; regression required.
- Disposition: Current window means/sums replace historical sign error.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/windowed_change_delta.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/math.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-10: Ownership Transfer Always Uses Maximum Acceptance Window

- Original UUID: `9592a6f2-161e-489b-a7ca-39dcd6381bd0`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Executed ownership transfer remains pending.
- Current assessment: Valid unresolved.
- Disposition: Append cancellation action sharing Admin authorization and transfer TTL.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/engine.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-11: Arbitrary Tie-Break Picks Aggregate Timestamp And Exponent

- Original UUID: `6ae3db6a-c3a8-4465-a172-8eb5f53dac25`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Numerically equal endpoints have different metadata.
- Current assessment: Valid unresolved metadata nondeterminism.
- Disposition: Median-local total order; preserve numeric SpecificPrice equality and exact exponents.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/aggregator/method/median/specific_price.rs`; `contract/proxy-oracle/kernel/src/proxy/aggregator/method/median/mod.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-12: Constructor Allows Zero Timelock, Unverified Proxy Oracle

- Original UUID: `24f405bf-20e3-4727-ae08-ec42ef45e26c`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Zero bootstrap timelocks/nonreciprocal constructor.
- Current assessment: Intentional bootstrap facility.
- Disposition: Verify reciprocal wiring and hardened timelocks as deployment postconditions.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-13: Rearmed Circuit Breakers Skip Trip Checks During Cool-Down

- Original UUID: `d1dc13cd-d7e5-443f-a4ca-270b1b78e3a9`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Privileged rearm enters cooldown.
- Current assessment: Intentional recovery authority.
- Disposition: Preserve ArmedAfter and histories; no fail-closed cooldown.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-14: Proxy Reconfiguration Leaves Stale History And Breakers

- Original UUID: `91a93fd5-087d-4365-8faf-168bfc0e5690`. Audit severity: **informational**. Report status: **Partially Solved**.
- Chain: Both. Precondition: Aggregator/source/quorum replacement or destructive removal.
- Current assessment: Partly fixed; allocator reset unresolved.
- Disposition: Retain cache/history clearing and only preserve next breaker ID on full reset.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/lib.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-15: Governance Keeper Never Refreshes Role Storage TTLs

- Original UUID: `1d75c83b-c8be-407d-8eb7-eeff7476b396`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Dormant role count/index/membership/ExistingRoles archive.
- Current assessment: Valid unresolved.
- Disposition: Bounded role TTL pages plus normal keeper ExistingRoles renewal; no membership cap.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/roles.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-16: Stale Breaker Id Silently Voids NEAR Proposal

- Original UUID: `5d4a4f15-0fb7-479b-86c7-c5b0970c0806`. Audit severity: **informational**. Report status: **Pending**.
- Chain: NEAR. Precondition: Two adds captured same expected breaker ID.
- Current assessment: Guard intentional; operator failure visibility unresolved.
- Disposition: Preserve one-shot dispatch and explicit IDs; reject failed target receipts in manager.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/error.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-17: Breaker Added Event Drops Configured Parameters

- Original UUID: `3c1199a0-0376-4f29-a6c2-9d8564b774b2`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Breaker added event omits parameters.
- Current assessment: Valid unresolved.
- Disposition: Keep old event; add typed config/baseline detail from existing DTO.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/events.rs`; `contract/proxy-oracle/soroban/contract/src/codes.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-18: Constructor Accepts Owner With No Reciprocity Check

- Original UUID: `1fc1450f-8b20-4d5a-8346-0dc6628abcf1`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Bootstrap owner not governance; adapter base differs.
- Current assessment: Base guard existing; bootstrap intentional.
- Disposition: Test ownership/wiring postconditions; avoid circular constructors.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/lib.rs`; `contract/proxy-oracle/soroban/sep40-adapter-contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-19: Matured Governance Proposals Remain Executable Indefinitely

- Original UUID: `b444f04e-eb41-4e7b-86d9-79e51be7157c`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Matured proposal persists indefinitely.
- Current assessment: Intentional minimum delay.
- Disposition: No expiry/grace period; current authorization and explicit cancellation remain.
- Audit source locations: `contract/proxy-oracle/governance-kernel/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-20: Aggregation Error Codes Collapse To One Value

- Original UUID: `44d398e9-bf01-4b10-8f39-b8119f0df191`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Aggregation errors share code.
- Current assessment: Valid unresolved diagnostics.
- Disposition: Keep unrelated codes; append 101/102 with expected/actual detail.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/aggregator/method/mod.rs`; `contract/proxy-oracle/soroban/contract/src/codes.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-21: Blocked Price Event Omits Which Breaker Tripped

- Original UUID: `8e7760b8-232e-48ce-830b-95ad21e15fa6`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Already blocked refresh including no-source path.
- Current assessment: Valid unresolved diagnostics.
- Disposition: Publish exact blocking IDs on every blocked refresh; manual IDs empty.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/events.rs`; `contract/proxy-oracle/soroban/contract/src/codes.rs`; `contract/proxy-oracle/soroban/contract/src/refresh.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-22: Breaker History Resize Skips Clear Or Rearm

- Original UUID: `0cf73c69-56df-47bd-becc-6615214bebe5`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: History capacity/cadence changed with installed rules.
- Current assessment: Existing rejection/preservation; regression required.
- Disposition: Reject insufficient capacity atomically and retain valid history.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/ring_buffer.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/windowed_change_delta.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-23: Governance Create Requires Predicting Shared Proposal Id

- Original UUID: `71b13c1e-240c-40d0-9ad0-4e922d9bc88e`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Contending or replayed explicit proposal ID.
- Current assessment: Intentional at-most-once guard; contention remains.
- Disposition: Retain expected IDs; unknown outcome stops, never reselect or claim ownership.
- Audit source locations: `contract/proxy-oracle/governance-kernel/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-24: Price Decline Measurement Caps Change At 1.0

- Original UUID: `f430a208-0be7-4b13-a6f4-e3efca776cf7`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Decline threshold at or above 1.
- Current assessment: Intentional conventional relative math.
- Disposition: Validate 0 < threshold <= 1; exact 1 allowed with strict comparison.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/math.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/stepwise_change.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/rule/monotonic_run.rs`; `contract/proxy-oracle/soroban/contract/src/conversion.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-25: Sep40Adapter Returns Price History Oldest First

- Original UUID: `093e6a69-09a5-49a7-972d-a385f8e8fcb7`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: SEP-40 historical bucket order/query alignment.
- Current assessment: Valid unresolved.
- Disposition: Newest-first distinct buckets; bucket query time and choose newest convertible entry.
- Audit source locations: `contract/proxy-oracle/soroban/sep40-adapter-contract/src/lib.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-26: Remove Proxy Skips Existence Check And Emits Event

- Original UUID: `4ba68345-7b17-4cab-bc2b-f3520b9ff022`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Remove unknown asset.
- Current assessment: Valid unresolved.
- Disposition: Existence guard before all mutation/events.
- Audit source locations: `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-27: Runtime Failure Mapping In Governance Call Is Dead Code

- Original UUID: `c5aa2669-eca2-4b7d-b349-ba1eb6c7ebc8`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Governed runtime invocation returns failure.
- Current assessment: Valid unresolved error mapping.
- Disposition: try_invoke_contract; RuntimeFailed with whole-transaction rollback/proposal retention.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/engine.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-28: Queue-Full Governance Error Aliases As Invalid Input

- Original UUID: `3c3238a9-14a2-436f-91fe-d30a5787bc8e`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Pending queue full.
- Current assessment: Valid unresolved error alias.
- Disposition: Append TooManyPendingProposals=12; retain all existing codes.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`; `contract/proxy-oracle/soroban/governance-common/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-29: Proxy Config Lacks Age And Source Bounds

- Original UUID: `db38f61d-0b93-4cdd-a64d-831aabc6d4ec`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Missing freshness/source bounds or duplicate wrappers.
- Current assessment: Existing bounds; provider independence not provable.
- Disposition: Retain fail-closed and zero-age policy; no invented identity/stale fallback.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/freshness_filter.rs`; `contract/proxy-oracle/soroban/contract/src/conversion.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`; `contract/proxy-oracle/soroban/contract/src/refresh.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-30: NEAR Proxy Oracle Lacks Aggregator Config Validation

- Original UUID: `eec1926b-3050-4162-ab28-818562fc739a`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Authorized config has duplicate descriptors/impossible quorum.
- Current assessment: Valid unresolved on NEAR.
- Disposition: Shared ingress shape validator; Priority first-valid fallback, no quorum.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/aggregator/mod.rs`; `contract/proxy-oracle/kernel/src/proxy/aggregator/method/median/mod.rs`; `contract/proxy-oracle/kernel/src/proxy/mod.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-31: Unbounded Sample Interval Freezes Circuit Breaker History

- Original UUID: `68fe64c7-9b06-4df2-9c56-6c9c13604ac6`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Seconds conversion overflows nanoseconds.
- Current assessment: Valid unresolved.
- Disposition: Checked runtime/static parsing; zero and all representable intervals allowed.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-32: Unbounded Rearm Timestamp Can Permanently Disable Breaker

- Original UUID: `1fde15dd-2c3a-48bf-b26b-fc9a230f8ba2`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Relative rearm delay overflows timestamp.
- Current assessment: Existing checked arithmetic; regression required.
- Disposition: Preserve relative delay and reject overflow; long intentional delay remains authorized.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/status.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-33: Governance Validation Admits Proposals The Runtime Rejects

- Original UUID: `8cd2a72b-1924-416c-90c6-a98f284a4f61`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Statically impossible breaker proposal queues.
- Current assessment: Valid partly unresolved.
- Disposition: Share pure shape/representability/feasibility parsing; execution retains dynamic checks.
- Audit source locations: `contract/proxy-oracle/soroban/governance-common/src/lib.rs`; `contract/proxy-oracle/soroban/governance-contract/src/engine.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`; `contract/proxy-oracle/soroban/contract/src/conversion.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-34: Role Revocation Leaves Queued Proposals Executable

- Original UUID: `c2bc2295-ef24-46b1-a586-e103eac7b97b`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Revoked creator queued proposals survive.
- Current assessment: Valid unresolved hygiene.
- Disposition: Purge only now-unauthorized creator operations after successful role removal.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/roles.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`; `contract/proxy-oracle/soroban/governance-contract/src/engine.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-35: Proposal Maturity Ignores Later Timelock Increases

- Original UUID: `9859b5f4-75a8-4161-b0f4-94f275ac1ce9`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Policy TTL increases after creation.
- Current assessment: Intentional snapshotted maturity.
- Disposition: Preserve queued maturity; document no retroactive delay.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/events.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`; `contract/proxy-oracle/soroban/governance-contract/src/engine.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-36: Decoupled Proposal TTL Policies Risk Liveness Friction

- Original UUID: `6adf5dc6-5bfe-4090-8531-9bf24d1367d3`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Soroban. Precondition: Proposal body archived while ID persists.
- Current assessment: Restoration requirement, not ordinary orphan.
- Disposition: Maintain body-required cancellation and keeper; never silently drop missing body IDs.
- Audit source locations: `contract/proxy-oracle/soroban/governance-contract/src/storage.rs`; `contract/proxy-oracle/soroban/governance-contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-37: Reused Breaker Id Matches Wrong Breaker State

- Original UUID: `2d6d1a28-0d6d-4ab5-a272-3c03ddd6ea2c`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Asset removed/recreated resets breaker allocator.
- Current assessment: Valid unresolved wrong-object hazard.
- Disposition: Reset bounded tombstone preserving next_id; hide absent-asset tombstones.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/error.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

## HAL-38: Unversioned Breaker Status Encoding Risks Silent Misdecode

- Original UUID: `7e05397f-523a-4def-9468-c1e01ec989b7`. Audit severity: **informational**. Report status: **Pending**.
- Chain: Both. Precondition: Code upgrade changes positional breaker bytes.
- Current assessment: Compatibility risk; not present forged-state attack.
- Disposition: Freeze old bytes/load real fixtures; skip incompatible Soroban runtime/governance migration.
- Audit source locations: `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/status.rs`; `contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs`; `contract/proxy-oracle/soroban/contract/src/storage.rs`; `contract/proxy-oracle/soroban/contract/src/lib.rs`.
- Scenario/evidence: baseline source assessment and executed verification recorded below as implementation proceeds; no passing result claimed.

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
- Kani has been started; no proof result is claimed here until it completes.

### Approved amendment — Soroban proposal body grouping

Actual governance host smoke reached delegate slot 63/Admin slot 64 and rejected
the original expected ID after cancellation. Revoking the delegated creator with
61 queued bodies then exceeded the host's transaction limits: **141 footprint
entries > 100**, **68 write entries > 50**. No test limits were increased and the
queue scenario was not reduced.

The user explicitly approved **Group proposal bodies** rather than requiring an
operator queue-drain precondition. The fresh Soroban stack will group unchanged
`Proposal` values into bounded persistent groups, preserving header fields,
creation IDs/arguments/results, action/policy shapes, and checkpoint identity.
This is an additional **fresh-stack-only storage change**; no old decoder,
fallback, or in-place governance migration is authorized. Worst-case group bytes,
atomic revocation and keeper budgets still require executed verification.

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
- Manager CLI help was exercised; it now explains explicit-ID retries, and
  selected proposal/breaker IDs are emitted before submission. Full manager
  target-receipt failure smoke remains a separate required step.

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
  This is not yet proof of an old-Wasm-to-new-Wasm adapter upgrade.
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
