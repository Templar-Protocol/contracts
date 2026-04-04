# Blockchain Gateway Refactor Plan

## Purpose

The blockchain gateway is a NEAR-only refactor of the code that interacts with smart contracts in this repository.

This first version does not include Soroban. Soroban may be added later, but it must not drive the current design.

The gateway boundary is strict:

- Include all direct NEAR blockchain interactions.
- Include direct NEAR contract queries.
- Include direct NEAR contract writes.
- Include signing, key management, and nonce management required to safely submit transactions.
- Include higher-level operations composed from direct NEAR interactions.
- Exclude non-NEAR business logic and non-RPC outbound HTTP integration.

Examples:

- Include universal account creation and execution.
- Include registry deployment flows.
- Include market interactions such as borrow, supply, repay, withdraw, liquidation, and accumulation.
- Exclude relayer PoW verification.
- Exclude generic off-chain HTTP fetch logic such as Hermes or RedStone payload retrieval. Those payloads should be fetched by the caller and passed into the gateway if needed.

## Product Shape

The refactor should comprise:

- blockchain gateway common crate(s)
- one service crate

The service crate is an internal binary, but partners may also deploy it in their own infrastructure.

## Communication Layer

The gateway should use JSON-RPC.

Why:

- The API is operation-oriented rather than resource-oriented.
- Go and JavaScript clients can consume it easily.
- It avoids protobuf and gRPC friction for JavaScript-heavy partner integrations.

Implementation constraint:

- `jsonrpsee` should be used only as thin transport glue.
- The RPC contract, types, business logic, auth policy, actor messages, and execution model must not depend on `jsonrpsee`.
- Only the service binary crate should depend on `jsonrpsee`.
- The service binary crate should contain no business logic. It should only perform request decoding, auth extraction, dispatch, and response encoding.

Transport preference:

- Prefer local/private deployment patterns.
- Unix domain sockets are a good default for same-host use.
- TCP may be supported for partner/self-hosted deployments.

## Crate Layout

All blockchain gateway code lives under `gateway/` at the project root.

Current intended crates:

1. `gateway/core`
2. `gateway/near`
3. `gateway/testing`
4. `service/blockchain-gateway`

### `gateway/core`

This crate is the transport-agnostic source of truth for the public contract.

Responsibilities:

- shared primitive newtypes
- RPC method taxonomy
- request/response types
- operation IDs and operation status types
- auth policy types
- error taxonomy
- schema/spec metadata traits

Requirements:

- no `jsonrpsee`
- derive serde and schema information from the same Rust types used in production
- prefer strong newtypes over raw strings where practical
- use `near_account_id::AccountId` for account identifier newtypes, not `String`

### `gateway/near`

This crate contains NEAR-specific business logic and internal orchestration.

Responsibilities:

- `near-api` integration
- actor model implementation
- auth enforcement logic
- operation planning and execution
- account/key lane management
- nonce management
- signing and submission
- Postgres-backed persistence for auth, operations, idempotency, and audit
- typed contract/domain logic for registry, market, universal account, and storage interactions

Requirements:

- no `jsonrpsee`
- no transport-specific request handling
- keep all business logic here rather than in the service crate

### `gateway/testing`

This crate will eventually absorb the test controller abstractions currently in `test-utils`.

Responsibilities:

- typed test controllers
- replacement for the current controller DSL / `define!`-style ergonomics
- near-sandbox support
- migration path away from `near-workspaces` assumptions where practical

### `service/blockchain-gateway`

This crate is the JSON-RPC server binary.

Responsibilities:

- configure and start the RPC server
- extract auth credentials
- deserialize JSON-RPC requests into internal typed requests
- dispatch into `gateway/near`
- encode results and errors back into JSON-RPC responses

Requirements:

- this is the only crate allowed to depend on `jsonrpsee`
- no business logic here
- no direct NEAR execution logic here
- no auth policy evaluation beyond calling into internal logic

## API Design Principles

### Reads vs Writes

Reads are public and should not require auth because blockchain data is public.

Writes require auth.

The auth policy surface should therefore whitelist write methods only, plus any future admin-only internal methods if such methods are added.

### Typed Domain Methods

The public API should prefer typed service-level methods rather than raw generic function calls.

