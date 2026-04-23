# Templar Gateway Architecture

## Purpose

The Templar Gateway is the NEAR-facing service boundary for off-chain code in this repository.

It owns:

- direct NEAR contract reads
- direct NEAR contract writes
- transaction planning and operation semantics
- transaction execution runtime and signer management
- durable operation/idempotency state
- tightly scoped oracle payload integrations needed to prepare writes

It does not own:

- JSON-RPC transport concerns beyond the service binary
- broader relayer business logic
- unrelated off-chain workflows
- authentication and authorization policy beyond whatever typed request shape is needed to execute an operation

This version is NEAR-only. Soroban or other chains may be added later, but they should not distort the current crate boundaries.

## Current Crate Layout

All gateway code lives under `gateway/`, with the transport binary under `service/gateway`.

Current crates:

1. `gateway/core`
2. `gateway/runtime`
3. `gateway/store`
4. `gateway/oracle-pyth`
5. `gateway/oracle-redstone`
6. `gateway/testing`
7. `service/gateway`

### `gateway/core`

This is the transport-agnostic source of truth for gateway behavior.

Responsibilities:

- request and response types
- method taxonomy
- operation plan and operation state types
- gateway error taxonomy
- NEAR client/query surface used by planning
- current dispatch and write-planning logic
- shared traits such as the oracle payload source abstraction

Requirements:

- no `jsonrpsee`
- no runtime ownership
- no actor lifecycle management
- no durable store implementation
- expose lightweight functions and traits that define operation behavior

The long-term direction is for `gateway/core` to move further toward a single-phase requirements model:

1. declare all required reads up front
2. resolve them outside the planner
3. finalize a plan from resolved inputs

That refactor is not complete yet, but `core` is the place where it should land.

### `gateway/runtime`

This crate owns runtime-hosted execution.

Responsibilities:

- signer ownership
- write execution lanes
- actor/message-passing execution scaffolding
- transaction submission orchestration
- runtime startup and lifecycle

Requirements:

- no transport glue
- no request decoding logic
- no ownership of operation semantics that belong in `gateway/core`

### `gateway/store`

This crate owns durable operation storage.

Responsibilities:

- operation store interfaces and implementations
- idempotency persistence
- in-memory and Postgres-backed stores
- migrations and `sqlx` metadata

Current structure:

- `memory.rs`
- `postgres.rs`

Notes:

- run migrations from `gateway/store/`
- regenerate `.sqlx` from `gateway/store/`
- normal builds should use offline metadata

### `gateway/oracle-pyth`

This crate owns the Hermes/Pyth integration.

Responsibilities:

- fetch Pyth update payloads
- implement the shared oracle payload trait for Pyth price IDs

### `gateway/oracle-redstone`

This crate owns the RedStone bridge integration.

Responsibilities:

- communicate with the RedStone bridge
- fetch RedStone payloads
- implement the shared oracle payload trait for RedStone feed IDs

### `gateway/testing`

This crate owns gateway-specific testing helpers.

Responsibilities:

- sandbox setup and helpers
- test-time client/runtime wiring
- future home for more extracted gateway test abstractions

### `service/gateway`

This is the JSON-RPC server binary.

Responsibilities:

- start and configure the RPC server
- decode JSON-RPC requests
- call into the gateway crates
- encode responses back to JSON-RPC
- wire together `core`, `runtime`, `store`, and the oracle integrations

Requirements:

- this is the only gateway crate that should depend on `jsonrpsee`
- keep it as thin transport and composition glue
- do not move planning or execution semantics into this crate

## Communication Layer

The gateway uses JSON-RPC because the API is operation-oriented rather than resource-oriented, and it is easy to consume from Go and JavaScript clients.

Constraints:

- `jsonrpsee` stays in `service/gateway`
- transport should not become the source of truth for method shapes
- business logic should remain outside the service binary

## API Design Principles

### Reads vs Writes

Reads and writes are differentiated by behavior, not by transport shape.

Writes are modeled as operations. Even a one-transaction write is still an operation with one step.

The current architecture already plans writes separately from executing them. The next refinement is to make read requirements explicit up front so planning can become more deterministic, cacheable, and easier to test.

### Typed Methods

The public API should prefer typed domain methods over generic function-call interfaces.

Examples:

- `registry.deploy`
- `market.borrow`
- `market.supply`
- `market.withdrawCollateral`
- `market.repay`
- `market.liquidate`
- `ua.execute`
- `ua.createAccount`
- `storage.ensureDeposit`

### Generic Escape Hatch

A generic low-level write surface may still exist where needed, but it should remain clearly secondary to typed operations.

### Idempotency

Transport request IDs are not durable operation identities. Durable write idempotency remains part of the gateway design and belongs in the store layer.

## Auth Scope

Authentication and authorization policy are out of scope for the gateway itself.

The surrounding deployment or caller-facing service is responsible for access control. The gateway should not become an auth system.

The gateway does not:

- store auth tokens
- enforce caller permissions
- evaluate deployment-specific policy

Key material should come from host-managed secret sources, not from Postgres.

## Persistence Model

Use Postgres for durable control-plane state.

Persist:

- operation records
- operation step records
- idempotency mappings
- managed account metadata as needed by execution

Do not persist:

- private keys
- raw secrets
- ephemeral RPC caches
- volatile signer nonce cache state

The runtime should reconstruct volatile signer state from durable metadata plus externally provided secrets.

## Runtime Model

The runtime crate currently uses actor-style ownership boundaries for execution and signer coordination.

That is acceptable as an execution detail, but those concerns should remain outside `gateway/core`.

The key architectural boundary is:

- `core` defines what should happen
- `runtime` owns how a planned write is executed
- `store` owns durability

## Method Taxonomy

The gateway surface is expected to include:

- public read methods
- operation-producing write methods

Illustrative read surface:

- `system.health`
- `system.version`
- `chain.viewAccount`
- `chain.viewFunction`
- `chain.getTransaction`
- `registry.listDeployments`
- `registry.listVersions`
- `market.getConfiguration`
- `market.listBorrowPositions`
- `ua.getKey`
- `storage.getBalanceBounds`
- `storage.getBalanceOf`

Illustrative write surface:

- `tx.functionCall`
- `storage.deposit`
- `storage.ensureDeposit`
- `registry.deploy`
- `ua.execute`
- `ua.createAccount`

The exact surface should continue to become more typed over time.

## Documentation And Spec Generation

The gateway contract should be derived from Rust types, not hand-maintained transport docs.

Approach:

- define the contract in Rust types first
- derive serde and schema information from those types
- keep the public contract and method metadata close to `gateway/core`

`jsonrpsee` must not become the source of truth for the public API.

## Near-Term Direction

The current crate split is in place. The next meaningful architectural step is not more crate churn.

The next step is to move `gateway/core` toward a single-phase requirements model for planning:

1. compute a deterministic set of reads required for an operation
2. resolve them with caching and deduplication outside the planner
3. finalize the operation plan from resolved data

That should make requirements more visible, easier to cache, and easier to test without pushing lifecycle/runtime concerns back into the core crate.
