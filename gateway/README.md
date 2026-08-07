# Gateway Naming

This document defines the RPC naming conventions for the Templar Gateway.

## Read Methods

Read-only methods should use one of these forms:

- `get*` for fetching a single thing or lookup-style queries
- `list*` for fetching multiple things
- `viewFunction` for generic contract view calls where the method name and arguments are part of the input

Examples:

- `account.get`
- `tx.get`
- `contract.getVersion`
- `contract.viewFunction`
- `registry.getDeployment`
- `registry.listVersions`

## Write Methods

Write methods should use imperative verbs.

Examples:

- `account.delete`
- `registry.addVersion`
- `registry.removeVersion`
- `registry.deploy`
- `storage.deposit`
- `storage.unregister`
- `ft.transfer`

## Namespace Levels

Methods in the same namespace should stay at roughly the same level of abstraction.

- `account.*`: account state and account lifecycle
- `contract.*`: generic contract introspection and generic contract view calls
- `tx.*`: low-level transaction submission and transaction inspection
- `ft.*`: NEP-141 (fungible token) standard operations
- `mt.*`: NEP-245 (multi-token) standard operations
- `token.*`: standard-agnostic token operations that dispatch NEP-141 vs NEP-245 internally
- `storage.*`: NEP-145 standard operations
- `artifact.*`: contract artifact (WASM byte) operations across the artifact catalog and `registry.addArtifactVersion`
- `registry.*`, `market.*`, `ua.*`: protocol/domain-specific operations

## Guidance

- Prefer a domain namespace over a low-level namespace when the method represents a standard or protocol concept.
- For token transfers and balances where the token standard is not fixed at the call site (e.g. an asset that may be NEP-141 or NEP-245), prefer the standard-agnostic `token.*` methods over `ft.*`/`mt.*`. They dispatch on the standard internally, so a caller cannot pick the wrong one.
- A method must not carry an opaque payload when this codebase has types for it. Give it typed fields; leave the passthrough to the one designated escape-hatch method per namespace (`registry.deploy` for deploys, `contract.viewFunction` for reads). A command with typed args for some inputs and passthrough for others belongs on the typed method — the passthrough case is the escape hatch's.

## Local DB

- Use a local Postgres server with separate databases per crate.
- Run gateway store migrations from `gateway/store/`.
- Run `cargo sqlx prepare` from `gateway/store/` so `.sqlx` stays crate-local.
- `gateway/store` defaults `SQLX_OFFLINE=true` during normal builds; override it when regenerating `.sqlx` against a live dev database.
