# Pyth Pro Adapter Spec

Naming: **Pyth Pro** for everything this repo owns, **Lazer** only for upstream/legacy identifiers
(see [`README.md`](./README.md)).

## Scope

- `templar-pyth-pro-verifier`: chain-agnostic payload verification/parsing.
- `templar-pyth-pro-adapter-contract`: NEAR storage, governance, fees, feed mapping, events, and Pyth-compatible views.

## Build Contract

- The verifier is chain-agnostic logic with no `near-sdk` dependency. It may use `std`; the
  upstream parser it wraps uses `std::io`, and the NEAR contract target provides `std`.
- Contract must compile for `wasm32-unknown-unknown`.
- Parser dependency changes must be pinned by exact revision and reviewed as security-sensitive.

Required checks:

```bash
cargo check --target wasm32-unknown-unknown -p templar-pyth-pro-adapter-contract
```

## Type Contract

- External Pyth Lazer protocol types are allowed at the parser boundary.
- Internal adapter/verifier time values should use `templar_primitives::Nanoseconds`.
- Raw timestamp `u64` values are allowed only when required by an external ABI or protocol type.
- A `Nanoseconds` field that is serialized to JSON must carry an `_ns` suffix (the type is erased in
  JSON, so the field name has to convey the unit), e.g. `FeedData.publish_time_ns`.
- Price output must preserve Pyth semantics: `price`, `conf`, `expo`, and `publish_time`.

## Verification Contract

An accepted update must satisfy:

- signed solana-format (ed25519) message decodes;
- the public key carried in the envelope is configured and unexpired;
- the ed25519 signature verifies over the payload under that public key;
- channel is allowed;
- package timestamp is within configured past/future bounds;
- each stored feed has price, exponent, and explicit strictly-positive confidence (a wire `0` is
  indistinguishable from absent and is rejected);
- EMA price and explicit strictly-positive EMA confidence are **required** for storage: a spot-only
  payload is rejected wholesale (the feed is skipped), so it cannot overwrite a stored feed and drop
  its EMA. This applies only to the stateful storage path; the stateless `verify_update` view stays
  at parity with the official Pyth Pro contracts and does not require EMA;
- effective per-feed publish time strictly advances stored data;
- age-gated reads reject stale and future-dated prices.

## Storage Contract

- Callers must pay newly consumed storage plus configured update fee.
- Feed mapping is `PriceIdentifier -> Lazer feed id`.
- Unmapping a price id does not delete retained feed data.
- Policy for storing signed but unmapped feeds must be explicit.

## Governance Contract

- Owner controls config, signer set, feed mappings, and withdrawals.
- Config updates must preserve signer-set and freshness-window invariants.
- Signer updates must preserve signer uniqueness. This invariant is structural: `SignerSet` is
  backed by a `BTreeMap<[u8; 32], u64>` (public key → `expires_at_s`), so duplicate keys cannot
  exist and iteration order is deterministic. It serializes transparently as the `Vec<TrustedSigner>`
  array, so the stored layout and `get_config` JSON shape are unchanged.
- Accepted trade-off: the verifier and contract keep separate `TrustedSigner` representations (the
  contract's carries JSON/Borsh serializers; the verifier's is plain), and verification builds a
  fresh `Vec<verifier::TrustedSigner>` per call (`SignerSet::verifier_signers`). This duplication and
  the per-update allocation are low-risk for a small (`SignerSet::MAX`) signer set and avoid
  feature-gating serializers onto the verifier type.

## View Contract

- Pyth-compatible spot and EMA view names must remain stable.
- Unsafe views may return latest stored data without age filtering.
- Age-gated and default-window views must apply freshness checks.
- EMA views must not fall back to spot prices.
- `verify_update(payload)` is a read-only, stateless verify-and-return: it runs the full
  verification (signer / ed25519 signature / channel / freshness) and returns the **complete** Lazer
  data (all properties) without writing storage, charging a fee, or touching feed mappings. It is
  the official-Lazer-contract-style parity surface, intended for off-chain RPC callers (and on-chain
  async callers via cross-contract call + callback — NEAR has no synchronous read calls).

## Model & parity (intentional scope)

This adapter is a **`pyth-oracle.near`-compatible storage oracle** (push → persist → serve), not a
stateless verifier like the official Lazer contracts — the firm drop-in requirement (rationale in
[`README.md`](./README.md)). `verify_update` adds the stateless surface alongside it. Known
narrowing: the verifier preserves every property as `Option` but does not distinguish "not
requested" from "requested-but-missing" (the official EVM `triStateMap` does) — a possible future
enrichment.