Typed domain methods are RPC methods that correspond to a contract/domain concept such as:

- `registry.deploy`
- `market.borrow`
- `market.supply`
- `market.withdrawCollateral`
- `market.repay`
- `market.liquidate`
- `ua.execute`
- `ua.createAccount`
- `storage.ensureDeposit`

These methods do not have to mirror on-chain contract method names exactly. They may be renamed or grouped to present a cleaner service-level API.

### Generic Escape Hatch

Include a generic write method for now:

- `tx.functionCall`

But this is a temporary escape hatch. The long-term goal is to remove or de-emphasize it once typed methods cover all necessary interactions.

If present, it must be tightly constrained by auth policy.

### All Writes Are Operations

There is no useful architectural distinction between single-transaction and multi-transaction writes.

All writes should be modeled as operations.

- a one-transaction write is just an operation with one transaction step
- a multi-transaction write is just an operation with multiple transaction steps

Every write should return an operation ID.

Default write behavior should wait for final completion unless or until a later non-blocking mode is added.

### Idempotency

Write idempotency is useful in v1.

Reason:

- JSON-RPC request `id` is only a transport correlation field
- it is not a durable operation identity
- retries may come from a different client session or process

The gateway therefore needs durable operation tracking and idempotency records for write requests.

## Auth Model

Auth is whitelist-only and deny-by-default.

If a method is not explicitly enabled for a token, it is not allowed.

Auth is account-scoped.

Requirements:

- a token may only submit transactions for an explicitly allowed set of signer accounts
- a token may be authorized for more than one signer account
- every write request must include the signer account the caller wants the service to use

Examples:

- the relayer token may only be allowed to sign for `registry.near`
- the accumulator token may only be allowed to sign for `accumulator.near`

Auth policy should be expressive enough to capture:

- allowed signer accounts
- allowed typed write methods
- constraints for generic function calls such as allowed receivers and allowed on-chain method names

Reads should generally remain unauthenticated.

Secrets must not be stored in the database.

Key material should come from:

- environment variables
- a secrets manager
- a keychain or equivalent host-managed secret source

Persist only key metadata such as:

- logical key ID
- owning account
- public key
- status
- secret source reference

## Persistence Model

Use Postgres for durable control-plane state.

This is already a reasonable dependency in this repository because the relayer already uses `sqlx` and Postgres.

Persist in Postgres:

- auth tokens / principals / policy
- managed account metadata
- managed key metadata only
- operation records
- operation step records
- idempotency mapping
- audit events

Do not persist in Postgres:

- private keys
- raw secret values
- ephemeral block hash caches
- ephemeral in-memory lane scheduling state

The volatile runtime should reconstruct signer and lane state on startup from durable metadata plus host-provided secrets.

## Actor Model

Prefer message passing throughout. Full actor model is ideal.

The relayer implementation should be used as inspiration here.

At minimum, the architecture should preserve actor-like ownership boundaries.

Current intended actors:

1. `ManagedAccountActor`
2. `KeyLaneActor`
3. `RpcActor`
4. domain actors such as `RegistryActor`, `MarketActor`, `UniversalAccountActor`, `StorageActor`
5. `OperationActor`

### `ManagedAccountActor`

Owns one managed signer account and routes writes across its configured keys.

### `KeyLaneActor`

Owns one `(account_id, public_key)` lane.

This is the fundamental nonce lane because NEAR nonces are per access key, not per account globally.

Responsibilities:

- allocate nonces for that key
- refresh access key state
- sign transactions
- submit transactions
- track in-flight transactions
- reconcile state after timeout/failure

### `RpcActor`

Owns NEAR RPC access.

Responsibilities:

- view calls
- account/access-key queries
- transaction submission
- transaction status polling
- block lookups needed for transaction freshness

### `OperationActor`

Coordinates multi-step operations and records durable progress.

## Nonce Management

The current repository contains two relevant patterns:

### Liquidator `NonceTracker`

The liquidator currently has a small atomic nonce tracker.

It:

- tracks the highest locally used nonce
- combines that with the RPC-reported access-key nonce
- allocates `max(rpc_nonce, tracked_nonce) + 1`
- uses CAS to guarantee process-local uniqueness under concurrency

