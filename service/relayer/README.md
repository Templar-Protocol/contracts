# Templar Relayer

The Templar Relayer is a service that will relay a [signed delegate action](https://nomicon.io/RuntimeSpec/Actions#delegate-actions) to the NEAR blockchain, paying for the transaction fees in the process. This allows accounts that do not hold NEAR tokens to still interact with the on-chain applications.

## Setup

### Development

Using the sample .env file `.env.sample`, copy it to `.env` and edit it to match your environment.

For development, you can start the relayer with a local Postgres database:

```bash
cd service/relayer
docker compose up
```

> [!NOTE]
>
> Be sure to run `cargo sqlx prepare` after changing SQL queries, otherwise the CI/CD will not be able to build the project.

### Production

Use the `relayer.Dockerfile` to build the relayer image:

```bash
docker build -f relayer.Dockerfile . -t templar-relayer
```

Upload the image to the server:

```bash
docker save templar-relayer | ssh -C user@server docker load
```

On the server, using Caddy:

`/etc/caddy/Caddyfile`:

```Caddyfile
templar-relayer.example.com {
    reverse_proxy localhost:3000
}
```

Run the relayer:

```bash
docker run \
    --network host \
    --env-file .env \
    --restart always \
    templar-relayer
```

## Help

```text
Usage: templar-relayer [OPTIONS] --database-url <DATABASE_URL> --account-id <ACCOUNT_ID> <--registry <REGISTRY>|--market <MARKET>>

Options:
  -p, --port <PORT>
          Run the relayer on this port [env: PORT=] [default: 3000]
      --database-url <DATABASE_URL>
          Postgres database connection URL [env: DATABASE_URL=]
      --rpc-url <RPC_URL>
          NEAR RPC connection URL [env: RPC_URL=] [default: https://rpc.testnet.near.org]
      --registry <REGISTRY>
          Comma-separated list of registries to query for markets to monitor [env: REGISTRY=]
      --market <MARKET>
          Comma-separated list of markets to monitor [env: MARKET=]
  -a, --account-id <ACCOUNT_ID>
          Account ID of the NEAR account that the relayer controls [env: ACCOUNT_ID=]
  -k, --secret-key <SECRET_KEY>
          Comma-separated list of private keys to use to sign transactions for the account that the relayer controls [env: SECRET_KEY=]
      --allowed-methods <ALLOWED_METHODS>
          Comma-separated list of allowed methods [env: ALLOWED_METHODS=] [default: borrow apply_interest harvest_yield withdraw_static_yield withdraw_collateral create_supply_withdrawal_request cancel_supply_withdrawal_request execute_next_supply_withdrawal_request storage_deposit]
      --starting-allowance-yocto <STARTING_ALLOWANCE_YOCTO>
          Starting allowance in yoctoNEAR [env: STARTING_ALLOWANCE_YOCTO=] [default: "0.25 NEAR"]
      --cache-gas-price-secs <CACHE_GAS_PRICE_SECS>
          Refresh the cached gas price after X seconds [env: CACHE_GAS_PRICE_SECS=] [default: 600]
      --cache-nonce-secs <CACHE_NONCE_SECS>
          Refresh a cached nonce after X seconds [env: CACHE_NONCE_SECS=] [default: 60]
      --broom-batch-size <BROOM_BATCH_SIZE>
          Broom batch size [env: BROOM_BATCH_SIZE=] [default: 16]
      --broom-interval-secs <BROOM_INTERVAL_SECS>
          Broom interval in seconds [env: BROOM_INTERVAL_SECS=] [default: 300]
  -h, --help
          Print help
```

## Routes

### `POST /relay`

This route will relay a [signed delegate action](https://nomicon.io/RuntimeSpec/Actions#delegate-actions) to the NEAR blockchain, paying for the transaction fees in the process.

The JSON body has one required field, `signed_delegate_action`, which contains the Borsh-serialized, base64-encoded signed delegate action.

In addition, there are two optional fields.

- `storage_deposit: bool` \
  If `true`, the relayer will attempt to pay the minimum [storage deposit](https://nomicon.io/Standards/StorageManagement) to the receiver of the delegate action on behalf of the delegating account. It will fail with an error if the receiver does not support storage deposits or if the account already has a storage balance. The amount paid to the account is deducted from the user's allowance.

- `wait_until: TxExecutionStatus` \
  If provided, the relayer will wait for the transaction to reach the specified status before returning. If not provided, the default is `TxExecutionStatus::ExecutedOptimistic`.

### `GET /get_allowance`

This route will return the current allowance of the relayer for the given account.
