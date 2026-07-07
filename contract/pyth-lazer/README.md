# Pyth Lazer oracle adapter

A push-style NEAR oracle that ingests [Pyth Lazer](https://docs.pyth.network/lazer) signed price
payloads and re-serves them by their native Lazer `u32` feed id. It is **Lazer-native**: consume it
by wrapping it in a **proxy oracle** as a `Lazer` source (addressed by feed id), not by pointing a
market/proxy `Pyth` source directly at it. The proxy-oracle owns the `PriceIdentifier ↔ feed_id`
mapping.

> **Naming:** Pyth now markets this product as **Pyth Pro** (formerly Pyth Lazer). We standardize on
> **Pyth Lazer** / `lazer` internally.

This is intentionally a **storage oracle backed by Pyth Lazer** (push → persist → serve the feed-id
views), *not* a stateless verifier like Pyth's official EVM/Sui/Aptos Lazer contracts — stored,
view-served prices are what the proxy-oracle reads. A read-only `verify_update` method adds the
official-style stateless verify-and-return surface alongside it (see the contract README).

Flow: a relayer submits a signed Pyth Lazer payload (Pyth's **solana** / ed25519 format) → the adapter
verifies it (ed25519 signature against a trusted, non-expired signer set; channel filter; freshness
window; monotonic-per-feed timestamp for anti-replay) → stores the prices by feed id → consumers read
them via the feed-id view methods.

## Crates

| Crate | Path | What |
|-------|------|------|
| `templar-pyth-lazer-verifier` | `verifier/` | Chain-agnostic verify + parse. No `near-sdk`. |
| `templar-pyth-lazer-adapter-contract` | `contract/` | NEAR cdylib: storage, governance, feed-id views. |

The verifier wraps a forked, slimmed [`pyth-lazer-protocol`](https://github.com/Templar-Protocol/pyth-lazer-public/tree/feat/protocol-slim-build)
(pinned by `rev = "10aebfd0075887e9784f9fb65ef28ddbadb57139"` on the `feat/protocol-slim-build`
branch, `default-features = false`) for the wire format and adds the trust checks an on-chain
adapter needs.

## Governance

Single owner (`near_sdk_contract_tools::Owner`); all privileged methods are `admin_*`, each
`#[payable]` + `assert_one_yocto`. `update_price_feeds` is permissionless — authenticity is
cryptographic.

## Integration

Reference the deployed adapter as a proxy-oracle **`Lazer` source** (`OracleRequest::Lazer`,
addressed by native `u32` feed id). Do **not** wire it as a classic `Pyth` source
(`OracleRequest::Pyth`, by `PriceIdentifier`) — the gateway rejects that for a Pyth Lazer adapter and
directs you to a `Lazer` source.

## Build & test

```sh
cargo test -p templar-pyth-lazer-verifier -p templar-pyth-lazer-adapter-contract
cargo check --target wasm32-unknown-unknown -p templar-pyth-lazer-adapter-contract
```

## Before mainnet

Real Pyth Lazer signer public key(s) + expiry for a `config_prod` helper (see
[`TRUSTED_SIGNERS.md`](./TRUSTED_SIGNERS.md) for how to obtain/verify them on-chain); the channel to
accept (`allowed_channel_id`); an off-chain `service/pyth-lazer-bridge` to subscribe and push updates.
