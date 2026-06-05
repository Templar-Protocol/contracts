# Soroban Vault Runtime

This crate hosts the Soroban executor/runtime for the Templar vault kernel.

## Runtime Architecture

This crate is the Soroban executor layer for the shared vault kernel. It owns:

- Soroban entrypoints and contract wiring
- address mapping from Soroban addresses to kernel addresses
- persistent state storage and migration gating
- RBAC/auth enforcement via `require_auth()` + shared `ActionKind`
- execution of `KernelEffect`s against Soroban token contracts

Governance timelock/orchestration lives in the dedicated `contract/vault/soroban/governance`
contract. The runtime still applies canonical governance state changes. Vault-bound governance
actions cross the contract boundary via `execute_governance(env, caller, payload)`, where the
payload carries a `GovernanceCommand`. `SetTimelock` and `Other` actions stay local to the
governance contract. The generic `execute(payload)` path remains for user flows and the
`CancelMigration` recovery command. Runtime support for `VIRTUAL_OFFSETS` remains in the retained
config subset and has no shipped governance-contract submitter; allocator and adapter-allowlist
changes are routed through `execute_governance`.

```mermaid
graph TB
    subgraph Contract["contract/vault/soroban"]
        ENTRY["SorobanVaultContract\nentrypoints"]
        CVAULT["CuratorVault<S, A, E>\nload state, authorize, apply kernel action"]
        AUTH["RbacAuth / Soroban auth\nrequire_auth() + ActionKind policy"]
        STORAGE["SorobanStorage\nversioned state blob\nTTL extension + migrate gate"]
        ADDR["kernel_address_from_sdk()\nSHA256(domain || strkey)"]
EFFECTS["SorobanEffectInterpreter\nshare + asset token effects\ntyped kernel events"]

        ENTRY --> CVAULT
        CVAULT --> AUTH
        CVAULT --> STORAGE
        CVAULT --> ADDR
        CVAULT --> EFFECTS
    end

    KERNEL["templar-vault-kernel\npure state machine"] --> CVAULT
    PRIMS["templar-curator-primitives\npolicy + RBAC classes"] --> AUTH
    PRIMS --> CVAULT
    EFFECTS --> SHARE["SEP-41 share token"]
    EFFECTS --> ASSET["underlying asset token"]
```

### Main Execution Loop

```mermaid
sequenceDiagram
    actor Caller
    participant Entry as contract entrypoint
    participant Vault as CuratorVault
    participant Kernel as apply_action()
    participant Interp as SorobanEffectInterpreter
    participant Storage as SorobanStorage

    Caller->>Entry: invoke deposit / atomic withdraw / queued action
    Entry->>Entry: require_auth() for deposit/request/execute callers
    Entry->>Vault: load bootstrap + map addresses
    Note over Entry,Vault: atomic_withdraw_impl / atomic_redeem_impl delegate operator auth to the vault/share-token path
    Vault->>Storage: load versioned state / config
    Vault->>Vault: authorize(ActionKind, caller)
    Vault->>Kernel: apply_action(...)
    Kernel-->>Vault: new state + KernelEffect[]
    Vault->>Interp: execute_effects(...)
    Vault->>Storage: save state / policy / mappings
    Entry-->>Caller: return result
```

### Fee Anchor And Idle Balance Accounting

The Soroban vault treats unsolicited underlying transfers as idle assets for existing
shareholders, not as profit that the next depositor can capture. Read-only conversion and preview
helpers first compare the persisted `idle_assets` value with the asset token balance held by the
vault and simulate the reconciled state before quoting shares or assets.

State-changing paths that depend on current share pricing use the same lazy reconciliation rule
before executing kernel actions. `DepositWithMin`, `RefreshFees`, and `ResyncIdleBalance` all read
the live asset token balance, update `idle_assets`, recompute `total_assets`, and reset the
`fee_anchor` to the reconciled total at the current ledger timestamp. This keeps direct transfers,
fee refreshes, and later deposits on one accounting baseline without requiring a separate keeper
transaction before every user deposit.
When fees are active, deposits first crystallize any elapsed management/performance fees before
the post-deposit anchor is written, so deposit principal cannot erase already accrued fees.

