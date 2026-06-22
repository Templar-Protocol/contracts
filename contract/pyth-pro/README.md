# Pyth Pro oracle adapter

A push-style NEAR oracle that ingests [Pyth Pro](https://docs.pyth.network/lazer) signed price
payloads and re-serves them through the **same view ABI as `pyth-oracle.near`** — so it is a
drop-in Pyth oracle for the market and proxy-oracle.

> **Naming:** this adapter consumes **Pyth Pro** signed updates. Pyth Pro was formerly known as
> **Pyth Lazer**; some upstream protocol crates and contract paths still use the `lazer` name (e.g.
> `pyth-lazer-protocol`, the `lazer/contracts` reference paths). We use **Pyth Pro** for everything
> this repo owns and keep **Lazer** only when quoting such an upstream/legacy identifier.

This is intentionally a **storage oracle backed by Pyth Pro** (push → persist → serve the Pyth
views), *not* a stateless verifier like Pyth's official EVM/Sui/Aptos Lazer contracts — the drop-in
requirement needs stored, view-served prices. A read-only `verify_update` method adds the
official-style stateless verify-and-return surface alongside it (see the contract README).

Flow: a relayer submits a signed Pyth Pro payload (Pyth's **solana** / ed25519 format) → the adapter
verifies it (ed25519 signature against a trusted, non-expired signer set; channel filter; freshness
window; monotonic-per-feed timestamp for anti-replay) → stores the prices → consumers read them via
the Pyth view methods.

## Crates

| Crate | Path | What |
|-------|------|------|
| `templar-pyth-pro-verifier` | `verifier/` | Chain-agnostic verify + parse. No `near-sdk`. |
| `templar-pyth-pro-adapter-contract` | `contract/` | NEAR cdylib: storage, governance, Pyth views. |

The verifier wraps a forked, slimmed [`pyth-lazer-protocol`](https://github.com/Templar-Protocol/pyth-lazer-public/tree/feat/protocol-slim-build)
(pinned by `rev = "10aebfd0075887e9784f9fb65ef28ddbadb57139"` on the `feat/protocol-slim-build`
branch, `default-features = false`) for the wire format and adds the trust checks an on-chain
adapter needs.

## Identifier spaces

Pyth Pro keys feeds by a small `u32`; Pyth Core consumers use 32-byte `PriceIdentifier`s. The
adapter stores by `u32` and translates at read time through one isolated module
(`contract/src/feed_map.rs`). Map each consumer `PriceIdentifier` to its existing Pyth Core
identifier (`admin_set_feed_mapping`) to stay drop-in. That module is the *only* coupling between
the two spaces, so the mapping can later move to the proxy-oracle by deleting it alone.

## Governance

Single owner (`near_sdk_contract_tools::Owner`); all privileged methods are `admin_*`, each
`#[payable]` + `assert_one_yocto`. `update_price_feeds` is permissionless — authenticity is
cryptographic.

## Integration

No proxy-oracle changes: register the deployed account as `OracleType::Pyth(<account>)`.

## Build & test

```sh
cargo test -p templar-pyth-pro-verifier -p templar-pyth-pro-adapter-contract
cargo check --target wasm32-unknown-unknown -p templar-pyth-pro-adapter-contract
```

## Before mainnet

Real Pyth Pro signer public key(s) + expiry for a `config_prod` helper (see
[`TRUSTED_SIGNERS.md`](./TRUSTED_SIGNERS.md) for how to obtain/verify them on-chain); the channel to
accept (`allowed_channel_id`); an off-chain `service/pyth-pro-bridge` to subscribe and push updates.
