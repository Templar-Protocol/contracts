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

## Read Results

A read's `output` is the value itself — `Option<Proxy<Source>>`, `Vec<PriceIdentifier>`, `bool` — not a struct whose only field relabels it (`GetProxyResult { proxy }`). The method name already says what the value is, and a bare value can be piped from one command straight into another that takes it.

Give a read's output its own struct only when it has two or more fields. This governs the output itself, not the element types inside it (`Vec<PriceEntry>` is fine). `op.get` is the one exception: its `{ operation }` matches `WriteOperationResult`, so a write's response and a later lookup of the same operation share a shape.

`read_outputs_are_not_single_field_wrappers` in `templar-gateway-catalog` is a tripwire for the common case, a single-field struct returned directly, across the shared read lists (`op.get` is registered outside them). It does not look inside `Option` or `Vec`; review covers the rest.

## Supplied Oracle Price Resolution

`oracle.resolvePrice` and `oracle.resolvePrices` preserve the configured aggregator's fallback behavior. For consumers that require a complete source set, `oracle.resolvePriceAllSources` accepts the same flattened input as `oracle.resolvePrice` and returns `None` if any effective proxy source is missing, fails transformation, is nonpositive after transformation, or cannot convert to a kernel price. Successful resolution uses the same transformations and aggregator math; direct and LST results must also be positive.

This opt-in method preserves the ordinary resolver's source-specific age and projection rules. It does not fetch or verify supplied payloads: callers requiring verified fresh data must first use the provider-backed previews below.

## Provider-Backed Oracle Previews

`oracle.getLazerUpdate { oracle_id, feed_ids }` and `oracle.getRedStoneUpdate { oracle_id, feed_ids }` fetch signed data through the configured provider source, then verify it using the deployed adapter's stateless view. They return the same bare price shapes as `lazer.getFeedsData` and `redstone.readPriceData`, without writing or merging stored prices. Rust clients use `Client::via::<OracleUpdatesDispatch>()` and `read`.

Verification, Lazer projection configuration, and the projection clock share one block snapshot selected after the provider fetch. Lazer returns `None` for absent or intrinsically invalid requested feeds (including spot-only feeds without EMA); duplicate request IDs collapse, and repeated signed feeds select the first valid candidate unless a strictly newer valid candidate follows. RedStone preserves request order and duplicates, and rejects a verified payload missing any requested feed. Its `write_timestamp` is a preview at the verification block, not evidence of a write.

Empty requests return empty collections without provider or RPC access. Provider, block, and adapter-view failures propagate; they never fall back to stored prices. Previews do not simulate write authorization, fees, storage charges, write-only limits, or replay checks against stored state. A later write can observe different state and fail.

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
- `dao.*`: typed Sputnik policy reads and proposal submission

## Guidance

- Prefer a domain namespace over a low-level namespace when the method represents a standard or protocol concept.
- For token transfers and balances where the token standard is not fixed at the call site (e.g. an asset that may be NEP-141 or NEP-245), prefer the standard-agnostic `token.*` methods over `ft.*`/`mt.*`. They dispatch on the standard internally, so a caller cannot pick the wrong one.
- A method must not carry an opaque payload when this codebase has types for it. Give it typed fields; leave the passthrough to the one designated escape-hatch method per namespace (`registry.deploy` for deploys, `contract.viewFunction` for reads). A command with typed args for some inputs and passthrough for others belongs on the typed method — the passthrough case is the escape hatch's.

## Sputnik DAO Proposals

`dao.getPolicy { dao_id }` returns the native policy; `dao.getProposal { dao_id, id }`
returns the flattened proposal, including its ID. Policy and proposal-input types reuse
the pinned Sputnik serializers. The typed proposal read boundary also preserves the
canonical deployed DAO's numeric vote counts and absent audit history, alongside
modern decimal-string counts and available history. Missing history is not an empty log.
`dao.addProposal { dao_id, proposal }` plans `add_proposal` with the current exact proposal bond
and 30 Tgas. Policy preflight rejects known-ineligible proposers, including proposal-kind-specific
permissions; delegated-token membership is left to the contract. Authorization and the bond can
change after planning, so contract checks remain authoritative.