### Governance Control-Plane Boundary

- The governance contract owns proposal submission, timelocks, approval/revocation, and abdication.
- The configured Sentinel is a separate emergency role holder. The governance contract is not
  implicitly treated as Sentinel and should not be granted `Role::Sentinel` just to make governance
  proposals work.
- The runtime remains the canonical owner of applied vault config/policy state.
- Vault-bound governance actions cross the boundary through a single bridge:
  `execute_governance(env, caller, payload)`. The payload is a `GovernanceCommand` that the
  runtime decodes and dispatches to the corresponding internal config/policy/state helpers.
- Emergency pause and restriction tightening are immediate Sentinel actions. Unpause and
  relaxing/removing restrictions are governance actions and must pass through the configured
  timelock before the runtime applies them.
- Skim recipient changes and skim execution are governance actions and must pass through the
  configured `Skim` timelock before the runtime applies them.
- `execute(payload)` remains for user flows and the `CancelMigration` recovery command.
  Allocator and adapter-allowlist governance changes use `execute_governance`; `VIRTUAL_OFFSETS`
  remains a runtime governance-config kind without a shipped governance-contract submitter.

### Soroban-Specific Withdrawal Path

The vault intentionally exposes two withdrawal modes:

- `withdraw` / `redeem` are ERC-4626-style atomic exits from idle liquidity only. They never
  enqueue work, never pull from adapters, and fail if the requested assets exceed `idle_assets`.
- `request_withdraw` is the async path for positions that may require allocator/keeper work.
  `execute_withdraw` advances the queue only when the head request is cooled down and fully
  covered by idle assets; otherwise it fails atomically and leaves the request queued.

```mermaid
sequenceDiagram
    actor User
    actor Keeper
    participant Contract as SorobanVaultContract
    participant Vault as CuratorVault
    participant Kernel as apply_action()
    participant Share as share token
    participant Asset as asset token

    User->>Contract: request_withdraw(owner, receiver, shares, min_assets_out)
    Contract->>Vault: request_withdraw(...)
    Vault->>Kernel: RequestWithdraw
    Kernel-->>Vault: queue update + escrow-share transfer effect
    Vault->>Share: transfer owner shares into escrow
    Contract-->>User: request_id

    Keeper->>Contract: execute_withdraw(caller)
    Contract->>Vault: execute_withdraw(...)
    Vault->>Kernel: ExecuteWithdraw
    alt queue head is cooled down and fully idle-funded
        Vault->>Vault: complete_withdrawal_from_idle()
        Vault->>Asset: transfer assets to receiver
        Vault->>Kernel: SettlePayout
        Vault->>Share: burn escrow shares / refund remainder
    else liquidity must be freed first
        Note over Vault: transaction fails atomically; no partial payout is made\nallocator path must free liquidity before retry
    end
    Contract-->>Keeper: ok
```

The typed `execute_withdraw` entrypoint keeps returning `Result<(), _>` for
the stable contract ABI. The generic `execute(payload)` command path returns
`VaultCommandResult::ExecuteWithdrawStatus` for
`VaultCommand::ExecuteWithdraw`, with:

- `op_state_before` and `op_state_after`: kernel operation-state codes
  (`0 = Idle`, `1 = Allocating`, `2 = Withdrawing`, `3 = Refreshing`,
  `4 = Payout`).
- `assets_transferred`: assets paid to receivers during this command.
- `events_emitted`: kernel/runtime events emitted while processing the command.

Keepers should treat a failed `ExecuteWithdraw` with the kernel low-liquidity
error as a signal to free market liquidity before retrying. A successful
command with `assets_transferred == 0` and a non-idle `op_state_after` should
be alerted as an unexpected no-progress withdrawal state. The A-002 fix is
intended to reject that zero-progress transition before it is persisted, but the
structured result keeps automation from relying on a bare `Unit` success.

