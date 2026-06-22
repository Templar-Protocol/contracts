# Agent Guide

This repository is a Rust workspace for Templar Protocol smart contracts, services, test helpers, and CLI tools.

## Repository Layout

- `common`: shared protocol logic used by contracts and tests
- `contract`: deployable smart contracts (`market`, `registry`, `vault`, `universal-account`, `lst-oracle`)
- `service`: standalone executables and off-chain services
- `tools`: operator and developer CLIs
- `test-utils`: shared test harness utilities
- `mock`: mock contracts used in tests
- `universal-account`: shared account crate
- `fuzz`: fuzz targets

## High-Impact Areas

Use this section as an execution checklist: read the local docs first, preserve the listed invariants, and run at least the listed checks.

- `common` (`templar-common`)
  Read/inspect: `common/src/borrow.rs`, `common/src/market/impl.rs`, `common/src/event.rs`, `common/src/vault/mod.rs`.
  Why it matters: this crate is the protocol source of truth. Accounting, event schemas, oracle types, borrow/supply logic, and shared vault interfaces all live here.
  Watch for: arithmetic edge cases, state-transition semantics, serialization changes, and event-schema drift. A small change here can silently alter multiple contract APIs.
  Minimum verification: `cargo test -p templar-common --lib -- --nocapture`.
- `contract/market` (`templar-market-contract`)
  Read/inspect: `contract/market/src/lib.rs`, `contract/market/src/impl_market_external.rs`, plus the corresponding logic in `templar_common::market`.
  Why it matters: the deployable contract is thin, but it adds NEP-145 storage behavior and wraps asynchronous borrow/collateral/withdraw flows around shared market logic.
  Watch for: storage charging/refunds, `storage_unregister` implications, force-unregister behavior, cross-contract finalize paths, and in-flight accounting.
  Minimum verification: `cargo test -p templar-common --lib -- --nocapture`; if contract entrypoints or callbacks changed, also run `cargo test -p templar-market-contract -- --nocapture`.
- `contract/vault` (`templar-vault-contract`)
  Read first: `contract/vault/README.md` and `contract/vault/STRIDE.md`.
  Read/inspect: `contract/vault/src/lib.rs`, `contract/vault/src/impl_callbacks.rs`, `contract/vault/src/governance.rs`, `common/src/vault/*`.
  Why it matters: this is the most complex state machine in the repository and the highest-risk place for async accounting bugs.
  Watch for: `OpState` transitions, escrow accounting, keeper-routed withdrawals, callback ordering, idle-balance resync, fee accrual, and reconciliation after partial failures.
  Minimum verification: `cargo test -p templar-vault-contract -- --nocapture`.
- `contract/registry` (`templar-registry-contract`)
  Read/inspect: `contract/registry/src/lib.rs`.
  Why it matters: this is a deployment/orchestration contract, not just a map of version keys.
  Watch for: the distinction between `Reserved` and `Deployed`, deployment finalization paths, soft deletion of version code, and failure cleanup after partial deploy flows.
  Minimum verification: `cargo test -p templar-registry-contract -- --nocapture`.
- `contract/universal-account` (`templar-universal-account-contract`) and `universal-account` (`templar-universal-account`)
  Read first: `contract/universal-account/README.md`.
  Read/inspect: `contract/universal-account/src/lib.rs`, `contract/universal-account/src/impl_migrate.rs`, and the shared transaction/signature code in `universal-account`.
  Why it matters: these crates define authentication, signature verification, nonce progression, transaction execution, and migration behavior.
  Watch for: replay protection, signing payload compatibility, migration compatibility, supported signature schemes, and any wire-format changes.
  Minimum verification: `cargo test -p templar-universal-account-contract -- --nocapture`.
- `service/relayer` (`templar-relayer`)
  Read first: `service/relayer/README.md`.
  Why it matters: this service is an operational security boundary for delegated actions and universal-account flows.
  Watch for: allowed-method changes, nonce handling, gas settings, SQL query changes, storage-deposit behavior, and universal-account deployment/execution integration.
  Minimum verification: run the narrowest relevant `cargo test -p templar-relayer ...`; if SQL changes, update prepared queries as documented in the README.
- `gateway/*` (the Templar gateway: `templar-gateway-*`)
  Read first: `gateway/README.md` (RPC naming) and `gateway/METHODS.md` (the generated catalog of every method: kind, input → output, summary).
  Why it matters: the gateway is the single standardized implementation of NEAR reads and writes (planning, signing, multi-step finalization, idempotency/replay). Rust consumers integrate it in-process via `templar-gateway-client`; the JSON-RPC service is for non-Rust clients.
  Watch for: when migrating a consumer onto the gateway, diff it against the original operation-by-operation and map each call by **semantics, not name**. Prefer domain/standard-agnostic methods over low-level ones (e.g. `token.transfer`, which dispatches NEP-141 vs NEP-245, over `ft.transfer` — an asset may be a multi-token). If the gateway lacks a method a consumer needs, add it to the gateway rather than hand-rolling a NEAR call in the consumer. The method lists are canonical in the spec crates' `for_each_*_method!` macros (the RPC service registration and `METHODS.md` both expand them); add or remove a method's line there whenever you add or remove a method — it is the only step, and a removed spec left in the list is a compile error.
  Minimum verification: `cargo check --workspace`; `cargo test -p templar-gateway-catalog` (keeps `METHODS.md` in sync — regenerate with `cargo test -p templar-gateway-catalog regenerate_methods_md -- --ignored`); plus the narrowest relevant `cargo test -p templar-gateway-<crate> -- --nocapture`.