Submission success means a proposal was created, not approved or executed. Read the ID returned by
the submission, then fetch that exact proposal. Do not derive it from a later proposal counter.
Execution and downstream receipt success must be verified separately.

`proxyOracleGovernance.createProposal` and `executeProposal` accept optional exact
`gas` overrides. When omitted, planning budgets payload-sized governance work and
a safety buffer; execution additionally reserves the stored target call's declared
gas, or the authoritative self-upgrade migration gas and deployment overhead.
Defaults leave a conservative single-call Sputnik forwarding/callback allowance
within the live protocol cap. Impossible budgets are rejected, never clamped.
An explicit override remains available for direct or separately budgeted callers,
but must exceed the child reservation and satisfy the protocol cap.

These are conservative, receipt-calibrated planning allowances, not a statistical
99% guarantee for arbitrary DAO versions, policies, payloads or multi-action proposals.
Already-approved child gas and transaction bytes are never rewritten. A successful
governance receipt also does not prove detached target success; inspect child receipts.

## Snapshot-Bound Market Reads

Rust callers comparing a market's raw storage snapshot with its configuration can use
`near_client.market(account).get_configuration_at(block_hash).await`. This typed read bypasses the ordinary
configuration cache and uses the exact snapshot block rather than a newer finalized block.

## Safety Inspection Reads

- `account.getSpendableBalance { account_id }` returns liquid NEAR after storage
  backing not already covered by locked stake, using the live protocol byte cost.
  `account.get.amount` alone is not a spendable funding budget; gas and attached
  deposits must still fit within the returned amount.
- `chain.getIdentity` takes JSON `null` and returns the node's chain ID and actual
  genesis block hash from `status`; it does not require archival block access.
- `chain.getTransactionValidityPeriod` takes JSON `null` and returns a bare integer
  validity window in blocks from the live `EXPERIMENTAL_protocol_config` response.
  Query the selected endpoint rather than assuming mainnet/testnet defaults,
  especially when using an RPC override.
- `token.getMetadata { token }` uses the existing typed FT/MT token reference and
  returns decimals plus optional name/symbol. MT identity is the complete opaque
  token ID: missing IDs, ambiguous metadata, and absent/out-of-range decimals
  fail rather than falling back to an embedded FT contract or inferred precision.
- `proxyOracle.getCachedPrice { oracle_id, id, age }` reads the proxy's accepted
  EMA cache directly. A live source does not imply a populated or usable cache;
  clearing a manual trip still requires a successful refresh.
- `proxyOracle.getFreshSourcePrice { oracle_id, id, age }` requires every effective
  stored source to be present, transformable, positive, convertible, nonfuture,
  and fresh under both the deployed filter and requested age, using a chain block
  timestamp read after all source views. These reads are not one block-pinned
  snapshot or an atomic guarantee about a later transaction. The method does not
  change `oracle.getPrice` or the supplied `oracle.resolvePriceAllSources`
  resolver's existing semantics.
- `tx.get` includes optional original transaction inspection: reported hash,
  signer, receiver, total action count, indexed top-level function calls, and
  indexed canonical NEP-461 delegate payload hashes. Inner delegated calls are
  not presented as top-level calls. Pending results have no inspection; mismatched
  requested hash/sender metadata fails closed. Receipt success alone does not
  establish that a transaction executed the expected proposal or signed payload.

## Local DB

- Use a local Postgres server with separate databases per crate.
- Run gateway store migrations from `gateway/store/`.
- Run `cargo sqlx prepare` from `gateway/store/` so `.sqlx` stays crate-local.
- `gateway/store` defaults `SQLX_OFFLINE=true` during normal builds; override it when regenerating `.sqlx` against a live dev database.
