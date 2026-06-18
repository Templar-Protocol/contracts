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
- `ft.*`: NEP-141 standard operations
- `storage.*`: NEP-145 standard operations
- `registry.*`, `market.*`, `ua.*`: protocol/domain-specific operations

## Guidance

- Prefer a domain namespace over a low-level namespace when the method represents a standard or protocol concept.
- Prefer first-class typed methods when permissioning or validation matters.
- Use `contract.viewFunction` only for generic contract views that do not deserve a more specific typed RPC.

## Local DB

- Use a local Postgres server with separate databases per crate.
- Run gateway store migrations from `gateway/store/`.
- Run `cargo sqlx prepare` from `gateway/store/` so `.sqlx` stays crate-local.
- `gateway/store` defaults `SQLX_OFFLINE=true` during normal builds; override it when regenerating `.sqlx` against a live dev database.