If withdrawal execution enters `Withdrawing` and cannot progress because idle
liquidity remains below the kernel minimum, an allocator-emergency actor can
submit `VaultCommand::AbortWithdrawing { caller, op_id }` through `execute`.
The command reuses the kernel recovery transition: it validates the active
operation id and queue head, refunds escrowed shares, emits the kernel
`WithdrawalStopped` event, dequeues the request, and returns the vault to
`Idle`.

`AbortWithdrawing` uses the `ActionKind::AbortWithdrawing` authorization class.
In the default Soroban RBAC policy this is available to allocator-emergency
operators (`allocator`, `sentinel`, and `curator`), not ordinary users. The
transition restores any `Withdrawing.collected` amount to idle accounting before
refunding escrowed shares, dequeuing the head request, and returning to `Idle`.

## Prerequisites

### Stellar CLI

The Stellar testnet is on the protocol 26 upgrade path, so use `stellar-cli`
v26. The workspace toolchain is **Rust 1.92** because the current Stellar CLI
and OpenZeppelin Stellar crates require it.

**With devenv** (handles it automatically):

```
devenv shell
```

On first entry, devenv installs Rust 1.92 and builds `stellar-cli` v26.
Subsequent entries skip this (~3-4 min first time).

**Without devenv:**

```
./scripts/install-stellar-cli.sh
```

The script installs Rust 1.92 (via rustup) and builds the CLI. The optimized
contract build path requires the CLI's default native integrations, so Linux
hosts need dbus development headers:

| OS | Packages |
|----|----------|
| Arch/CachyOS | `pacman -S dbus systemd pkg-config` |
| Ubuntu/Debian | `apt install libdbus-1-dev libudev-dev pkg-config` |
| Fedora | `dnf install dbus-devel systemd-devel pkgconf-pkg-config` |
| macOS | (none — dbus is not needed) |

### Nix / devenv note

The nix environment isolates libraries from the host.  If `stellar` segfaults or
reports `libdbus-1.so.3: cannot open`, ensure `dbus` is in the devenv
`LD_LIBRARY_PATH` (already configured in `devenv.nix`).

## Quick start (testnet)

Use recipes from [contract/vault/soroban/justfile](./justfile):

- `setup`
- `deploy-all`
- `demo-deposit`
- `demo-withdraw`

From repo root: `just -f contract/vault/soroban/justfile <recipe>`.

The build step compiles the runtime, governance, and share-token WASMs, runs the Stellar optimizer,
and strips runtime contractspec metadata into a deploy artifact. The runtime deploy artifact is
budgeted separately from the optimizer output because it is the artifact used for size gating.

## Blend Adapter

Blend integration lives in the dedicated crate `contract/vault/soroban/blend-adapter`.
Use recipes in [contract/vault/soroban/justfile](./justfile):

- `just build-blend-adapter`
- `just deploy-blend-adapter <BLEND_POOL_ADDRESS>`
- `just deploy-all-with-blend <BLEND_POOL_ADDRESS>`

After deployment, register the adapter as a vault market before allocation.

## Deployment Artifact

The Soroban justfile builds two runtime artifacts:

- `templar_soroban_runtime.wasm` with Stellar optimizer output and contractspec metadata
- `templar_soroban_runtime.deploy.wasm` with contractspec metadata stripped for deployment and
  size-budget checks

Useful commands:

- `wasm-path` -> default runtime artifact, currently `templar_soroban_runtime.wasm`
- `optimized-wasm-path` -> explicit optimized artifact path
- `deploy-wasm-path` -> contractspec-stripped deploy artifact path used for deployment and size verification
- `size-budget-check` -> verifies `templar_soroban_runtime.deploy.wasm <= 131072` bytes

## State Size and Operational Limits