This is useful as a primitive, but it is not a full nonce manager.

It does not by itself provide:

- reservation lifecycle tracking
- submitted vs reserved distinction
- restart recovery
- per-key operation reconciliation
- robust stale block hash handling

### Relayer Cache Service

The relayer has a more actor-like cache service that also serves nonce-related state and a block hash.

This centralizes access but mixes caching and sequencing concerns.

### Desired Direction

The gateway should treat each access key as its own serialized nonce lane.

The service should support multiple keys per managed account and use those lanes for parallelism.

Do not expose raw nonce reservation or raw signing as public RPC methods.

Reason:

- signing is tightly coupled to nonce reservation, block hash freshness, and submission
- exposing `sign.transaction` or `reserve_nonce` would create ambiguity if the caller never submits or submits out of band

Instead:

- the nonce manager should be internally separate as an actor
- but externally hidden behind write operations that sign and submit as one service-owned flow

Important conclusion:

- public raw signing primitives are dangerous for logical consistency
- signing and submission should remain tightly integrated

## Method Taxonomy

The current intended taxonomy is:

- public read methods
- write methods

The current core crate already has preliminary enums for this structure.

### Initial Read Surface

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

### Initial Write Surface

- `tx.functionCall`
- `storage.deposit`
- `storage.ensureDeposit`
- `registry.deploy`
- `ua.execute`
- `ua.createAccount`

### Future Typed Market Surface

The gateway is expected to grow beyond the minimal first pass.

Future typed market methods should include at least:

- `market.borrow`
- `market.supply`
- `market.withdrawCollateral`
- `market.withdrawSupply`
- `market.repay`
- `market.liquidate`
- `market.accumulateBorrow`
- `market.accumulateStaticYield`

The general rule is that the gateway should eventually encompass all service needs of partners for NEAR interactions.

## Documentation and Spec Generation

Spec generation must not be an afterthought.

The system should be as self-documenting as possible.

Approach:

- define the contract in Rust types first
- derive serde and schema information from those same types
- use traits and method metadata in the core crate as the source of truth
- generate machine-readable spec/docs from those types

Important constraint:

- do not hand-build a large documentation/spec generation system as part of this refactor
- prefer a dependency to do the spec generation work for us

Current direction:

- use `schemars` in the core types
- find a dependency-assisted way to generate a spec from the same type registry
- do not let `jsonrpsee` become the source of truth for the API contract

## Type System Principles

Compile-time verification of invariants is highly valuable.

The design should leverage the Rust type system for:

- explicit method taxonomy
- strong newtypes rather than primitive obsession
- account ID wrappers
- auth policy types
- operation status and lifecycle modeling
- internal request state transitions where typestate improves clarity and safety

Typestate is considered useful here, particularly for:

- parsed vs authorized write requests
- operation lifecycle stages
- internal execution stages where illegal transitions should be made unrepresentable

## Current Scaffold Status

Initial module structure has already been created.

Current files/crates introduced:

- `gateway/core`
- `gateway/near`
- `gateway/testing`
- `service/blockchain-gateway`

Current notable decisions already reflected in code:

- `jsonrpsee` is only used in `service/blockchain-gateway`
- `gateway/core` owns method taxonomy and primitive newtypes
- `gateway/near` owns actor/business scaffolding
- `gateway/testing` exists as the future home for extracted testing abstractions
- transparent newtype helpers have been introduced in `gateway/core`
- `near_account_id::AccountId` is used for account ID wrappers

## Next Implementation Steps

The immediate next steps are:

1. define the initial typed RPC request/response types in `gateway/core`
2. define method metadata/spec traits strongly enough to support future spec generation
3. define internal service traits and actor message interfaces in `gateway/near`
4. define persistence interfaces and initial Postgres schema for auth, operations, idempotency, and audit
5. begin implementing the `jsonrpsee` adapter only after the internal typed contract is stable enough

## Guardrails

- Keep changes small and incremental.
- Preserve the boundary that the service crate is glue only.
- Avoid introducing business logic into the transport layer.
- Avoid stringly typed method dispatch outside the JSON-RPC boundary.
- Prefer typed service-level methods over generic function calls.
- Keep the generic function-call escape hatch constrained and temporary.
- Never store secrets in the database.
