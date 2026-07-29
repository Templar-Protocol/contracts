# Soroban Vault CLI Command Reorganization

## Scope and current state

The Soroban vault CLI currently concentrates its application layer in
`tools/soroban-vault-cli/src/commands.rs`. On `origin/dev` that file is 8,030 lines and combines
top-level dispatch, safety guards, deployment and recovery workflows, command handlers, manifest
auditing, response models, rendering, error envelopes, and 61 tests.

This reorganization targets the latest `origin/dev` implementation. It preserves the public CLI,
manifest and output schemas, command ordering, error text, authority checks, checkpoint behavior,
and the public `commands::{run, print_error, print_parse_error}` API.

## Proposed structure

```text
tools/soroban-vault-cli/src/commands/
├── mod.rs                 # Public facade and top-level dispatch
├── context.rs             # CommandContext and manifest loading
├── safety.rs              # Mainnet, fresh-state, and governance confirmation guards
├── audit.rs               # Successful-write transaction records
├── doctor.rs
├── user.rs
├── curator.rs
├── governance.rs
├── share_token.rs
├── adapter.rs
├── ttl.rs
├── inventory.rs           # Manifest lookups, adapter indexing, status/export projections
├── invoke.rs              # Named Soroban arguments and output-to-response conversion
├── vault_ops.rs           # Amount conversion and shared vault execution
├── output/
│   ├── mod.rs             # Response enum and serialized models
│   ├── render.rs          # Human and JSON rendering
│   └── error.rs           # Runtime and parse error envelopes
├── deploy/
│   ├── mod.rs             # Deployment command dispatch
│   ├── session.rs         # DeploymentContext and checkpointed state transitions
│   ├── stack.rs           # Full-stack deployment and resume orchestration
│   ├── adapters.rs        # Adapter validation, capability checks, and deployment
│   ├── curator_proxy.rs   # Proxy provenance, initialization, and version checks
│   ├── reconcile.rs       # Chain/manifest reconciliation and wiring checks
│   └── plan.rs            # Strictly read-only deployment planning
├── test_support.rs
└── tests/                 # Feature-aligned facade and command tests
```

## Rationale and typed cleanup

- Keep `commands/mod.rs` as a small stable facade. `lib.rs` continues to resolve `pub mod commands`
  without call-site changes.
- Add a private `CommandContext` that owns the CLI reference, executor reference, and `Stellar`
  client so command handlers receive one explicit dependency bundle.
- Add a private `DeploymentContext` that combines the command context with mutable manifest access
  and owns deployment checkpoint operations.
- Replace long deployment helper argument lists with a private `ContractDeployment` request and an
  explicit initialization-state enum.
- Replace the fixed-key stack artifact map with typed stack artifact and deployed-contract results.
  Persisted manifest maps remain unchanged because adapter keys are dynamic and schema-visible.
- Split by top-level CLI feature. Shared modules (`context`, `inventory`, `invoke`, `vault_ops`, and
  `output`) must not import command handlers, preventing dependency cycles.
- Preserve checkpoint call ordering, including redundant checkpoints. Checkpoint deduplication and
  consolidation of the two curator-proxy workflows are deliberately excluded because they would
  change failure boundaries.

## Call-site and import checklist

- Keep `commands::run`, `commands::print_error`, and `commands::print_parse_error` public with their
  existing signatures.
- Keep `tools/soroban-vault-cli/src/lib.rs` unchanged.
- Keep `cli.rs`, `manifest.rs`, `types.rs`, `stellar.rs`, `artifacts.rs`, `Cargo.toml`, and
  `schema/output.schema.json` unchanged.
- Move imports into their owning feature modules and expose cross-module internals with the
  narrowest `pub(super)` visibility.
- Route top-level execution as: safety validation -> meta/doctor early exits -> manifest load ->
  dangerous-governance confirmation -> feature handler -> audit/final save -> output renderer.
- Route deployment writes through `DeploymentContext`; deployment plans and reconciliation receive
  immutable manifest access only.
- Move shared recording executors and fixtures to `test_support`; preserve every existing behavioral
  assertion when redistributing tests.

## Migration steps

1. Establish a green `origin/dev` crate baseline and save deterministic CLI help output.
2. Move `commands.rs` to `commands/mod.rs` mechanically.
3. Extract output, context, inventory, invocation, safety, and audit foundations.
4. Extract doctor and each top-level command family.
5. Add deployment request/context types and extract deployment modules in dependency order:
   session, adapters, curator proxy, reconciliation, planning, and stack orchestration.
6. Redistribute tests and add focused context/type tests without replacing behavioral coverage.
7. Tighten visibility and add concise module-level invariant documentation.

## Risks and verification

The principal risks are accidental changes to security-check ordering, manifest checkpoint timing,
Soroban argument ordering, JSON field order, and response/error rendering. Verification therefore
includes the complete crate test suite, fake-executor call assertions, partial-failure recovery
tests, deterministic output comparison, formatting, clippy, and a workspace check.

Required final commands:

```text
cargo fmt --check
cargo clippy -p templar-soroban-vault-cli --all-targets
cargo test -p templar-soroban-vault-cli
cargo check --workspace
```

No production command module should exceed 900 lines, and the facade should remain below 250 lines.