- Soroban enforces per-entry and per-transaction resource limits. Current network values are documented by Stellar: https://developers.stellar.org/docs/networks/resource-limits-fees
- Vault runtime state is persisted as a compact versioned `StateBlob` header plus domain-paged withdrawal queue entries. Each `wqpage` stores up to 128 pending withdrawals, so the queue can use the kernel `MAX_PENDING = 1024` cap without coupling the whole queue to one 64 KiB storage entry.
- Restrictions and policy blobs use the generic blob-paging transport. Small payloads are stored inline; larger payloads are split into bounded 32 KiB pages.
- One contract invocation is still bounded by Soroban transaction resource limits. Very large sanctions-list style updates should use a batched governance/update flow instead of one giant replacement payload.
- In-flight operation plans (`Allocating.plan`, `Refreshing.plan`) are expected to remain small under allocator policy; if that assumption changes, the paged blob transport protects storage entry size but not per-transaction CPU/write-byte budgets.
- Persistent storage blobs carry a compact `TVS` version header. Decoders reject pre-header bytes and unsupported versions; schema upgrades should add explicit per-version decode/migration dispatch before any layout change.

## Practical Risk Model

- TVL growth by itself does not significantly increase serialized state size.
- Risk comes from queue backlog plus unusually large in-flight plans.
- If state would exceed Soroban storage write limits, storage save paths return a typed runtime storage error before the host storage write.

## Runtime TTL and Keeper Responsibility

Soroban contract data is not permanent. Vault deployments must include an ops/keeper job that
periodically calls the permissionless `VaultCommand::ExtendTtl` path through `execute(payload)`.
Do not rely on a curator remembering to do this manually.

The runtime TTL keeper renews the vault contract's own storage, not every external contract or
every user-owned entry elsewhere. In particular, one successful vault runtime TTL call renews:

- runtime instance storage;
- the canonical `StateBlob`, including any paged blob entries;
- policy and restriction blobs: `PolicyLocks`, `PolicySupplyQueue`, `PolicyMarkets`,
  `PolicyPrincipals`, `PolicyCapGroups`, and `Restrictions`, including their paged entries;
- withdrawal queue pages currently referenced by the state header;
- runtime address-book mappings referenced by pending withdrawals, active withdrawal/payout
  operation state, and fee recipients.

Normal state-saving vault paths also refresh runtime storage TTL, but a quiet vault can still
approach archival. Schedule the keeper on cadence well before the TTL threshold. Related contracts
need their own TTL maintenance: share token, governance, adapters, proxy contracts, and oracle
contracts do not inherit the vault runtime's TTL renewal. Vault governance and the 4626 proxy
each have their own permissionless `extend_ttl()` entrypoint for config/proposal-state
maintenance.

## Parity Tests

Parity tests check behavioral equivalence across the shared kernel and chain executors (NEAR and Soroban). They ensure state transitions, accounting behavior, and invariants stay aligned as implementations evolve.

- Guide: `contract/vault/README.md#parity-tests`

## Threat Model

- Soroban-specific STRIDE: `contract/vault/soroban/STRIDE.md`

## Share Token Policy

- Soroban share-token transfers are user-authorized (`from.require_auth()`).
- The vault can still transfer shares for internal flows (escrow/payout effects).

## Share Token TTL and Archival Recovery

- Share-token instance storage is refreshed by every public share-token entrypoint, including SEP-41 read-only methods (`total_supply`, `balance`, `allowance`, `decimals`, `name`, and `symbol`) and the custom `admin` / `vault` getters.
- The admin-only `extend_ttl(caller)` entrypoint is the explicit keeper path for proactive instance maintenance. Operators should schedule it well before the instance reaches the TTL threshold; if the instance is archived, restore the contract instance through the Stellar/Soroban archival restore flow first, then call `extend_ttl` as the configured admin.
- Per-holder balances are persistent entries owned by the upstream `stellar-tokens` implementation. Balance reads and balance-changing writes refresh the specific holder balance that is touched; the share token intentionally does not maintain an enumerable holder index or perform unbounded global balance refreshes from `extend_ttl`.
- Allowances are temporary entries bounded by their explicit `live_until_ledger`. They are not extended beyond that caller-selected expiry by the share-token keeper path; owners should renew approvals when continued delegated spending is desired.
