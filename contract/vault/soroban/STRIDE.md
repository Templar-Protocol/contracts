# Soroban Vault STRIDE Threat Model

This document captures a Soroban-specific STRIDE threat model for `contract/vault/soroban`, following the [Stellar Foundation STRIDE template](https://developers.stellar.org/docs/build/security-docs/threat-modeling/STRIDE-template).

---

## What are we working on?

### Scope

- Soroban contract entrypoints in `src/contract/mod.rs` (50+ public methods).
- Soroban auth adapter and RBAC wiring in `src/auth/mod.rs`.
- Soroban storage serialization/versioning in `src/storage/mod.rs`.
- Market adapter interactions used by allocation/refresh/withdraw flows.
- Shared policy/auth/governance logic from `curator-primitives` crate.
- Chain-agnostic state machine and effects from `templar-vault-kernel`.
- SEP-41 / ERC-4626 fungible vault interface in `src/fungible_vault.rs`.
- Upgrade/migration lifecycle via OpenZeppelin Stellar Contracts utilities.
- Governance contract timelocks and action lifecycle in `soroban-governance/src/lib.rs`.
- Share token contract with vault-controlled mint/burn and user-authorized transfers in `soroban-share-token/src/lib.rs`.

### Assets to Protect

- Underlying token balances held by the vault contract.
- Share accounting integrity (total_shares, total_assets, idle_assets, external_assets).
- Correctness of `VaultState` and withdrawal queue/order semantics.
- Authorization boundaries for governance/curator/guardian/allocator/sentinel/user actions.
- Liveness of withdrawal, allocation, and refresh workflows.
- Kernel-to-Soroban address mapping integrity (critical for token routing).
- Fee configuration and fee recipient integrity.
- Non-asset token balances recoverable via skim (airdrop/reward tokens).
- Governance action integrity — timelocks, abdication permanence, action routing.

### Trust Boundaries

1. **User ↔ Vault contract** — Soroban `require_auth()` enforces caller identity.
2. **Vault contract ↔ External token contracts** — SEP-41 token calls (mint, burn, transfer) for asset and share tokens.
3. **Vault contract ↔ Market adapter contracts** — Adapters supply/withdraw assets to external markets; adapter selection is derived from on-chain `supply_queue` + `allowed_adapters` ordering.
4. **Governance ↔ Curator** — Governance controls configuration (fees, curator appointment, caps, restrictions, adapter allowlist, upgrades/migration). Curator controls operational actions via RBAC.
5. **Kernel address mapping** — One-way SHA-256 hash from `SdkAddress` to kernel `[u8; 32]`; all effect routing depends on this mapping.
6. **Stored state ↔ Runtime** — Postcard-serialized blobs with version validation on deserialization.
7. **Upgrade boundary** — Two-step upgrade (upgrade → migrate) with blocking interim period.
8. **Governance contract ↔ Vault contract** — Governance contract invokes vault setters (`set_fees`, `set_sentinel`, `set_cap`, `set_skim_recipient`, `skim`, etc.) after timelock maturity. The vault trusts that the governance contract enforces timelocks; the vault itself applies changes immediately when called by governance.
9. **Vault contract ↔ Share token contract** — Share token enforces vault auth for `mint()`/`burn()` and `from.require_auth()` for user `transfer()` (while allowing vault-driven internal transfers). The vault address is immutable in the share token after initialization.

### Privilege Hierarchy

| Role | Set by | Powers |
|---|---|---|
| **Governance** | `initialize()` or `set_governance()` | Set curator/governance, fees, caps, cap groups, restrictions, supply queue, adapter allowlist, sentinel, skim recipient, and upgrade/migration controls. Must be a contract address. Can abdicate individual actions permanently. |
| **Curator** | `initialize()` or `set_curator()` (governance) | Operational role fallback in RBAC (guardian/allocator) and kernel-level privileged operations via role checks. |
| **Sentinel** | `initialize()` or `set_sentinel()` (governance) | Emergency pause/unpause and time-sensitive guardian actions. Distinct from curator — stored in `VaultDataKey::Sentinel` and loaded into RBAC as `Role::Sentinel`. |
| **Allocator** | Curator (via RBAC config) | Allocate supply/withdraw, refresh markets. **Note**: In current Soroban production, no separate allocator is wired — curator key is used for all allocator actions. |
| **Guardian** | Curator (via RBAC config) | Pause/unpause vault. **Note**: Same as allocator — no separate guardian wired in production. |
| **User** | Any signed account | Deposit, request withdrawal, execute withdrawal. Subject to restrictions (whitelist/blacklist/pause). |

### High-Level Dataflow

```
                           ┌──────────────────────────────────────────────────────────┐
                           │                    SOROBAN LEDGER                        │
                           │  ┌──────────────┐   ┌─────────────┐   ┌──────────────┐  │
                           │  │ Asset Token   │   │ Share Token  │   │  Persistent  │  │
                           │  │ (SEP-41)      │   │ (SEP-41)    │   │  Storage     │  │
                           │  └──────┬───────┘   └──────┬──────┘   └──────┬───────┘  │
                           │         │                   │                 │          │
                           └─────────┼───────────────────┼─────────────────┼──────────┘
                                     │                   │                 │
                  ┌──────────────────┼───────────────────┼─────────────────┼─────────┐
                  │                  │   VAULT CONTRACT   │                 │         │
                  │    ┌─────────────┴───────────────────┴─────────────────┴──────┐  │
                  │    │           Effect Interpreter (effects/mod.rs)            │  │
                  │    │  MintShares · BurnShares · TransferAssets · EmitEvent    │  │
                  │    └─────────────────────────┬───────────────────────────────┘  │
                  │                              │                                  │
                  │    ┌─────────────────────────┴───────────────────────────────┐  │
                  │    │              Kernel State Machine (kernel crate)         │  │
                  │    │  Deposit · RequestWithdraw · ExecuteWithdraw            │  │
                  │    │  BeginAllocating · FinishAllocating                     │  │
                  │    │  BeginRefreshing · FinishRefreshing · Pause             │  │
                  │    │  Fee math · Share accounting · Queue management         │  │
                  │    └────────────┬──────────────────────┬────────────────────┘  │
                  │                 │                      │                        │
                  │    ┌────────────┴────────┐  ┌─────────┴──────────┐             │
                  │    │   Auth Adapter       │  │   Storage Layer    │             │
                  │    │ (RBAC via curator-   │  │ (Versioned blobs,  │             │
                  │    │  primitives)         │  │  postcard serde)   │             │
                  │    └─────────────────────┘  └────────────────────┘             │
                  │                                                                │
                  └────────────────┬────────────────────────────┬──────────────────┘
                                   │                            │
          ┌────────────────────────┼────────────────────────────┼───────────────┐
          │                        │                            │               │
    ┌─────┴──────┐  ┌─────────────┴──────────┐  ┌──────────────┴──────┐  ┌─────┴──────┐
    │   Users     │  │   Governance           │  │   Curator           │  │  Adapters   │
    │ deposit     │  │ set_fees               │  │ allocate_supply     │  │ supply()    │
    │ withdraw    │  │ set_curator            │  │ allocate_withdraw   │  │ withdraw()  │
    │ redeem      │  │ set_cap                │  │ refresh_markets     │  │ total_assets│
    │ request_wd  │  │ set_restrictions       │  │                     │  │             │
    └────────────┘  │ set_supply_queue       │  │                     │  └─────────────┘
                     │ set_sentinel           │  └─────────────────────┘
                     │ set_skim_recipient     │
                     │ skim                   │
                     │ abdicate               │
                     └──────────────────────-─┘
```

### Interaction Inventory

| # | Interaction | Mutates State | Auth | Trust Boundary Crossed |
|---|---|---|---|---|
| I1 | User → `deposit_with_min` | Yes | `require_auth(owner)` | User ↔ Vault, Vault ↔ Asset Token, Vault ↔ Share Token |
| I2 | User → `request_withdraw` | Yes | `require_auth(owner)` | User ↔ Vault, Vault ↔ Share Token (escrow) |
| I3 | User → `execute_withdraw` | Yes | `require_auth(caller)` | User ↔ Vault, Vault ↔ Asset Token, Vault ↔ Share Token |
| I4 | User → `withdraw` / `redeem` (SEP-41) | Yes | `require_auth(operator+owner)` | User ↔ Vault, Vault ↔ Asset Token, Vault ↔ Share Token |
| I5 | Curator → `allocate_supply` | Yes | `require_auth(caller)` + RBAC Allocator | Vault ↔ Asset Token, Vault ↔ Adapter |
| I6 | Curator → `allocate_withdraw` | Yes | `require_auth(caller)` + RBAC Allocator | Vault ↔ Adapter, Vault ↔ Asset Token |
| I7 | Curator → `refresh_markets` | Yes | `require_auth(caller)` + RBAC Allocator | Vault ↔ Kernel state machine |
| I8 | Governance → `set_fees` | Yes | `require_auth(caller)` + governance check | Governance contract ↔ Vault storage |
| I9 | Governance → `set_curator` | Yes | `require_auth(caller)` + governance check | Governance ↔ Vault storage |
| I10 | Governance → `set_governance` | Yes | `require_auth(caller)` + governance check | Governance ↔ Vault storage |
| I11 | Governance → `set_share_token` (disabled / immutable post-init) | No (always reverts) | `require_auth(caller)` + governance check | Governance ↔ Vault storage |
| I12 | Governance → `set_supply_queue` / `set_cap` / `set_group_*` / `set_restrictions` / `remove_market` / `set_allowed_adapters` | Yes | `require_auth(caller)` + governance check | Governance ↔ Vault storage |
| I13 | Governance → `upgrade` | Yes | `require_auth(caller)` + governance check | Governance ↔ Soroban deployer |
| I14 | Governance → `migrate` | Yes | `require_auth(caller)` + governance check | Governance ↔ Vault storage |
| I15 | Governance → `cancel_migration` | Yes | `require_auth(caller)` + governance check | Governance ↔ migration state |
| I16 | Anyone → `initialize` | Yes | None (only checks `!Initialized`) | Deployer ↔ Vault storage |
| I17 | Anyone → `extend_ttl` | Yes (TTL only) | None | Caller ↔ Vault storage TTL |
| I18 | Anyone → read-only methods (`config`, `vault_snapshot`, `fee_info`, `total_assets`, `convert_to_*`, `max_*`, `preview_*`, `is_paused`, `withdraw_status`, `queue_tail`, `cap_groups`, `is_migrating`, `query_asset`, `supply_queue`) | No | None | Caller ↔ Vault storage |
| I19 | Governance → `set_paused` | Yes | `require_auth(caller)` + governance check | Governance ↔ Vault storage, OZ Pausable |
| I20 | Governance → `set_sentinel` | Yes | `require_auth(caller)` + governance check | Governance contract ↔ Vault storage (`VaultDataKey::Sentinel`) |
| I21 | Governance → `set_skim_recipient` | Yes | `require_auth(caller)` + governance check | Governance contract ↔ Vault storage (`VaultDataKey::SkimRecipient`) |
| I22 | Governance → `skim(token)` | Yes | `require_auth(caller)` + governance check | Governance contract ↔ Vault, Vault ↔ External token contract |
| I23 | Governance contract → `submit()` / `approve()` / `consume()` | Yes | `require_auth(admin)` + timelock maturity | Governance contract internal state |
| I24 | Governance contract → `abdicate(method_name)` | Yes | `require_auth(admin)` | Governance contract storage (irreversible) |
| I25 | Vault/User ↔ Share token (`transfer`/`mint`/`burn`) | Yes | `require_vault_invoker()` for `mint`/`burn`; `from.require_auth()` for user `transfer` | Vault/User ↔ Share token contract |

---

## What can go wrong?

### STRIDE Reminders

| Mnemonic Threat | Definition | Question |
|---|---|---|
| **S**poofing | The ability to impersonate another user or system component to gain unauthorized access. | Is the user who they say they are? |
| **T**ampering | Unauthorized alteration of data or code. | Has the data or code been modified in some way? |
| **R**epudiation | The ability for a system or user to deny having taken a certain action. | Is there enough data to "prove" the user took the action if they were to deny it? |
| **I**nformation Disclosure | The over-sharing of data expected to be kept private. | Is there anywhere where excessive data is being shared? |
| **D**enial of Service | The ability for an attacker to negatively affect the availability of a system. | Can someone, without authorization, impact the availability of the service? |
| **E**levation of Privilege | The ability for an attacker to gain additional privileges beyond what they were granted. | Are there ways for a user to gain access to additional privileges through legitimate or illegitimate means? |

### Threat Table

| Threat | Issues |
|---|---|
| **Spoofing** | **Spoof.1** — Caller impersonates a privileged actor (curator, governance, allocator, sentinel) by signing with a compromised key. Interactions: I5–I15, I19–I24. |
| | **Spoof.2** — `initialize()` has no caller authentication; anyone can front-run deployment to set themselves as curator/governance. Interaction: I16. (`contract/mod.rs` — only checks `!has(Initialized)`, no `require_auth`.) |
| | **Spoof.3** — If vault's `set_sentinel` or `set_skim_recipient` endpoints do not verify the caller is the governance contract, any caller with `require_auth` could invoke them directly, bypassing governance timelocks. Interactions: I20, I21. |
| **Tampering** | **Tamper.1** — Malicious or buggy adapter reports incorrect balances during refresh, causing `external_assets` accounting drift. Interactions: I5, I6, I7. |
| | **Tamper.5** — In `allocate_supply`, accounting is updated before the token transfer and adapter `supply()` call. The relevant Soroban risk is adapter correctness and full-transaction rollback semantics, not EVM-style reentrancy. Interaction: I5. |
| | **Tamper.6** — All persistent state uses postcard serialization. A deserialization bug or version mismatch — especially during upgrade/migrate — could corrupt state or brick the vault. Interaction: I14. |
| | **Tamper.7** — Fee timelock enforcement is now solely in the governance contract, not the vault. If governance contract has a bug in timelock validation or is upgraded to a version that skips timelocks, `set_fees` applies immediately on the vault with no secondary check. Interaction: I8, I23. |
| | **Tamper.8** — `RemoveMarket` via governance could be invoked for a market with non-zero external exposure, potentially stranding assets or breaking kernel accounting invariants. Interaction: I12. |
| | **Tamper.9** — Governance abdication uses free-form `method_name` strings. A typo or non-canonical string in `abdicate()` would not disable the intended action, while `require_not_abdicated` would pass for the actual method name. Interaction: I24. |
| **Repudiation** | **Repudiate.1** — Operators deny executing sensitive actions. Kernel state transitions emit `KernelEvent` envelopes, and `set_paused` emits OZ Pausable events. However, many privileged operations are auditable via transaction history. Interactions: I1–I7, I19. |
| | **Repudiate.2** — Some privileged/governance operations still rely primarily on tx-level observability where structured events are sparse or lightweight. Interactions: I5, I6, I8–I15. |
| | **Repudiate.3** — New governance actions (sentinel change, skim recipient change, abdication, skim execution) should emit structured events for auditability. Without events, irreversible actions like abdication are harder to detect and audit. Interactions: I20–I24. |
| **Information Disclosure** | **Info.1** — No confidentiality assumptions exist for contract storage or events; this is expected on-chain transparency. All state is publicly readable. Interaction: I18. |
| | **Info.2** — `config()` exposes the governance contract address. `fee_info()` exposes fee anchor details and fee WAD values. This metadata could aid targeted social engineering or phishing attacks against key holders. Interaction: I18. |
| | **Info.3** — `withdraw_status()` and `queue_tail()` expose withdrawal queue internals (next pending ID, active withdrawal op, current request ID). Observable queue state could enable front-running of large withdrawals or MEV-style extraction. Interaction: I18. |
| **Denial of Service** | **DoS.1** — Withdrawal progression depends on allocator/keeper execution by design for queued flows; this is an accepted operational liveness assumption. Interactions: I3, I5, I6. |
| | **DoS.2** — `upgrade()` enables migration state via OpenZeppelin's `enable_migration`, which blocks vault operations until `migrate()` or `cancel_migration()` completes. If governance is unavailable, migration can stall operational liveness. Interactions: I13, I14, I15. |
| | **DoS.3** — Adapter revert in `supply()`/`withdraw()` fails that allocation operation and primarily impacts the affected market lane (not global solvency). Interactions: I5, I6. |
| | **DoS.4** — Large postcard state remains a theoretical resource-limit risk, but current queue sizing analysis indicates practical headroom under expected usage. Interactions: I1–I7. |
| | **DoS.5** — Persistent storage entries expire if `extend_ttl` is not called within ~6 months (3,110,400 ledgers at ~5s/ledger). Mitigated: `extend_ttl()` is permissionless (anyone can call it). Interaction: I17. |
| | **DoS.6** — Governance abdication is irreversible. Abdicating safety-critical actions (e.g., `set_paused`, `set_guardian`) permanently removes the ability to respond to emergencies, creating a liveness/safety risk that cannot be recovered from. Interaction: I24. |
| | **DoS.7** — Setting cap to 0 via `SetCap` or `SetGroupCap` governance actions blocks new deposits/allocations into the affected market or group. While intentional for wind-down scenarios, misconfiguration could halt vault operations. Interaction: I12. |
| | **DoS.8** — `skim(token)` fails if the skim recipient address cannot receive the token (e.g., missing trustline on Stellar classic). This blocks recovery of that specific token but does not affect vault operations. Interaction: I22. |
| **Elevation of Privilege** | **Elevation.1** — Role mapping or configuration errors could grant unintended powers. Interactions: I1–I15. |
| | **Elevation.2** — Role separation is available in-code, but deployments can still collapse duties by leaving guardian/allocator/sentinel sets empty or reusing one key across roles. Interactions: I5–I7, I19, I20. |
| | **Elevation.3** — SEP-41 `withdraw()` and `redeem()` bypass the withdrawal queue entirely via `atomic_withdraw_internal()`, directly debiting `idle_assets` and burning shares. While limited to idle assets and requiring the vault to be in `Idle` state, this is an alternate withdrawal path not subject to queue ordering or cooldown. Interaction: I4. |
| | **Elevation.4** — Governance controls curator appointment, fees, caps/restrictions, adapters, sentinel, skim recipient, and upgrade/migration entrypoints. A single governance key compromise grants broad vault control. Interactions: I8–I15, I19–I24. |
| | **Elevation.5** — Skim recipient, once set via governance, receives all non-asset/non-share token balances when `skim()` is called. If the recipient is set to a malicious address, airdrop/reward tokens intended for vault depositors are redirected. Interaction: I21, I22. |
| | **Elevation.6** — The hard-coded `ESCROW_ADDRESS = [0u8; 32]` is mapped to the vault's own contract address during bootstrap. If any code path treats escrow as a distinct entity, it could cause address confusion or accounting drift. Interactions: I1–I3. |
| | **Elevation.7** — If `has_role` for `Role::Sentinel` falls back to curator when no sentinel is set, the curator implicitly gains sentinel powers. This may be acceptable for bootstrapping but should be an explicit documented decision. Interaction: I20. |
| | **Elevation.8** — Share token vault address is set at initialization and enforced for privileged operations (`mint`/`burn`). If the share token contract is upgradeable and the vault address is mutable post-init, an attacker who gains upgrade access could redirect privileged share operations to a different contract. Interaction: I25. |
| | **Elevation.9** — Governance timelock kind/decision function mappings in `soroban-governance` must match the sensitivity of each action. A misconfigured mapping (e.g., `Skim` using an immediate timelock instead of a delayed one) reduces the governance friction intended for high-impact actions. Interaction: I23. |

---

## What are we going to do about it?

| Threat | Remediations |
|---|---|
| **Spoofing** | **Spoof.1.R.1** — `require_auth()` is called on all privileged entrypoints. Role-based authorization via `ActionKind` → `required_role()` mapping in curator-primitives enforces least-privilege. **Spoof.1.R.2** — Operational: use multisig or segregated keys for curator and governance. Hardware security modules for high-value deployments. |
| | **Spoof.2.R.1** — Deploy and initialize atomically (e.g., via a factory contract that deploys + calls `initialize` in a single transaction). **Spoof.2.R.2** — Consider adding an `admin` parameter to `initialize()` or requiring the deployer's auth to prevent front-running. **Spoof.2.R.3** — Accepted risk: Soroban contract deployment is typically atomic with initialization in practice, but this is a procedural control, not a technical one. |
| | **Spoof.3.R.1** — ✅ **Implemented**: Vault endpoints `set_sentinel`, `set_skim_recipient`, and `skim` require `require_auth(caller)` + governance address check before applying changes. Only the governance contract can invoke these setters. **Spoof.3.R.2** — The governance contract itself enforces `require_auth(admin)` + timelock maturity before calling vault endpoints. |
| **Tampering** | **Tamper.1.R.1** — `allocate` and `refresh_markets` query adapters internally; the kernel validates state transitions. **Tamper.1.R.2** — Restrict adapters to vetted, audited contracts. Monitor external_assets drift vs adapter-reported totals. **Tamper.1.R.3** — Consider adding a maximum drift threshold that pauses the vault if adapter-reported assets deviate beyond tolerance. |
| | **Tamper.5.R.1** — Soroban transactions are atomic; if the adapter call fails, the entire transaction (including state update) reverts. **Tamper.5.R.2** — Focus review on adapter behavior, authorization boundaries, and accounting correctness around external calls instead of reentrancy guards. **Tamper.5.R.3** — The state is intentionally consistent at the external call boundary (`external_assets` updated before `allocate_supply` transfers, realized assets applied before `allocate_withdraw` returns). |
| | **Tamper.6.R.1** — Storage decode validates blob deserialization, version key presence, version match, and compatibility before using persisted state. **Tamper.6.R.2** — Pin postcard crate version; audit serialization round-trip in CI. **Tamper.6.R.3** — Upgrade/migrate flow validates storage version compatibility before proceeding. |
| | **Tamper.7.R.1** — ✅ **Implemented**: Fee timelocks are enforced in the `soroban-governance` contract via `TimelockKind::Fees` with decision functions from `curator-primitives` that distinguish fee increases (timelocked) from decreases (immediate). **Tamper.7.R.2** — Governance contract enforces `require_contract_address` — it cannot be an EOA. Upgrading the governance contract itself requires governance auth + timelock. **Tamper.7.R.3** — Future consideration: optional vault-side minimum delay or governance-proof verification for defense-in-depth. |
| | **Tamper.8.R.1** — `remove_market` in the vault validates via kernel state that the market has zero exposure before removal. Attempting to remove a market with outstanding allocations will fail. **Tamper.8.R.2** — Governance timelock on `RemoveMarket` provides observation window for operators to verify market state. |
| | **Tamper.9.R.1** — ✅ **Implemented**: `method_name_for_action()` in `soroban-governance` canonicalizes action kinds to method names, ensuring `abdicate()` and `require_not_abdicated()` use the same string mapping. **Tamper.9.R.2** — The governance contract's `abdicate()` accepts the same method name format used by `submit()`, preventing typo-based bypasses. |
| **Repudiation** | **Repudiate.1.R.1** — Actions require signed caller auth. Kernel state transitions emit `KernelEvent` envelopes via `publish_kernel_event`. OZ Pausable events emitted for pause/unpause. **Repudiate.1.R.2** — Maintain off-chain indexing/audit trails keyed by `op_id`, caller address, and timestamps. |
| | **Repudiate.2.R.1** — ✅ **Implemented**: Admin/allocation events are emitted for high-impact privileged operations (`set_curator`, `set_governance`, `set_fees`, `set_restrictions`, `set_allowed_adapters`, `upgrade`, `migrate`, `cancel_migration`, `allocate_supply`, `allocate_withdraw`). **Repudiate.2.R.2** — Soroban transaction-level observability (sender, function name, arguments) provides additional backup. |
| | **Repudiate.3.R.1** — Governance contract actions (`submit`, `approve`, `consume`, `revoke`, `abdicate`) are observable via Soroban transaction history (function name + arguments). **Repudiate.3.R.2** — Consider adding structured events in the governance contract for abdication and high-impact action completion. **Repudiate.3.R.3** — Vault-side setters (`set_sentinel`, `set_skim_recipient`, `skim`) emit admin events via the existing `emit_admin_event()` pattern. |
| **Information Disclosure** | **Info.1.R.1** — Accepted: no confidentiality assumptions on-chain. This is expected behavior for public blockchain contracts. **Info.1.R.2** — Avoid introducing unnecessary detailed event payloads that could leak operational patterns. |
| | **Info.2.R.1** — Accepted risk: governance and fee configuration are operational parameters that need to be publicly verifiable. **Info.2.R.2** — Protect governance/curator keys through operational security (multisig, HSM), not obscurity. |
| | **Info.3.R.1** — Accepted risk: queue transparency is a feature for user trust. **Info.3.R.2** — Monitor for unusual withdrawal patterns that might indicate front-running. **Info.3.R.3** — Consider: withdrawal cooldown (`DEFAULT_COOLDOWN_NS`) already provides some protection against same-block front-running. |
| **Denial of Service** | **DoS.1.R.1** — Accepted operational model: queued withdrawals require keeper/operator progression. Queue bounded by `MAX_PENDING = 1024`; guarded transitions prevent partial corruption. **DoS.1.R.2** — Operate redundant keepers and queue-staleness alerts. |
| | **DoS.2.R.1** — Ensure governance key/contract is resilient (multisig/DAO, signer redundancy) to avoid migration-state liveness failures. **DoS.2.R.2** — Test upgrade/migrate flow thoroughly on testnet before mainnet deployment. **DoS.2.R.3** — ✅ **Implemented**: `cancel_migration()` governance method added. Governance can cancel a pending migration, reverting the contract to operational state if `migrate()` has not been called. **DoS.2.R.4** — Document the upgrade procedure and key custody requirements. |
| | **DoS.3.R.1** — Adapter failures are localized to affected market operations; maintain diversified/vetted adapters. **DoS.3.R.2** — ✅ Implemented: adapter allowlist + queue-index routing enables rapid disabling of bad adapters. |
| | **DoS.4.R.1** — Accepted with monitoring based on current sizing math and workload expectations. **DoS.4.R.2** — Keep telemetry on queue depth/resource usage and revisit if workload or network limits change. |
| | **DoS.5.R.1** — `extend_ttl()` is permissionless — anyone can call it. TTL threshold is ~30 days (518,400 ledgers), extension is to ~6 months (3,110,400 ledgers). **DoS.5.R.2** — Operate a keeper bot that calls `extend_ttl` periodically. **DoS.5.R.3** — `save_state` and `save_address` automatically extend TTL on writes, providing additional safety margin. |
| | **DoS.6.R.1** — Abdication is intentionally irreversible — it provides credible commitment that governance cannot perform certain actions. **DoS.6.R.2** — Operational: maintain a clear policy on which actions are safe to abdicate (e.g., fee changes, skim) vs. which must never be abdicated (e.g., pause, upgrade). **DoS.6.R.3** — Consider: the governance contract could maintain a hardcoded deny-list of method names that cannot be abdicated (e.g., `set_paused`). |
| | **DoS.7.R.1** — Accepted: cap=0 is a valid wind-down configuration. **DoS.7.R.2** — Governance timelocks on `SetCap` and `SetGroupCap` provide observation window for operators to detect misconfiguration. |
| | **DoS.8.R.1** — Skim failure is isolated to the specific token and does not affect vault operations. **DoS.8.R.2** — `set_skim_recipient` should validate that the recipient is a valid address (non-zero). |
| **Elevation of Privilege** | **Elevation.1.R.1** — Centralized action authorization via `ActionKind` → `required_role()` in curator-primitives. **Elevation.1.R.2** — Preserve strict role review on new entrypoints, especially those that perform external calls after state transitions. **Elevation.1.R.3** — Keep governance-only setters and adapter allowlisting explicit and test-covered. |
| | **Elevation.2.R.1** — Accepted design decision for initial deployment: single curator key simplifies operations. **Elevation.2.R.2** — ✅ **Implemented**: Separate guardian, allocator, and sentinel address sets stored in persistent storage (`VaultDataKey::Guardians`, `VaultDataKey::Allocators`, `VaultDataKey::Sentinel`). Loaded in `load_vault_bootstrap()` via `rbac_config.add_role()`. **Elevation.2.R.3** — ✅ **Implemented**: Governance methods `set_guardians()`, `set_allocators()`, and `set_sentinel()` added to enable operational role separation. |
| | **Elevation.3.R.1** — Atomic withdrawals require vault to be in `Idle` state, sufficient `idle_assets`, and are capped to idle balance. **Elevation.3.R.2** — `refresh_fees_for_atomic()` is called before atomic withdrawals to ensure fees are current. **Elevation.3.R.3** — Document the atomic withdrawal path clearly in user-facing documentation as an intentional feature for immediate withdrawal from idle assets. |
| | **Elevation.4.R.1** — Require governance to be a multisig or DAO contract (enforced: `require_contract_address` in `set_governance`). **Elevation.4.R.2** — ✅ **Implemented**: `soroban-governance` contract enforces timelocks on all high-impact actions (fees, caps, sentinel, guardian, curator, restrictions, adapter changes, upgrade). Decision functions from `curator-primitives` determine whether changes are immediate or timelocked based on direction (increase vs decrease). **Elevation.4.R.3** — Monitor all governance transactions with alerting. |
| | **Elevation.5.R.1** — ✅ **Implemented**: `skim()` explicitly rejects the asset token and share token, preventing drainage of vault-critical balances. **Elevation.5.R.2** — Skim recipient is set via timelocked governance action (`SetSkimRecipient`). **Elevation.5.R.3** — Operational: governance should set skim recipient to a treasury/multisig, not an individual key. |
| | **Elevation.6.R.1** — `ESCROW_ADDRESS = [0u8; 32]` is mapped to the vault's own contract address, ensuring escrow operations (share transfers during withdrawal) route correctly. **Elevation.6.R.2** — The escrow mapping is set during vault bootstrap and is consistent across all invocations. No additional remediation needed. |
| | **Elevation.7.R.1** — ✅ **Implemented**: `SorobanAuth::has_role` checks the sentinel address distinctly from the curator. When no sentinel is set (`VaultDataKey::Sentinel` absent), `Role::Sentinel` checks fall back to curator as a bootstrap convenience. **Elevation.7.R.2** — Operational: deploy with an explicit sentinel address from day one. Use `set_sentinel` to establish a distinct sentinel as soon as operational key infrastructure is ready. |
| | **Elevation.8.R.1** — ✅ **Implemented**: Share token stores the vault address at initialization; `require_vault_invoker()` is enforced on `mint`/`burn`, while user transfers require `from.require_auth()`. **Elevation.8.R.2** — The share token contract has no admin endpoint to change the vault address post-initialization. **Elevation.8.R.3** — If the share token contract is made upgradeable in the future, ensure the vault address remains immutable across migrations. |
| | **Elevation.9.R.1** — ✅ **Implemented**: `soroban-governance` maps each `GovernanceActionKind` to a `TimelockKind` with appropriate sensitivity levels. Decision functions from `curator-primitives` enforce directional timelocks (e.g., fee increases are timelocked, decreases are immediate). **Elevation.9.R.2** — Add integration tests that verify timelock kind mappings for all governance action kinds (partially done: 7 governance tests cover sentinel, cap, and core actions). |

---

## Soroban-Specific Notes

- **Reentrancy model**: Soroban does not expose the traditional EVM reentrancy model. Cross-contract calls are still synchronous within a transaction, so the main review focus is adapter behavior, authorization, and atomic rollback semantics.
- **Storage decode**: Validates blob deserialization, version key presence, version match, and compatibility before using persisted state.
- **Storage TTL**: Persistent entries must be maintained via `extend_ttl`. Default threshold is ~30 days; extension target is ~6 months. Automatic extension on state writes provides additional safety.
- **Resource limits**: Soroban network-level constraints on CPU, memory, ledger footprint, and transaction size. Writes fail atomically when limits are exceeded — no partial state corruption.
- **Auth model**: Soroban `require_auth()` is invocation-scoped. The vault uses it for caller identity, then delegates to RBAC for role checks. `require_auth` on the caller is distinct from contract-level auth (the vault contract itself authorizes token operations as the contract address).
- **Kernel architecture**: The `#[contractimpl]` block provides the Soroban on-chain API; `CuratorVault` is chain-agnostic and reuses `curator-primitives` for auth/rbac/policy and `templar-vault-kernel` for state machine/transitions/effects/fee math.
- **Production flows**: `allocate_supply` and `allocate_withdraw` directly manage the kernel state machine, then perform external calls. `refresh_markets` drives the kernel refresh flow through the runtime helper that persists kernel state and effects.
- **Removed methods**: `sync_external_assets`, `verify_external_assets_against_adapter`, `manual_reconcile`, `abort_allocating`, `abort_refreshing`, `abort_withdrawing`, `recover`, `settle_payout`, `refresh_fees`, and market lock methods (`acquire_market_lock`, `release_market_lock`, `is_market_locked`) no longer exist in the Soroban implementation.
- **Fee timelock architecture**: Fee timelocks are enforced exclusively in the `soroban-governance` contract. The vault's `set_fees` applies changes immediately when called by governance. The vault-level `PendingFeesChange` queue has been removed. This is a deliberate single-responsibility design: governance owns timelock policy, vault owns state application.
- **Share token policy**: The share token enforces `require_vault_invoker()` on `mint`/`burn`, and user transfers require `from.require_auth()`. Vault-driven internal transfer effects remain supported. The vault address in the share token is set at initialization and is immutable.
- **Governance abdication**: `abdicate(method_name)` is irreversible. Once an action is abdicated, `require_not_abdicated` permanently blocks `submit()` for that method. This provides credible commitment that governance cannot perform certain actions — a feature, not a bug, when used for depositor protection (e.g., abdicating fee increases).

---

## Did we do a good job?

### Checklist

- [x] Has the data flow diagram been referenced since it was created?
  - Yes — all threats are mapped to specific interactions (I1–I25) in the dataflow.
- [x] Did the STRIDE model uncover new design issues or concerns?
  - Yes — several significant findings:
    - **DoS.2**: Upgrade/migrate enters migration mode and can stall liveness without operator recovery.
    - **Repudiate.2**: Several privileged operations initially lacked structured eventing.
    - **Elevation.2**: Role separation can still be weakened by deployment/operator key configuration.
    - **Spoof.2**: `initialize()` lacks caller authentication.
    - **Tamper.7** (new): Fee timelock single-point enforcement in governance contract.
    - **DoS.6** (new): Irreversible abdication of safety-critical actions.
    - **Elevation.9** (new): Timelock kind/decision mapping drift risk.
- [x] Did the treatments adequately address the issues identified?
  - Yes. The top priority items have been implemented:
    1. ✅ Events on all privileged operations (Repudiate.2.R.1) — `emit_admin_event()` / `emit_alloc_event()` helpers.
    2. ✅ Fee governance hardening — timelocked via `soroban-governance` with directional decision functions from `curator-primitives`. Vault-level duplicate timelock removed.
    3. ✅ Adapter routing hardening — adapter is no longer caller-supplied; routing is derived from on-chain `supply_queue` + `AllowedAdapters`.
    4. ✅ Role separation (Elevation.2.R.2/R.3) — `VaultDataKey::Guardians` / `VaultDataKey::Allocators` / `VaultDataKey::Sentinel` + governance setters.
    5. ✅ Governance-only upgrade/migrate + cancel path — `upgrade()`/`migrate()` moved to governance, plus `cancel_migration()`.
    6. ✅ Sentinel parity (Elevation.7.R.1) — Distinct sentinel address stored and loaded into RBAC, with `set_sentinel` governance action.
    7. ✅ Skim parity (Elevation.5.R.1) — `skim()` rejects asset/share tokens, recipient set via timelocked governance.
    8. ✅ Governance timelocks on all high-impact actions (Elevation.4.R.2) — `soroban-governance` with per-action timelock kinds.
    9. ✅ Governance abdication (DoS.6.R.1) — `abdicate()` permanently disables actions with canonical method name mapping.
  - Remaining open items (documented as future work): `initialize()` auth (Spoof.2.R.2), adapter/runtime monitoring controls (Tamper.1.R.2/R.3), vault-side fee delay for defense-in-depth (Tamper.7.R.3), abdication deny-list for safety-critical actions (DoS.6.R.3), structured governance events (Repudiate.3.R.2).
- [ ] Have additional issues been found after the threat model?
  - To be updated after external audit.

### Severity Summary

| Severity | Count | Key Items |
|---|---|---|
| **Critical (Mitigated)** | 1 | DoS.2 (migration liveness risk) — ✅ governance cancel path + governance-gated upgrade/migrate |
| **High (Mitigated)** | 3 | Spoof.3 (direct vault setter bypass) — ✅ governance-only auth, Tamper.7 (fee timelock single-point) — ✅ governance contract enforcement + contract-only requirement, Elevation.5 (skim recipient redirect) — ✅ timelocked + asset/share rejection |
| **High** | 2 | Spoof.2 (init auth), Elevation.4 (governance compromise blast radius) |
| **Medium (Mitigated)** | 4 | Tamper.8 (market removal with exposure) — ✅ kernel validates zero exposure, Tamper.9 (abdication typo) — ✅ canonical method names, Elevation.9 (timelock mapping drift) — ✅ per-action mapping + tests, Repudiate.2 — ✅ expanded event coverage |
| **Medium** | 4 | Tamper.1, Tamper.5, Elevation.3, DoS.6 (irreversible abdication of safety actions) |
| **Medium (Operational)** | 1 | Elevation.2 (role-separation can still be weakened by deployment key choices) |
| **Low (Accepted/Monitored)** | 1 | DoS.4 (resource-limit risk with current sizing headroom) |
| **Low** | 6 | Tamper.6, Info.2, Info.3, DoS.5, DoS.7, DoS.8 |
| **Low (Mitigated)** | 2 | Elevation.7 (sentinel fallback to curator) — ✅ documented bootstrap behavior, Elevation.8 (share token vault address) — ✅ immutable post-init |
| **Accepted** | 2 | Info.1 (on-chain transparency), Elevation.6 (escrow address) |

### Review Cadence

- Revisit this model when adding new privileged actions, new adapters, or storage schema changes.
- Revisit after significant Soroban SDK or network resource limit changes.
- Revisit after any upgrade/migrate deployment.
- Revisit after external security audit findings.
- Revisit after any governance contract upgrade or new action kind addition.
