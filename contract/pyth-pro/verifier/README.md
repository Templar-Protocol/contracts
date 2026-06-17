# templar-pyth-pro-verifier

Chain-agnostic verification and parsing for Pyth Pro (formerly Pyth Lazer) price updates. No `near-sdk`;
the host supplies a [`Crypto`] impl and converts the neutral result into its own ABI types. See
the [adapter overview](../README.md).

## API

- `Crypto` — host crypto: a single `ed25519_verify(signature, message, public_key) -> bool`
  (on NEAR, `env::ed25519_verify`).
- `verify_solana_update(crypto, raw_message, params) -> Result<VerifiedUpdate, VerifyError>` —
  decode the solana envelope, check the carried public key is trusted and unexpired, verify the
  ed25519 signature over the payload, decode the (little-endian) payload, then apply the channel
  filter and freshness window.
- `VerifyParams` — `trusted_signers`, `now_s`, `max_timestamp_delay_s`, `max_timestamp_ahead_s`
  (seconds), and `allowed_channel_id` (channel byte).
- `VerifiedUpdate { signer: [u8; 32], channel_id, timestamp: Nanoseconds, feeds: Vec<ParsedFeed> }`;
  each `ParsedFeed` field is `None` when the property was absent (or carried Lazer's zero sentinel).

## Notes

- Targets Pyth Pro's **solana** format (ed25519 — NEAR's native scheme), little-endian payload.
  The signer is identified by the 32-byte ed25519 public key carried in the envelope, so there is
  no hashing, recovery, or address derivation — just a membership check plus a signature verify.
- The verifier may use `std`; the upstream parser it wraps uses `std::io`, and the NEAR contract
  target provides `std`. It must not depend on `near-sdk`.
- Tests sign payloads with `ed25519-dalek` and round-trip through `verify_solana_update`; the
  `tests/real_pyth_pro_payloads.rs` regression test runs over real captured solana payloads.
