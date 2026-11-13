# Threat model: Templar Protocol contracts

Templar is an overcollateralized lending protocol on NEAR and Stellar (Soroban). This repository holds the contracts that custody user funds, the price oracles those contracts read, and the off-chain services that operate them. A contract or oracle bug can mean direct loss of user funds. The scanned branch is `dev`. Mainnet releases are tagged from it, so `dev` can run ahead of what is deployed.

Read these first. They are the maintainers' own map of the code and its invariants:

- `AGENTS.md`, section "High-Impact Areas": per component, what to read, what to watch for, and the minimum tests.
- `docs/src/security-overview.md` and `docs/src/governance.md`: the trust model and the roles.
- `contract/vault/soroban/STRIDE.md`: the Stellar vault threat model.
- `contract/proxy-oracle/soroban/AUDIT.md`: the Soroban proxy-oracle audit boundary, invariants and non-goals.
- `audits/README.md`: design decisions auditors asked about, and known issues.
- `docs/src/addresses.md` and `deployments/`: what is live on mainnet.

## What this project does and where untrusted input enters

Components:

- **Markets** (`contract/market`, logic in `common/src/market`, `common/src/borrow.rs`). One collateral asset and one borrow asset per contract. Supply, borrow, repay, collateral deposit and withdrawal, liquidation, interest, fees and static yield. Markets are immutable: no owner, no upgrade, no pause, no parameter setters.
- **Proxy oracles** (`contract/proxy-oracle`). A shared kernel (`kernel`, `governance-kernel`) with NEAR and Soroban runtimes. NEAR uses weighted medians and Priority; Soroban uses equal-weight MedianLow/MedianHigh and ordered Priority. Both apply freshness filters, circuit breakers, fail-closed reads and timelocked governance. Soroban also has a SEP-40 adapter, a Pyth Lazer source and a batcher.
- **Price sources**: the Pyth Lazer verifier and adapter (`contract/pyth-lazer`), the RedStone adapter (`contract/redstone-adapter`), and the LST oracle (`contract/proxy-oracle/near/lst-contract`).
- **Curated vaults** (`contract/vault`). A shared kernel (`kernel`, `curator-primitives`), a NEAR runtime (`near`) and a Soroban runtime (`soroban`: governance, share token, Blend adapter, custodial adapter, ERC-4626-style proxy, curator proxy). Curated vaults are live on Stellar.
- **Registry** (`contract/registry`): market version reservation, deployment and two-step finalization.
- **Universal account** (`contract/universal-account`, `universal-account`): signature verification, nonces, transaction execution and migration.
- **Patch-state** (`contract/patch-state`): a vendored state-manipulation contract used in the reviewed storage-patch process.
- **Off-chain services** (`service/*`, `gateway/*`): the relayer (delegated actions), the liquidator, the funding bridge, the accumulator, the market monitor and the gateway (NEAR read/write planning, signing, idempotency).

Treat all of these as attacker-controlled:

