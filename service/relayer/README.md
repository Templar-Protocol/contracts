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
docker build -f relayer.Dockerfile .
```

## How to use

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
