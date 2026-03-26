# Universal Account

This smart contract accepts many different signature types and converts them into NEAR actions.

Currently supported:

- NIST-P256 (Webauthn passkey)
- Raw Ed25519 (Solana)
- Ethereum EIP-191 and EIP-712
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

## Upgrade

Older versions of the universal account contract can be upgraded to newer versions.

The first step in this process is always to deploy new contract code to the desired account. Since universal accounts are usually _locked_ (have no native NEAR full-access keys deployed), this should be done via a normal action execution.

Universal account contracts are usually deployed as global contract hashes, so to replace the currently-deployed code on a given account with new code, the account should execute a [`UseGlobalContract` action](https://docs.templarfi.org/doc/templar_universal_account/transaction/enum.Action.html#variant.DeployGlobalContract).

If the universal account was deployed from a registry and you wish to upgrade to a certain version of the contract deployed from that registry, view `get_version_code_hash({"version_key":"<version-key>"})` on the registry contract to retrieve the code hash. If using a relayer, the `version_key` string can be retrieved by `GET /universal_account`.

## Migration

Sometimes a version upgrade will require a migration, and sometimes it will not. Freshly initialized accounts store their current state version immediately. To determine if a migration is necessary after a code upgrade, perform a view call of `needs_migration`. You can also inspect `get_stored_state_version` and `get_target_state_version`. If `needs_migration` resolves to `false`, then no migration is required.

If a migration _is_ required, we prepare a migration payload in order to properly parameterize the migration. Multiple migrations may be required (e.g. the code was upgraded from state version 0 &rarr; 2, so it must first migrate to version 1, then to version 2). Only one state migration can occur at a time.

1. Choose the state migration to perform. The list of state migrations can be found [here](https://docs.templarfi.org/doc/templar_universal_account/state/migration/enum.Migration.html). For example, if the contract is being upgraded from state version 0 to state version 1, we choose `V0`. If it is then upgraded from state version 1 to state version 2, we choose `V1`.
2. Parameterize (if necessary). Some migrations require new information, some do not. In our example, the `V0` migration requires a `chain_id` parameter, so we choose `397` (NEAR Mainnet). The `V1` migration has no additional arguments.
3. Call `migrate` with the migration payload. For example: `migrate({"from_version":"v0","chain_id":"397"})`, then `migrate({"from_version":"v1"})` if another migration step is still required. Note that this function call is annotated with `#[private]`, meaning that the universal account itself is the only account that is allowed to call this function. Therefore, it may be executed as a `FunctionCall` action signed by one of the universal account's keys.

If the account was deployed with the buggy `0.4.0` contract, the serialized state is already `V1` but the `__v` storage key is still `0`. In that case, `V1` will fail because the stored version is incorrect. Use `UnbrickV1` instead to migrate that broken `0 -> 2` shape directly.

Generally speaking, it should be possible to combine a code upgrade and migration into a single transaction.