- Every public or payable entry point of every contract, called by any account with any arguments, attached deposit and gas.
- Token callbacks: `ft_on_transfer` / `mt_on_transfer` and their `msg` payloads. A call that claims to come from a market's configured token can come from any account.
- Cross-contract callback results. A promise can fail, return unexpected data, or run out of gas. On NEAR, state can change between a call and its callback: other receipts run in between.
- Oracle payloads: signed Pyth Lazer updates, RedStone payloads, and the responses of source oracles, including stale, future-dated, replayed, reordered, mis-scaled, negative or zero prices, and missing exponents.
- On Soroban, any `Address` passed in, any authorization entries, and any contract a caller names (for example the batcher's targets).
- For the off-chain services: every request to the relayer and gateway HTTP/JSON-RPC interfaces, and all data read from chain RPC.

Trusted:

- The 2-of-3 DAO multisig, proxy-oracle and vault governance admins acting through their timelocks, and retained deployer keys. An issue that needs one of these to act maliciously is out of scope.
- The token contracts and oracle providers a deployment configures, as configured. A single oracle provider that reports a wrong price within the circuit-breaker bounds is a documented residual risk. Report it only when the code fails to apply a check it claims to apply.

Bounded roles. These are in scope when they can act outside their documented limits:

- Vault curator, allocator, guardian and Sentinel roles (see `docs/src/curator-guide.md` and `contract/vault/soroban/STRIDE.md`). For example, a role that can skip a timelock, relax a restriction it may only tighten, or move funds outside the caps.
- The proxy-oracle `ManualTripper` and breaker roles.
- Keepers and any permissionless caller of refresh, TTL or keeper-routed paths.
- The relayer's allowed-method and nonce rules for delegated actions.

## Components that matter most / least

Most important, in this order:

1. Market accounting and liquidation: `common/src/market`, `common/src/borrow.rs`, `common/src/supply.rs`, `common/src/fee.rs`, `common/src/interest_rate_strategy.rs`, `common/src/withdrawal_queue.rs`, `common/src/snapshot.rs`, `contract/market/src`. Watch for rounding direction, in-flight accounting across async borrow and withdraw flows, storage charging and refunds, and `storage_unregister`.
2. The price path: both proxy-oracle runtimes and the kernel, the Pyth Lazer verifier and source contracts, the RedStone adapter, the LST oracle, and the price types in `common/src/oracle`. Watch for freshness, replay, signer and trust checks, decimal rescaling, confidence-interval use, and breaker bypass.
3. Vaults: `contract/vault/kernel`, `contract/vault/soroban/**`, `contract/vault/near`. Watch for share math, escrow and withdrawal queues, `OpState` transitions, callback ordering, fee accrual, and adapter accounting against Blend.
4. Authorization and upgrades: proxy-oracle and vault governance contracts, the registry, the universal account (replay, signature payloads, migration).
5. The relayer and gateway, where an outside request can make the service sign or pay for something it should not.

Lower priority, but in scope: the liquidator, the funding bridge, the accumulator, the market monitor, `tools/manager`, `tools/soroban-vault-cli`, `client/`.

Out of scope as targets (use them as tools): `mock/`, `test-utils/`, `gateway/testing`, `fuzz/`, Kani harnesses, `script/`, `docs/`, and test-only fixtures such as `contract/universal-account/tests/migration/0_4_0.wasm`.

## How to exercise it

The maintainers' rule: run tests only through the `just` recipes, never `cargo test` or `cargo nextest` directly. The image is offline and ready for both gates.

- Start Postgres first. Some tests use `#[sqlx::test]`: `templar-postgres-up`. `DATABASE_URL` is already set.
- Fast (non-node) gate: `just test-fast`, or narrow it, for example `just test-fast -p templar-common --lib`. The released Wasms that migration tests deploy are already in `$TEMPLAR_ARTIFACT_CACHE`.
- Node-backed gate on a local neard sandbox: `just test-sandbox --stale --test-threads 1 -p templar-market-contract`. `--stale` reuses the Wasms already built in `target/near`. The neard binary is at `$NEAR_SANDBOX_BIN_PATH`. Use one test thread: each thread starts its own neard node. If you change contract code, drop `--stale` so the Wasms rebuild (cargo-near is installed).
- Soroban contracts are tested natively with `soroban-sdk` test utilities, through `just test-fast`. For example: `just test-fast -p templar-proxy-oracle-soroban-integration-tests`. The Soroban Wasm toolchain and the stellar CLI are not installed.
- Write a proof of concept as a new test next to the existing ones: a unit test in the crate, an integration test in the crate's `tests/`, or a sandbox test that uses the `SandboxHarness` in `gateway/testing`. Name a node-backed test inside `src/` `requires_sandbox_*`. Otherwise the fast gate selects it, and the fast gate runs no neard.
- `fuzz/` holds libFuzzer targets and `kani` proofs exist for the vault kernel. cargo-fuzz and Kani are not installed, so treat them as documentation of the intended invariants.

## How you rate severity

- **Critical**: an unprivileged account can steal or permanently freeze user funds (supply, collateral, vault deposits or shares), or can borrow against collateral it does not have. A forged, replayed or stale price is accepted and makes such a loss possible. Unauthorized upgrade, role grant, or governance execution. A universal-account signature or replay bypass.
- **High**: an unprivileged account can create bad debt or protocol insolvency through accounting or rounding, or can block repay, withdraw or liquidation for others at low cost. A bounded role acts outside its documented limits. A remote request makes the relayer or gateway sign, spend or execute something outside its allowed rules.
- **Medium**: value leaks that are small or bounded per transaction but repeatable. Griefing that costs the victim more than the attacker but loses no funds. A circuit breaker or freshness filter that fails open in a reachable edge case without a demonstrated loss.
- **Low**: gas or storage inefficiency, wrong events or logs, missing input validation with no security effect, and issues that need a trusted role to misbehave.

Rate on demonstrated impact. A finding with a passing proof-of-concept test is worth more than a theoretical one. Rate a finding in code that is not yet released at the same severity, and say that it is not yet deployed.

## How to format reports

- One report per root cause. The same bug in a shared kernel that both the NEAR and the Soroban runtimes use is one report.
- Give the file and function, the attacker's steps, and the impact in funds or in blocked operations.
- Include a proof of concept as a test that runs with the `just` commands above, and the exact command that runs it.
- Include a minimal patch that fixes the root cause, plus the test that now passes. Keep the patch small. Do not refactor.
- Say whether the affected contract version is live, using `docs/src/addresses.md`, the release catalog in `contract/artifacts/releases/`, and `deployments/`.

## Anything to leave alone

- Known issues and design decisions recorded in `audits/README.md` and in each component's `AUDIT.md`, `STRIDE.md` or README non-goals. One example: a borrow whose token transfer fails still pays fees. That is intentional.
- Bugs that live only in third-party crates, the Rust toolchain, nearcore or the Soroban host. The pinned git forks (`Templar-Protocol/pyth-lazer-public` and `Templar-Protocol/stellar-contracts`) are dependencies, so a bug inside them is out of scope here. A bug in how this repository calls them is in scope.
- Markets under the `templar-alpha.near` registry and the `deployments/alpha` and `liqtest-*` configurations. These are pre-release and liquidation-test markets, not production markets.
- Risk-parameter choices in `deployments/` and `docs/src/risk-parameters.md`, unless the code fails to enforce a parameter.
- Clippy lints, formatting, documentation typos, and gas use without a security effect.