## Working Norms

- Prefer small, targeted changes over broad refactors.
- Never fully delete and recreate an existing file when editing. Apply small, in-place patches that preserve unaffected content.
- Do not revert unrelated user changes in the worktree.
- Treat `common` changes as high-impact: they often affect multiple contracts and tests.
- Keep event/schema changes deliberate. If a public event or JSON payload changes, check versioning and downstream compatibility.
- Preserve existing crate structure and naming patterns unless there is a strong reason to change them.
- This codebase is security-sensitive. Review changes with an auditor mindset, especially in smart contracts and cross-contract flows.

## Build And Test

- Format: `cargo fmt`
- Workspace tests: `./script/test.sh`
- Common crate: `cargo test -p templar-common --lib -- --nocapture`
- One test file: `cargo test -p <package> --test <name> -- --nocapture`
- One unit test: `cargo test -p <package> <test_name> -- --nocapture`

Notes:

- Some integration tests use `near-workspaces` and may need permission to bind local ports.
- `cargo test -p templar-common --lib` is a good fast regression check for logic changes in `common`.
- `contract/vault`, `contract/registry`, and `contract/universal-account` all have `near-workspaces`-backed tests. If they fail in a restricted environment, say that clearly instead of silently skipping them.
- For tests that deploy contracts into `near-workspaces`, prefer prebuilt test contracts. Rebuilding WASM inside each test run is much slower.
- `./script/test.sh` already handles this by running `./script/prebuild-test-contracts.sh` first and setting `TEST_CONTRACTS_PREBUILT=1`.
- If you run `near-workspaces` tests directly, prefer following the same pattern: prebuild first, then run tests with `TEST_CONTRACTS_PREBUILT=1`.

## Code Search

- Use `rg` for text search.
- Use `rg --files` to find files.
- When reviewing behavior changes, check both staged and unstaged diffs if the worktree is dirty.

## Rust Conventions

- Prefer parsing over validation. Express invariants in types wherever practical, and make invalid states unrepresentable.
- Prefer stronger types over loosely constrained values: enums over well-known strings, sets over vectors when uniqueness matters, dedicated newtypes over primitive obsession, and structured state machines over ad hoc flags.
- Follow existing error-handling patterns with `anyhow`, `thiserror`, and `require!`/panic helpers already used in the codebase.
- Prefer a functional, pure style when it remains readable and efficient. Favor transformations over mutation, message passing over locks, and declarative code over imperative code. If the purely functional version would be materially less readable or obviously less efficient, prefer the simpler maintainable implementation.
- Keep the codebase DRY. When non-trivial logic is repeated, consider extracting a helper, module, or shared crate instead of duplicating it.
- Avoid shorthands and abbreviations in identifiers unless the shorter form is already standard and clearer.
- Avoid introducing `unwrap()` in non-test code unless the surrounding file already relies on an invariant and documents it clearly.
- Keep serialization explicit for contract-facing and event-facing structs.
- When changing emitted events, verify the payload compiles and still reflects the intended business semantics.

## Validation Expectations

- If you change public events, contract methods, or shared structs, run at least the narrowest relevant crate tests.
- If you change a high-impact crate, use the crate-specific verification command from the "High-Impact Areas" section unless you have a good reason not to.
- Write comprehensive unit tests for new, non-trivial logic. Prefer `#[rstest]` parameterization when it improves coverage and keeps cases readable.
- Keep fuzzers current when behavior changes affect parsing, arithmetic, state transitions, or other high-risk logic. Run them periodically, not only after major rewrites.
- If you cannot run an important verification step, say so explicitly.

## Documentation

- Pay attention to documentation comments, READMEs, and Markdown documents across the repository. Update them when behavior, interfaces, or operational expectations change.
- Read crate-local documentation before changing high-impact areas, especially `contract/vault/README.md`, `contract/vault/STRIDE.md`, `contract/universal-account/README.md`, and `service/relayer/README.md`.
- Keep this `AGENTS.md` file up to date when repository workflows, verification steps, important invariants, or high-impact crate guidance change in a way that would matter to future agents.

## Security Notes

- Treat every code change as potentially security-relevant.
- Evaluate edge cases around asynchronous receipt execution and cross-contract call ordering. Watch for TOCTOU-style issues.
- Be careful with gas usage and callback chains. Out-of-gas behavior in cross-contract flows can produce surprising partial-failure states.
- On NEAR, storage registration and unregistration behavior matters. In particular, consider the consequences of accounts calling `storage_unregister` and then becoming unable to receive returned NEP-141 assets or interact with NEP-145-aware contracts.
- Check invariants around refunds, withdrawals, account deletion, authorization, and replay or double-execution risks.
