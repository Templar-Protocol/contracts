# Pyth Pro Trusted Signers

How to obtain and verify the Ed25519/Solana signer public key(s) the adapter must trust
(`Config.signers`). See [`SPEC.md`](./SPEC.md) for the verification contract.

## Finding

There is **no static list** of Pyth Pro signer keys in Pyth's docs. The canonical Solana signer set
is on-chain state in the deployed Pyth Pro Solana program storage account. Signers **rotate and
expire** — never treat a recovered/hardcoded value as permanent.

- Solana program, from Pyth Pro contract-address docs:
  `pytd2yyk641x7ak7mkaasSJVXh6YYZnC7wTmtgAyxPt`
- Storage PDA, derived from seed `storage` under that program:
  `3rdJbqfnagQ4yx9HXJViD4zc4xpiSqmFsKpPuSCQVyQL`
- Upstream/local source shape:
  - `Storage.trusted_signers: [TrustedSignerInfo<Pubkey>; 5]`
  - `Storage.num_trusted_signers: u8`
  - `TrustedSignerInfo { pubkey: Pubkey, expires_at: i64 }`

## Current Value

Read from real Pyth Pro Solana payloads:

```text
publicKey:    9gKEEcFzSd1PDYBKWAKZi4Sq4ZCUaVX5oTr8kEjdwsfR
publicKeyHex: 80efc1f480c5615af3fb673d42287e993da9fbc3506b6e41dfa32950820c2e6c
```

The same key was present in the Solana storage account when checked on 2026-06-17:

```text
expiresAt:    2052483257
expiresAtIso: 2035-01-15T14:14:17.000Z
active:       true
```

This value is useful evidence and a regression fixture, but it is **not** a trust root by itself.
Trust comes from checking the public key against live Solana program state.

## Reproduce

### A. Read the signer from a Solana-format payload

The Solana Lazer envelope contains:

- 4-byte little-endian Solana format magic
- 64-byte Ed25519 signature
- 32-byte Ed25519 public key
- 2-byte little-endian payload length
- little-endian Lazer payload bytes

The public key is carried in the envelope (no recovery); inspect a captured payload and verify its
signature:

```sh
node contract/pyth-pro/solana-payload-signer.mjs <BASE64_OR_HEX_PAYLOAD>
printf '%s\n' '<BASE64_PAYLOAD>' | node contract/pyth-pro/solana-payload-signer.mjs
```

Expected shape:

```json
{
  "publicKey": "9gKEEcFzSd1PDYBKWAKZi4Sq4ZCUaVX5oTr8kEjdwsfR",
  "publicKeyHex": "80efc1f480c5615af3fb673d42287e993da9fbc3506b6e41dfa32950820c2e6c",
  "signatureVerified": true,
  "channel": 3,
  "feedIds": [7, 8, 1, 27, 23]
}
```

### B. Query the live trusted signer list on Solana

Use the dependency-free storage decoder:

```sh
node contract/pyth-pro/query-solana-trusted-signers.mjs
```

Optional environment:

```sh
SOLANA_RPC_URL=https://api.mainnet-beta.solana.com \
node contract/pyth-pro/query-solana-trusted-signers.mjs
```

Expected shape:

```json
{
  "programId": "pytd2yyk641x7ak7mkaasSJVXh6YYZnC7wTmtgAyxPt",
  "storageAccount": "3rdJbqfnagQ4yx9HXJViD4zc4xpiSqmFsKpPuSCQVyQL",
  "trustedSigners": [
    {
      "publicKey": "9gKEEcFzSd1PDYBKWAKZi4Sq4ZCUaVX5oTr8kEjdwsfR",
      "publicKeyHex": "80efc1f480c5615af3fb673d42287e993da9fbc3506b6e41dfa32950820c2e6c",
      "expiresAt": "2052483257",
      "active": true
    }
  ]
}
```

### C. Manual Storage Layout

The helper decodes the Anchor account directly:

```text
0..8      Anchor discriminator
8..40     top_authority Pubkey
40..72    treasury Pubkey
72..80    single_update_fee_in_lamports u64 LE
80        num_trusted_signers u8
81..281   trusted_signers[5], each:
          pubkey Pubkey (32 bytes)
          expires_at i64 LE
281..381  unused by this adapter path
```

A Solana signer is currently valid only if it appears in `trusted_signers[0..num_trusted_signers]`
and `expires_at > current_unix_timestamp_seconds`.

## Rotating the adapter's trusted signers

When Pyth rotates the Solana ed25519 signer, mirror it on the adapter with the owner-only,
1-yoctoNEAR `admin_*` methods (no redeploy). Signers are 32-byte ed25519 keys passed as **64-char
hex** — the Solana tools print base58, which is the same 32 bytes, so convert if needed. Always add
the new key *before* removing the old one (overlap window); the contract rejects removing the last
signer.

1. **Read the live Solana set** (source of truth):
   ```sh
   node contract/pyth-pro/query-solana-trusted-signers.mjs   # pubkeys + expiries
   ```
2. **Add / refresh** a signer on the adapter (pass `expires_at_s` = unix seconds, ideally matching
   Solana's `expiresAt`):
   ```sh
   near call <ADAPTER> admin_set_signer \
     '{"public_key":"<64-hex>","expires_at_s":<unix>}' \
     --accountId <OWNER> --depositYocto 1
   ```
   The same call with an existing key just refreshes its expiry.
3. **Remove** a retired signer once its overlap window has passed by omitting `expires_at_s`
   (rejected if it's the last one):
   ```sh
   near call <ADAPTER> admin_set_signer \
     '{"public_key":"<64-hex>"}' \
     --accountId <OWNER> --depositYocto 1
   ```
   Or replace the whole policy atomically (full set + windows + fee):
   ```sh
   near call <ADAPTER> admin_set_config '{"config":{ "signers":[...], ... }}' \
     --accountId <OWNER> --depositYocto 1
   ```
4. **Confirm**:
   ```sh
   near view <ADAPTER> get_config
   ```

Behavior is covered by `contract/.../tests/test.rs`
(`rotating_in_a_new_signer_accepts_its_updates`, `refreshing_signer_expiry_into_the_past_rejects_updates`,
`admin_set_signer_cannot_remove_last`, `admin_methods_reject_non_owner`).

## Before Mainnet

Read the public key from a fresh Solana-format payload (A), enumerate live signers from Solana
state (B), and set `Config.signers` to the current public key(s) with matching `expires_at_s`.

A dedicated CLI / `service/pyth-pro-bridge` step (planned) should reconcile our signer set against the
Solana storage account automatically so we track Pyth's rotations rather than updating by hand.
