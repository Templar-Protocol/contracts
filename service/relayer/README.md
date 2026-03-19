# Templar Relayer

The Templar Relayer is a service that will relay a [signed delegate action](https://nomicon.io/RuntimeSpec/Actions#delegate-actions) to the NEAR blockchain, paying for the transaction fees in the process. This allows accounts that do not hold NEAR tokens to still interact with the on-chain applications.

## Setup

### Development

Using the sample .env file `.env.sample`, copy it to `.env` and edit it to match your environment.

For development, you can start the relayer with a local Postgres database:

```bash
cd service/relayer
docker compose -f compose.dev.yaml up
```

> [!NOTE]
>
> Be sure to run `cargo sqlx prepare` after changing SQL queries, otherwise the CI/CD will not be able to build the project.

#### RedStone Adapter

The relayer interfaces with a small JavaScript child process that runs the RedStone SDK.

Install dependencies:

```bash
cd ./redstone-bridge
npm install
```

To run the JavaScript tests, you _can_ run `npm test` from the `redstone-bridge` directory, however, the Rust test `test/bridge.rs` wraps this, so the JavaScript tests are also automatically run when you simply run `cargo test` in the crate.

#### SQL formatting

Install [sleek](https://sleek.dev) to format SQL queries, including queries inline in Rust source files:

```bash
cargo install --locked sleek
make sql-fmt # from project root
```

### Production

1. Build the `templar-relayer` image:

   ```bash
   docker compose -f compose.prod.yaml build
   ```

1. Upload the image to the server:

   ```bash
   scp compose.prod.yaml templar-relayer:~/compose.yaml
   docker save templar-relayer:latest | ssh -C templar-relayer docker load
   ```

1. Set up Caddy on the server:

   `/etc/caddy/Caddyfile`:

   ```Caddyfile
   templar-relayer.example.com {
       reverse_proxy localhost:3000
   }
   ```

1. Run the relayer server:

   ```bash
   docker compose up -d
   ```

## Help

```text
Usage: templar-relayer [OPTIONS] --database-url <DATABASE_URL> --relay-account-id <relay-account-id> --ua-account-id <ua-account-id> --ua-registry-id <ua-registry-id> --ua-version-key <ua-version-key> <--monitor-registry-id <monitor-registry-id>|--monitor-market-id <monitor-market-id>>

Options:
  -p, --port <PORT>
          Run the relayer on this port

          [env: PORT=]
          [default: 3000]

      --database-url <DATABASE_URL>
          Postgres database connection URL

          [env: DATABASE_URL=]

      --rpc-url <RPC_URL>
          NEAR RPC connection URL

          [env: RPC_URL=]
          [default: https://rpc.testnet.near.org]

      --monitor-registry-id <monitor-registry-id>
          Comma-separated list of registries to query for markets to monitor

          [env: MONITOR_REGISTRY_ID=]

      --monitor-market-id <monitor-market-id>
          Comma-separated list of markets to monitor

          [env: MONITOR_MARKET_ID=]

      --relay-account-id <relay-account-id>
          Account ID of the NEAR account that the relayer controls

          [env: RELAY_ACCOUNT_ID=]

      --relay-secret-key <relay-secret-key>
          Comma-separated list of private keys to use to sign transactions for the account that the relayer controls

          [env: RELAY_SECRET_KEY=]

      --allowed-methods <ALLOWED_METHODS>
          Comma-separated list of allowed methods

          [env: ALLOWED_METHODS=]
          [default: borrow apply_interest harvest_yield withdraw_static_yield withdraw_collateral create_supply_withdrawal_request cancel_supply_withdrawal_request execute_next_supply_withdrawal_request storage_deposit]

      --oracle-allowed-methods <ORACLE_ALLOWED_METHODS>
          Comma-separated list of allowed methods

          [env: ORACLE_ALLOWED_METHODS=]
          [default: update_price_feeds]

      --starting-allowance-yocto <STARTING_ALLOWANCE_YOCTO>
          Starting allowance in yoctoNEAR

          [env: STARTING_ALLOWANCE_YOCTO=]
          [default: "0.250 NEAR"]

      --storage-deposit-guarantee-minimum-available <STORAGE_DEPOSIT_GUARANTEE_MINIMUM_AVAILABLE>
          The relayer will ensure that the account has at least `max(this_value, contract.storage_balance_bounds.min)` storage balance available when requested

          [env: STORAGE_DEPOSIT_GUARANTEE_MINIMUM_AVAILABLE=]
          [default: "0.005 NEAR"]

      --intents-id <INTENTS_ID>
          Account ID of the NEAR Intents contract

          [env: INTENTS_ID=]

      --intents-allowed-methods <INTENTS_ALLOWED_METHODS>
          Comma-separated list of sponsored methods on the intents contract

          [env: INTENTS_ALLOWED_METHODS=]
          [default: add_public_key remove_public_key]

      --ua-account-id <ua-account-id>
          Account ID of the NEAR account that the relayer controls for universal account creation

          [env: UA_ACCOUNT_ID=]

      --ua-secret-key <ua-secret-key>
          Comma-separated list of private keys to use to sign universal account creation transactions

          [env: UA_SECRET_KEY=]

      --ua-pow-difficulty <ua-pow-difficulty>
          How difficult should the proof-of-work for universal account creation be?

          iterations ~ 2^difficulty

          [env: UA_POW_DIFFICULTY=]
          [default: 17]

      --ua-blockref-max-age-secs <ua-blockref-max-age-secs>
          How fresh must the universal account creation signature be?

          Based on the block hash referenced in the creation request.

          [env: UA_BLOCKREF_MAX_AGE_SECS=]
          [default: 600]

      --ua-allowed-origin <ua-allowed-origin>
          From which origins are the payloads allowed to come?

          This is checked in the `clientDataJSON` field provided by WebAuthn.

          [env: UA_ALLOWED_ORIGIN=]

      --ua-chain-id <ua-chain-id>
          Chain ID. NEAR mainnet = 397, NEAR testnet = 398

          [env: UA_CHAIN_ID=]
          [default: 398]

      --ua-registry-id <ua-registry-id>
          Account ID of the registry from which to deploy universal accounts

          [env: UA_REGISTRY_ID=]

      --ua-version-key <ua-version-key>
          Version key of the universal account contract to deploy from the registry

          [env: UA_VERSION_KEY=]

      --ua-execute-tgas <ua-execute-tgas>
          How much gas does it take to execute the `execute` receipt on the universal account contract?

          [env: UA_EXECUTE_TGAS=]
          [default: 35]

      --redstone-node-path <redstone-node-path>
          Path to Node.js interpreter (or equivalent)

          [env: REDSTONE_NODE_PATH=]
          [default: node]

      --redstone-bridge-path <redstone-bridge-path>
          Path to the Redstone bridge JS application

          [env: REDSTONE_BRIDGE_PATH=]
          [default: ./redstone-bridge/dist/index.js]

      --redstone-refresh-secs <redstone-refresh-secs>
          Do not push price updates to Redstone oracle if the last push was less than this long ago, even if requested

          [env: REDSTONE_REFRESH_SECS=]
          [default: 3]

      --redstone-update-gas <redstone-update-gas>
          How much gas (in units of Tgas) to attach to oracle price update calls

          [env: REDSTONE_UPDATE_GAS=]
          [default: "300 Tgas"]

      --redstone-update-deposit <redstone-update-deposit>
          How much NEAR to attach as a deposit to oracle price update calls

          [env: REDSTONE_UPDATE_DEPOSIT=]
          [default: "0 NEAR"]

      --pyth-hermes-url <HERMES_URL>
          Pyth Hermes API URL. See: <https://docs.pyth.network/price-feeds/core/api-reference>

          [env: PYTH_HERMES_URL=]
          [default: https://hermes-beta.pyth.network]

      --pyth-refresh-secs <pyth-refresh-secs>
          Do not push price updates to Pyth oracle if the last push was less than this long ago, even if requested

          [env: PYTH_REFRESH_SECS=]
          [default: 3]

      --pyth-timeout-secs <pyth-timeout-secs>
          HTTP timeout for Hermes requests (in seconds)

          [env: PYTH_TIMEOUT_SECS=]
          [default: 10]

      --pyth-update-gas <pyth-update-gas>
          How much gas (in units of Tgas) to attach to oracle price update calls

          [env: PYTH_UPDATE_GAS=]
          [default: "300 Tgas"]

      --pyth-update-deposit <pyth-update-deposit>
          How much NEAR to attach as a deposit to oracle price update calls

          [env: PYTH_UPDATE_DEPOSIT=]
          [default: "0.01 NEAR"]

      --cache-gase-price-secs <cache-gase-price-secs>
          Refresh the cached gas price after X seconds

          [env: CACHE_GAS_PRICE_SECS=]
          [default: 600]

      --cache-nonce-secs <cache-nonce-secs>
          Refresh a cached nonce after X seconds

          [env: CACHE_NONCE_SECS=]
          [default: 60]

      --broom-batch-size <BROOM_BATCH_SIZE>
          Broom batch size

          [env: BROOM_BATCH_SIZE=]
          [default: 16]

      --broom-interval-secs <BROOM_INTERVAL_SECS>
          Broom interval in seconds

          [env: BROOM_INTERVAL_SECS=]
          [default: 300]

  -h, --help
          Print help (see a summary with '-h')
```

## Routes

### `POST /relay`

This route will relay a [signed delegate action](https://nomicon.io/RuntimeSpec/Actions#delegate-actions) to the NEAR blockchain, paying for the transaction fees in the process.

The JSON body has one required field, `signed_delegate_action`, which contains the Borsh-serialized, base64-encoded signed delegate action.

In addition, there are three optional fields.

- `storage_deposit: bool` \
  If `true`, the relayer will attempt to pay the minimum [storage deposit](https://nomicon.io/Standards/StorageManagement) to the receiver of the delegate action on behalf of the delegating account. It will fail with an error if the receiver does not support storage deposits or if the account already has a storage balance. The amount paid to the account is deducted from the user's allowance.

- `update_prices: bool` \
  If `true`, the relayer will update the prices for the known market or markets touched by the relayed transaction before it submits the transaction. The relayer derives those markets from the transaction itself and applies its normal relayer-side oracle refresh cooldowns.

- `wait_until: TxExecutionStatus` \
  If provided, the relayer will wait for the transaction to reach the specified status before returning. If not provided, the default is `TxExecutionStatus::ExecutedOptimistic`.

### `POST /update_prices`

Requests price refreshes for one or more known markets.

Example payload:

```json
{
  "market_ids": [
    "templar-market-a.testnet",
    "templar-market-b.testnet"
  ]
}
```

- `market_ids` must not be empty.
- Every market ID must already be known to the relayer.
- Duplicate market IDs are ignored.
- The relayer updates the configured borrow and collateral price inputs for each market, subject to its normal oracle refresh cooldowns.

### `GET /get_allowance`

This route will return the current allowance of the relayer for the given account.

### `GET /universal_account`

Returns the configuration of the universal account deployer.

Example output:

```json
{
  "executor_id": "templar-universal-service.testnet",
  "registry_id": "templar-user.testnet",
  "pow_difficulty": 17,
  "blockref_max_age_secs": "600"
}
```

- This means that the payload that is signed by the user must authorize `templar-universal-service.testnet` to perform the account creation action by including that account ID in the payload that it signs.
- The user's account will be created as a subaccount of `templar-user.testnet`, e.g. `a8c80cd27e49.templar-user.testnet`.
- The payload that accompanies the universal account creation request must solve a proof-of-work with difficulty of 17. This means that, when the SHA-256^2 of the proof-of-work payload is evaluated as a binary string, it begins with 17 zeros.
- The universal account creation request must include the hash of a block on the NEAR blockchain that is less than 600 seconds (10 minutes) old.

### `GET /universal_account/account_id?type=Passkey&key=p256:...`

Calculates the account ID that the given key would be deployed to when it is created.

Example output:

```text
a8c80cd27e49.templar-user.testnet
```

### `POST /universal_account/create`

Creates a universal account for a public key.

Example payload:

```json
{
  "Passkey": {
    "authenticator_data": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "message": "{\"parameters\":{\"index\":\"0\",\"nonce\":\"0\"},\"account_id\":\"templar-universal-service.testnet\",\"payload\":{\"pow_nonce\":\"2\",\"key\":\"p256:PxHzrVcBARoJQ2VoSWxZWc1aRdjag746M2JtrYTtmFUMwNQbnhFKKbacfVLCCKA6FCYDMBqPcs1u4HZJZKqjmnZC\",\"block_hash\":\"DLHG9rM3ebTT5K8GZQQ76Zb1nq52zV5z4u9n5CjUMSgi\"}}",
    "client_data_json": "{\"type\":\"type\",\"challenge\":\"PwlNC6mRQtDdv7yMBTC4iINcj11TPUEXEGb-7mFyehA\",\"origin\":\"origin\",\"crossOrigin\":null,\"topOrigin\":null}",
    "signature": "MEYCIQD3nf1Ud8aDeVXyobQSyCtP9LUpnC2FHUX3d7G16rJupgIhANEy68mGEJPYuI2x7c_WKmvu9hDn6TXqLI1J4cr-vI7N"
  }
}
```

These are mostly values that are returned from a Webauthn implementation.

To break down the `"message"` string a little more:

```json
{
  "parameters": {
    "index": "0",
    "nonce": "0"
  },
  "account_id": "templar-universal-service.testnet",
  "payload": {
    "pow_nonce": "2",
    "key": "p256:PxHzrVcBARoJQ2VoSWxZWc1aRdjag746M2JtrYTtmFUMwNQbnhFKKbacfVLCCKA6FCYDMBqPcs1u4HZJZKqjmnZC",
    "block_hash": "DLHG9rM3ebTT5K8GZQQ76Zb1nq52zV5z4u9n5CjUMSgi"
  }
}
```

### `POST /universal_account/relay`

Relays a signed message from a user, paying for the gas costs of the `execute` call.

The request may also include:

- `storage_deposit: [AccountId, ...]` to top up storage for interacted contracts as before.
- `update_prices: bool` to tell the relayer to refresh the prices for the known markets touched by the relayed universal-account transaction before submitting it.

Example payload:

```json
{
  "account_id": "f92e7ab484da.templar-user.testnet",
  "update_prices": true,
  "args": {
    "Passkey": {
      "key": "p256:QE4spgPCif6HrYkGhk2UadjYDogYXq8ARBFnB2RXCqj3JCfcL4EgW7CjfwSZsXAUcB6aGx4pTnrWRzKeuwzMg4kM",
      "message": {
        "authenticator_data": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "message": "{\"parameters\":{\"index\":\"0\",\"nonce\":\"1\"},\"account_id\":\"f92e7ab484da.templar-user.testnet\",\"payload\":[{\"receiver_id\":\"templar-market.testnet\",\"actions\":[{\"FunctionCall\":{\"function_name\":\"apply_interest\",\"arguments\":\"e30=\",\"amount\":\"0\",\"gas\":\"250000000000000\"}}]}]}",
        "client_data_json": "{\"type\":\"type\",\"challenge\":\"Aa-q6ZFal54g0lr6mSRhsvSovDQza8hIMbaCXbYfi5Y\",\"origin\":\"origin\",\"crossOrigin\":null,\"topOrigin\":null}",
        "signature": "MEQCIGIUrXmGylCF2CLhqCDeGp7892z4gICxoba2ofswaEiOAiAe9Io4g3EaNdPKkOKI_c0ubyPqXWq1RwN06JsU06hHjw"
      }
    }
  }
}
```

Parsed `"message"`:

```json
{
  "parameters": {
    "index": "0",
    "nonce": "1"
  },
  "account_id": "f92e7ab484da.templar-user.testnet",
  "payload": [
    {
      "receiver_id": "templar-market.testnet",
      "actions": [
        {
          "FunctionCall": {
            "function_name": "apply_interest",
            "arguments": "e30=",
            "amount": "0",
            "gas": "250000000000000"
          }
        }
      ]
    }
  ]
}
```
