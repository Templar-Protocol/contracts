# Universal Account

This smart contract accepts many different signature types and converts them into NEAR actions.

Currently supported:

- NIST-P256 (Webauthn passkey)
- Raw Ed25519 (Solana)
- Ethereum EIP-712
- Stellar SEP-53

## Build & Deploy

Generate init args by running the `init_args` example. Modify it first to include your own passkey.

```bash
cargo run --example 01_init_args
```

Example output:

```json
{"key":{"Passkey":"p256:S8avjv5zYFYhViXo7giqwynnMdox3RAytXQ7FG9a2tj8WxZnU6KUr36MSuUvgrwk4uGNMdiXt6vwtL9yBvj6VAUL"}}
```

```bash
cargo near deploy build-non-reproducible-wasm <account-id> \
    with-init-call new \
    json-args '<init-args>' \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '0 NEAR'
```

## Interact

1. Generate a payload to sign:

   ```bash
   cargo run --example 02_payload
   ```

   Example output:

   ```text
   Payload (stringified):
   UAccount Signed Message:
   {"parameters":{"block_height":"123456","index":"0","nonce":"1"},"account_id":"my-universal-account.testnet","payload":[{"receiver_id":"alice.testnet","actions":[{"Transfer":{"amount":"1000000000000000000000000"}}]}]}
   SHA-256 (base64):
   h29fWbM+x7bKkByAm9rQ4AsjH+1BbLjIR3BZQRRlYwQ
   ```

   Note that the payload begins with `"\x19UAccount Signed Message\n"`, and it is treated as a plain bytestring when calculating the SHA-256 hash.

1. Sign the payload using your passkey. As a part of this process, it should generate the following fields, which we will need for the next step:

   - `authenticatorData`
   - `clientDataJSON`
   - `signature`

1. Generate the message to send to the contract:

   ```bash
   cargo run --example 03_message
   ```

   Example output:

   ```json
   {"key":{"Passkey":"p256:S8avjv5zYFYhViXo7giqwynnMdox3RAytXQ7FG9a2tj8WxZnU6KUr36MSuUvgrwk4uGNMdiXt6vwtL9yBvj6VAUL"},"message":{"authenticator_data":"49960de5880e8c687434170f6476605b8fe4aeb9a28632c7995cf3ba831d97631d00000000","client_data_json":"{\"type\":\"webauthn.get\",\"challenge\":\"85VxczPqag4d7XrZFvpPZBBuac_cLiQZGONVSkdl9LE\",\"origin\":\"http://localhost:3000\",\"crossOrigin\":false}","message":"{\"parameters\":{\"block_height\":\"123456\",\"index\":\"0\",\"nonce\":\"1\"},\"account_id\":\"my-universal-account.testnet\",\"payload\":[{\"receiver_id\":\"alice.testnet\",\"actions\":[{\"Transfer\":{\"amount\":\"1000000000000000000000000\"}}]}]}","signature":"MEUCICy0TG2AuV8mOv-HEsTayGBiA4huWNJ5sUKzsQWt1xwnAiEA5_lulYfwRnf9dPSHBNciq63jrIFx0LAB519gQuJGxU8"}}
   ```

1. Send this message to the deployed contract:

   ```bash
   near contract call-function as-transaction <account-id> execute \
     json-args '{"args":<message>}' \
     prepaid-gas '100.0 Tgas' \
     attached-deposit '0 NEAR' \
     sign-as <any-account>
   ```

1. The action should be executed by the account.
