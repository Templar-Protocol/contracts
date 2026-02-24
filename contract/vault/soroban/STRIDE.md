# Soroban Vault STRIDE Threat Model

This document captures a Soroban-specific STRIDE threat model for `contract/vault/soroban`, following the [Stellar Foundation STRIDE template](https://developers.stellar.org/docs/build/security-docs/threat-modeling/STRIDE-template).

---

## What are we working on?

### Scope

- Soroban contract entrypoints in `src/contract/mod.rs` (47 public methods).
- Soroban auth adapter and RBAC wiring in `src/auth/mod.rs`.
- Soroban storage serialization/versioning in `src/storage/mod.rs`.
- Market adapter interactions used by allocation/refresh/withdraw flows.
- Shared policy/auth/governance logic from `curator-primitives` crate.
- Chain-agnostic state machine and effects from `templar-vault-kernel`.
- SEP-41 / ERC-4626 fungible vault interface in `src/fungible_vault.rs`.
- Upgrade/migration lifecycle via OpenZeppelin Stellar Contracts utilities.

### Assets to Protect

- Underlying token balances held by the vault contract.
- Share accounting integrity (total_shares, total_assets, idle_assets, external_assets).
- Correctness of `VaultState` and withdrawal queue/order semantics.
- Authorization boundaries for governance/curator/guardian/allocator/user actions.
- Liveness of withdrawal, allocation, and refresh workflows.
- Kernel-to-Soroban address mapping integrity (critical for token routing).
- Fee configuration and fee recipient integrity.

### Trust Boundaries

1. **User ↔ Vault contract** — Soroban `require_auth()` enforces caller identity.
2. **Vault contract ↔ External token contracts** — SEP-41 token calls (mint, burn, transfer) for asset and share tokens.
3. **Vault contract ↔ Market adapter contracts** — Adapters supply/withdraw assets to external markets; adapter address is a caller parameter (no on-chain registry).
4. **Governance ↔ Curator** — Governance controls configuration (fees, curator appointment, share token, caps). Curator controls operations (allocations, upgrades, address registration).
5. **Kernel address mapping** — One-way SHA-256 hash from `SdkAddress` to kernel `[u8; 32]`; all effect routing depends on this mapping.
6. **Stored state ↔ Runtime** — Postcard-serialized blobs with version validation on deserialization.
7. **Upgrade boundary** — Two-step upgrade (upgrade → migrate) with blocking interim period.

### Privilege Hierarchy

| Role | Set by | Powers |
|---|---|---|
| **Governance** | `initialize()` or `set_governance()` | Set curator, governance, share token, fees, caps, cap groups, restrictions, supply queue. Must be a contract address. |
| **Curator** | `initialize()` or `set_curator()` (governance) | Upgrade WASM, migrate, register addresses. Also acts as fallback for all RBAC roles (guardian, allocator). |
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
    │ request_wd  │  │ set_restrictions       │  │ upgrade / migrate   │  │             │
    └────────────┘  │ set_supply_queue       │  │ register_address    │  └─────────────┘
                     └──────────────────────-─┘  └─────────────────────┘
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
| I8 | Governance → `set_fees` | Yes | `require_auth(caller)` + governance check | Governance ↔ Vault storage |
| I9 | Governance → `set_curator` | Yes | `require_auth(caller)` + governance check | Governance ↔ Vault storage |
| I10 | Governance → `set_governance` | Yes | `require_auth(caller)` + governance check | Governance ↔ Vault storage |
| I11 | Governance → `set_share_token` | Yes | `require_auth(caller)` + governance check | Governance ↔ Vault storage |
| I12 | Governance → `set_supply_queue` / `set_cap` / `set_group_*` / `set_restrictions` / `remove_market` | Yes | `require_auth(caller)` + governance check | Governance ↔ Vault storage |
| I13 | Curator → `upgrade` | Yes | `require_auth(caller)` + curator check | Curator ↔ Soroban deployer |
| I14 | Curator → `migrate` | Yes | `require_auth(caller)` + curator check | Curator ↔ Vault storage |
| I15 | Curator → `register_address` | Yes | `require_auth(caller)` + curator check | Curator ↔ address mapping storage |
| I16 | Anyone → `initialize` | Yes | None (only checks `!Initialized`) | Deployer ↔ Vault storage |
| I17 | Anyone → `extend_ttl` | Yes (TTL only) | None | Caller ↔ Vault storage TTL |
| I18 | Anyone → read-only methods (`config`, `vault_snapshot`, `fee_info`, `total_assets`, `convert_to_*`, `max_*`, `preview_*`, `is_paused`, `withdraw_status`, `queue_tail`, `cap_groups`, `is_migrating`, `query_asset`, `supply_queue`) | No | None | Caller ↔ Vault storage |
| I19 | Governance → `set_paused` | Yes | `require_auth(caller)` + governance check | Governance ↔ Vault storage, OZ Pausable |

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
| **Spoofing** | **Spoof.1** — Caller impersonates a privileged actor (curator, governance, allocator) by signing with a compromised key. Interactions: I5–I15, I19. |
| | **Spoof.2** — `initialize()` has no caller authentication; anyone can front-run deployment to set themselves as curator/governance. Interaction: I16. (`contract/mod.rs:1521–1553` — only checks `!has(Initialized)`, no `require_auth`.) |
| | **Spoof.3** — `governance_caller()` returns the curator's kernel address for RBAC purposes, meaning governance actions operate under the curator's identity within the RBAC layer. A compromised governance key gains curator-equivalent RBAC privileges. Interactions: I8–I12, I19. (`contract/mod.rs:2397–2401`.) |
| | **Spoof.4** — Adapter address in `allocate_supply`/`allocate_withdraw` is a caller-supplied parameter with no on-chain registry of vetted adapters. A compromised curator could route vault funds to an arbitrary contract. Interactions: I5, I6. (`contract/mod.rs:1648–1720` — adapter passed as `soroban_sdk::Address` parameter.) |
| **Tampering** | **Tamper.1** — Malicious or buggy adapter reports incorrect balances during refresh, causing `external_assets` accounting drift. Interactions: I5, I6, I7. |
| | **Tamper.2** — `register_address()` (curator-only) can remap kernel → Soroban address mappings, potentially redirecting fee payouts, share minting, or asset transfers to attacker-controlled addresses. Interaction: I15. (`contract/mod.rs:1979–1992`.) |
| | **Tamper.3** — `set_share_token()` (governance-only) can swap the share token contract. All subsequent mint/burn/transfer operations would target the new token, effectively disconnecting the vault from existing shareholders. Interaction: I11. (`contract/mod.rs:1797–1807`.) |
| | **Tamper.4** — `set_fees()` performs no upper-bound validation on fee WAD values. A compromised governance could set 100% performance + management fees, extracting all vault growth as fee shares. Interaction: I8. (`contract/mod.rs:1908–1949` — negative check only, no max cap.) |
| | **Tamper.5** — In `allocate_supply`, state update (`external_assets++`) executes inside the reentrancy guard, but the token transfer and `adapter.supply()` call happen after the guard releases. The adapter receives tokens and could re-enter the vault within the same atomic Soroban transaction. Interaction: I5. (`contract/mod.rs:1675–1681` — guard released before external calls.) |
| | **Tamper.6** — All persistent state uses postcard serialization. A deserialization bug or version mismatch — especially during upgrade/migrate — could corrupt state or brick the vault. Interaction: I14. (`storage/mod.rs:43–61`.) |
| **Repudiation** | **Repudiate.1** — Operators deny executing sensitive actions. Kernel state transitions emit `KernelEvent` envelopes, and `set_paused` emits OZ Pausable events. However, many privileged operations are auditable via transaction history. Interactions: I1–I7, I19. |
| | **Repudiate.2** — Approximately 10 privileged/governance operations emit **no contract events**: `set_curator`, `set_governance`, `set_share_token`, `set_fees`, `set_restrictions`, `allocate_supply`, `allocate_withdraw`, `upgrade`, `migrate`, `register_address`. While transaction senders are visible on-chain, lack of structured events significantly degrades monitoring, indexer coverage, and forensic reconstruction. Interactions: I5, I6, I8–I15. (Confirmed via grep — `publish_kernel_event` only called in `set_paused`, fee refresh, and effect execution.) |
| **Information Disclosure** | **Info.1** — No confidentiality assumptions exist for contract storage or events; this is expected on-chain transparency. All state is publicly readable. Interaction: I18. |
| | **Info.2** — `config()` exposes the governance contract address. `fee_info()` exposes fee anchor details and fee WAD values. This metadata could aid targeted social engineering or phishing attacks against key holders. Interaction: I18. (`contract/mod.rs:1994–2054`.) |
| | **Info.3** — `withdraw_status()` and `queue_tail()` expose withdrawal queue internals (next pending ID, active withdrawal op, current request ID). Observable queue state could enable front-running of large withdrawals or MEV-style extraction. Interaction: I18. (`contract/mod.rs:2073–2104`.) |
| **Denial of Service** | **DoS.1** — Withdrawal progression stalls if allocator workflows (allocate_withdraw, execute_withdraw) are not executed. Queue-dependent users cannot withdraw without operator action. Interactions: I3, I5, I6. |
| | **DoS.2** — `upgrade()` enables migration state via OpenZeppelin's `enable_migration`, which blocks all vault operations until `migrate()` completes. If the curator loses access or `migrate()` fails, the vault is **permanently bricked** with no rollback mechanism. Interactions: I13, I14. (`contract/mod.rs:2112–2150`.) |
| | **DoS.3** — Adapter revert in `supply()`/`withdraw()` causes the entire `allocate_supply`/`allocate_withdraw` transaction to fail. A buggy or malicious adapter blocks all allocation operations for that market. Interactions: I5, I6. (`contract/mod.rs:1678–1681, 1700–1701`.) |
| | **DoS.4** — Entire `VaultState` (including up to 1024 withdrawal queue entries) is serialized as a single postcard blob. Near-capacity writes have increasing CPU/memory resource costs, potentially exceeding Soroban transaction resource limits. Interactions: I1–I7. (`storage/mod.rs:119–124`; `kernel: MAX_PENDING = 1024`.) |
| | **DoS.5** — Persistent storage entries expire if `extend_ttl` is not called within ~6 months (3,110,400 ledgers at ~5s/ledger). Mitigated: `extend_ttl()` is permissionless (anyone can call it). Interaction: I17. (`storage/mod.rs:19–23`.) |
| **Elevation of Privilege** | **Elevation.1** — Role mapping or configuration errors could grant unintended powers. Reentrancy on mutating entrypoints could bypass authorization assumptions. Interactions: I1–I15. |
| | **Elevation.2** — No role separation exists in production. `load_vault_bootstrap()` creates `RbacConfig::with_curator(curator_kernel)` with empty guardian/allocator sets. The curator key is required for **all** privileged operations. The STRIDE implies separate guardian/allocator roles, but the Soroban implementation does not wire them up. Interactions: I5–I7, I19. (`contract/mod.rs:1425–1427`.) |
| | **Elevation.3** — SEP-41 `withdraw()` and `redeem()` bypass the withdrawal queue entirely via `atomic_withdraw_internal()`, directly debiting `idle_assets` and burning shares. While limited to idle assets and requiring the vault to be in `Idle` state, this is an alternate withdrawal path not subject to queue ordering or cooldown. Interaction: I4. (`fungible_vault.rs:185–231`.) |
| | **Elevation.4** — Governance controls curator appointment, fees, share token, caps, and restrictions. A single governance key compromise grants **total vault control** — the ability to drain via fees, redirect tokens via share token swap, or replace the curator. Interactions: I8–I12. |
| | **Elevation.5** — `upgrade()` requires curator (not governance) and allows replacing the entire contract WASM. This is the most powerful privilege in the system — an arbitrary code execution equivalent. Interaction: I13. (`contract/mod.rs:2112–2127`.) |
| | **Elevation.6** — The hard-coded `ESCROW_ADDRESS = [0u8; 32]` is mapped to the vault's own contract address during bootstrap. If any code path treats escrow as a distinct entity, it could cause address confusion or accounting drift. Interactions: I1–I3. (`contract/mod.rs:56, 352`.) |

---

## What are we going to do about it?

| Threat | Remediations |
|---|---|
| **Spoofing** | **Spoof.1.R.1** — `require_auth()` is called on all privileged entrypoints. Role-based authorization via `ActionKind` → `required_role()` mapping in curator-primitives enforces least-privilege. **Spoof.1.R.2** — Operational: use multisig or segregated keys for curator and governance. Hardware security modules for high-value deployments. |
| | **Spoof.2.R.1** — Deploy and initialize atomically (e.g., via a factory contract that deploys + calls `initialize` in a single transaction). **Spoof.2.R.2** — Consider adding an `admin` parameter to `initialize()` or requiring the deployer's auth to prevent front-running. **Spoof.2.R.3** — Accepted risk: Soroban contract deployment is typically atomic with initialization in practice, but this is a procedural control, not a technical one. |
| | **Spoof.3.R.1** — Accepted design decision: governance acting as curator in RBAC is intentional to simplify the privilege model. **Spoof.3.R.2** — Document this equivalence explicitly in operational security procedures. Ensure governance key security is at least as strong as curator key security. |
| | **Spoof.4.R.1** — Only curator/allocator can call `allocate_supply`/`allocate_withdraw`; adapter choice is within their trust model. **Spoof.4.R.2** — Consider adding an on-chain adapter allowlist (stored in persistent storage, governed by governance) so that allocator can only route to pre-approved adapters. **Spoof.4.R.3** — Monitor adapter interactions off-chain; alert on calls to unrecognized adapter addresses. |
| **Tampering** | **Tamper.1.R.1** — `allocate` and `refresh_markets` query adapters internally; the kernel validates state transitions. **Tamper.1.R.2** — Restrict adapters to vetted, audited contracts. Monitor external_assets drift vs adapter-reported totals. **Tamper.1.R.3** — Consider adding a maximum drift threshold that pauses the vault if adapter-reported assets deviate beyond tolerance. |
| | **Tamper.2.R.1** — `register_address` requires curator auth. Address mappings are persisted and used for all effect execution. **Tamper.2.R.2** — Consider restricting `register_address` to governance (not curator) since it controls token routing. **Tamper.2.R.3** — Emit an event on every `register_address` call for audit trail. **Tamper.2.R.4** — Consider making address mappings immutable once set (append-only). |
| | **Tamper.3.R.1** — `set_share_token` requires governance auth and verifies the address is a contract. **Tamper.3.R.2** — Consider adding a timelock on share token changes so users can exit before the change takes effect. **Tamper.3.R.3** — Emit an event on `set_share_token` for monitoring. |
| | **Tamper.4.R.1** — Add upper-bound validation on fee WAD values (e.g., max 50% performance fee, max 5% management fee). **Tamper.4.R.2** — Consider a timelock on fee changes so users can exit before higher fees take effect. **Tamper.4.R.3** — Emit an event on `set_fees` for monitoring. |
| | **Tamper.5.R.1** — Soroban transactions are atomic; if the adapter call fails, the entire transaction (including state update) reverts. **Tamper.5.R.2** — The reentrancy guard prevents re-entering guarded functions during external calls from `allocate_supply`/`allocate_withdraw`. `ensure_not_reentrant()` is checked on all entrypoints. **Tamper.5.R.3** — Accepted risk: adapter could call non-guarded vault read methods, but these are read-only. The state is consistent at the point of external call (external_assets updated, tokens not yet transferred in `allocate_supply`). |
| | **Tamper.6.R.1** — Storage decode validates blob deserialization, version key presence, version match, and compatibility before using persisted state. **Tamper.6.R.2** — Pin postcard crate version; audit serialization round-trip in CI. **Tamper.6.R.3** — Upgrade/migrate flow validates storage version compatibility before proceeding. |
| **Repudiation** | **Repudiate.1.R.1** — Actions require signed caller auth. Kernel state transitions emit `KernelEvent` envelopes via `publish_kernel_event`. OZ Pausable events emitted for pause/unpause. **Repudiate.1.R.2** — Maintain off-chain indexing/audit trails keyed by `op_id`, caller address, and timestamps. |
| | **Repudiate.2.R.1** — **Add `publish_kernel_event` or equivalent contract events to all privileged operations**: `set_curator`, `set_governance`, `set_share_token`, `set_fees`, `set_restrictions`, `allocate_supply`, `allocate_withdraw`, `upgrade`, `migrate`, `register_address`. **Repudiate.2.R.2** — Until events are added, rely on Soroban transaction-level observability (sender, function name, arguments visible in ledger history). **Repudiate.2.R.3** — Prioritize events for highest-impact operations: `upgrade`, `set_governance`, `set_curator`, `register_address`. |
| **Information Disclosure** | **Info.1.R.1** — Accepted: no confidentiality assumptions on-chain. This is expected behavior for public blockchain contracts. **Info.1.R.2** — Avoid introducing unnecessary detailed event payloads that could leak operational patterns. |
| | **Info.2.R.1** — Accepted risk: governance and fee configuration are operational parameters that need to be publicly verifiable. **Info.2.R.2** — Protect governance/curator keys through operational security (multisig, HSM), not obscurity. |
| | **Info.3.R.1** — Accepted risk: queue transparency is a feature for user trust. **Info.3.R.2** — Monitor for unusual withdrawal patterns that might indicate front-running. **Info.3.R.3** — Consider: withdrawal cooldown (`DEFAULT_COOLDOWN_NS`) already provides some protection against same-block front-running. |
| **Denial of Service** | **DoS.1.R.1** — Queue bounded by `MAX_PENDING = 1024`. Guarded state transitions prevent partial corruption. **DoS.1.R.2** — Operate redundant keeper bots to drive allocation and withdrawal workflows. Alert on queue staleness (e.g., if queue depth increases without withdrawals being processed). |
| | **DoS.2.R.1** — Ensure curator key is backed by multisig with redundant signers to prevent single-point-of-failure on upgrade. **DoS.2.R.2** — Test upgrade/migrate flow thoroughly on testnet before mainnet deployment. **DoS.2.R.3** — Consider adding a governance-callable emergency `cancel_migration` that reverts to the previous WASM if `migrate()` has not been called within a timeout. **DoS.2.R.4** — Document the upgrade procedure and key custody requirements. |
| | **DoS.3.R.1** — Restrict adapters to vetted, audited contracts. **DoS.3.R.2** — Implement an adapter allowlist so governance can remove a malfunctioning adapter. **DoS.3.R.3** — Consider wrapping adapter calls in a bounded resource budget to prevent resource exhaustion attacks. |
| | **DoS.4.R.1** — Keep withdrawal queue small relative to the 1024 maximum. Monitor queue depth. **DoS.4.R.2** — Consider splitting state into multiple storage entries if queue depth approaches resource limits. **DoS.4.R.3** — Watch Soroban network resource limit changes and adjust `MAX_PENDING` if needed. |
| | **DoS.5.R.1** — `extend_ttl()` is permissionless — anyone can call it. TTL threshold is ~30 days (518,400 ledgers), extension is to ~6 months (3,110,400 ledgers). **DoS.5.R.2** — Operate a keeper bot that calls `extend_ttl` periodically. **DoS.5.R.3** — `save_state` and `save_address` automatically extend TTL on writes, providing additional safety margin. |
| **Elevation of Privilege** | **Elevation.1.R.1** — Centralized action authorization via `ActionKind` → `required_role()` in curator-primitives. **Elevation.1.R.2** — Contract-level reentrancy guard (`with_reentrancy_guard` / `ensure_not_reentrant`) covers all public entrypoints. Mutating entrypoints use full lock/unlock cycle; read-only and config entrypoints use check-only. **Elevation.1.R.3** — Preserve strict role review on new entrypoints. |
| | **Elevation.2.R.1** — Accepted design decision for initial deployment: single curator key simplifies operations. **Elevation.2.R.2** — Wire up separate guardian and allocator addresses via storage-persisted RBAC config (read from storage in `load_vault_bootstrap`). **Elevation.2.R.3** — Add governance methods `set_guardian` and `set_allocator` to enable operational role separation. |
| | **Elevation.3.R.1** — Atomic withdrawals require vault to be in `Idle` state, sufficient `idle_assets`, and are capped to idle balance. **Elevation.3.R.2** — `refresh_fees_for_atomic()` is called before atomic withdrawals to ensure fees are current. **Elevation.3.R.3** — Document the atomic withdrawal path clearly in user-facing documentation as an intentional feature for immediate withdrawal from idle assets. |
| | **Elevation.4.R.1** — Require governance to be a multisig or DAO contract (enforced: `require_contract_address` in `set_governance`). **Elevation.4.R.2** — Consider timelocks on high-impact governance actions (curator change, fee change, share token change). **Elevation.4.R.3** — Monitor all governance transactions with alerting. |
| | **Elevation.5.R.1** — `upgrade()` requires curator auth. The two-step pattern (upgrade → migrate) provides a review window. **Elevation.5.R.2** — Consider requiring governance approval for upgrades instead of or in addition to curator. **Elevation.5.R.3** — Use a timelock proxy for the curator address to provide users with exit time before WASM changes take effect. |
| | **Elevation.6.R.1** — `ESCROW_ADDRESS = [0u8; 32]` is mapped to the vault's own contract address (`contract/mod.rs:352`), ensuring escrow operations (share transfers during withdrawal) route correctly. **Elevation.6.R.2** — The escrow mapping is set during vault bootstrap and is consistent across all invocations. No additional remediation needed. |

---

## Soroban-Specific Notes

- **Reentrancy model**: Soroban does not have Ethereum-style reentrancy (no fallback functions), but cross-contract calls within a single transaction are synchronous. The vault's reentrancy guard uses instance storage (`VaultDataKey::ReentrancyLock`) to prevent re-entrance during guarded blocks. External calls in `allocate_supply` and `allocate_withdraw` happen outside the guard, but `ensure_not_reentrant` is checked on all entrypoints.
- **Storage decode**: Validates blob deserialization, version key presence, version match, and compatibility before using persisted state (`storage/mod.rs:326–347`).
- **Storage TTL**: Persistent entries must be maintained via `extend_ttl`. Default threshold is ~30 days; extension target is ~6 months. Automatic extension on state writes provides additional safety.
- **Resource limits**: Soroban network-level constraints on CPU, memory, ledger footprint, and transaction size. Writes fail atomically when limits are exceeded — no partial state corruption.
- **Auth model**: Soroban `require_auth()` is invocation-scoped. The vault uses it for caller identity, then delegates to RBAC for role checks. `require_auth` on the caller is distinct from contract-level auth (the vault contract itself authorizes token operations as the contract address).
- **Kernel architecture**: The `#[contractimpl]` block provides the Soroban on-chain API; `CuratorVault` is chain-agnostic and reuses `curator-primitives` for auth/rbac/policy and `templar-vault-kernel` for state machine/transitions/effects/fee math.
- **Production flows**: `allocate_supply` and `allocate_withdraw` directly manage the kernel state machine (begin/finish allocation) within the reentrancy-guarded block, then perform external calls. `refresh_markets` drives the kernel refresh flow entirely within the guard.
- **Removed methods**: `sync_external_assets`, `verify_external_assets_against_adapter`, `manual_reconcile`, `abort_allocating`, `abort_refreshing`, `abort_withdrawing`, `recover`, `settle_payout`, `refresh_fees`, and market lock methods (`acquire_market_lock`, `release_market_lock`, `is_market_locked`) no longer exist in the Soroban implementation.

---

## Did we do a good job?

### Checklist

- [x] Has the data flow diagram been referenced since it was created?
  - Yes — all threats are mapped to specific interactions (I1–I19) in the dataflow.
- [x] Did the STRIDE model uncover new design issues or concerns?
  - Yes — several significant findings:
    - **DoS.2**: Upgrade/migrate can permanently brick the vault with no rollback.
    - **Repudiate.2**: ~10 privileged operations emit no events.
    - **Elevation.2**: No actual role separation in production despite RBAC infrastructure.
    - **Tamper.4**: No upper bound on fee values.
    - **Spoof.2**: `initialize()` lacks caller authentication.
- [ ] Did the treatments adequately address the issues identified?
  - Partially. Several remediations are "accepted risk" or "consider adding." Highest priority implementation items:
    1. Add events to all privileged operations (Repudiate.2.R.1).
    2. Add fee cap validation (Tamper.4.R.1).
    3. Add adapter allowlist (Spoof.4.R.2).
    4. Wire up role separation (Elevation.2.R.2).
    5. Add upgrade rollback mechanism (DoS.2.R.3).
- [ ] Have additional issues been found after the threat model?
  - To be updated after external audit.

### Severity Summary

| Severity | Count | Key Items |
|---|---|---|
| **Critical** | 2 | DoS.2 (migration brick), Elevation.5 (curator WASM upgrade) |
| **High** | 5 | Spoof.2 (init auth), Tamper.2 (address remap), Tamper.3 (share token swap), Elevation.2 (no role separation), Elevation.4 (governance single point of failure) |
| **Medium-High** | 1 | Repudiate.2 (missing events on ~10 operations) |
| **Medium** | 7 | Spoof.3, Spoof.4, Tamper.4, Tamper.5, DoS.3, DoS.4, Elevation.3 |
| **Low** | 4 | Tamper.6, Info.2, Info.3, DoS.5 |
| **Accepted** | 2 | Info.1 (on-chain transparency), Elevation.6 (escrow address) |

### Review Cadence

- Revisit this model when adding new privileged actions, new adapters, or storage schema changes.
- Revisit after significant Soroban SDK or network resource limit changes.
- Revisit after any upgrade/migrate deployment.
- Revisit after external security audit findings.
